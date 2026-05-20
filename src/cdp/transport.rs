use async_channel::{Receiver, Sender};
use async_tungstenite::tungstenite::Message;
use futures_channel::oneshot;
use futures_lite::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::cdp::types::{CdpEvent, JsonRpcMessage};

pub struct CdpTransport {
    // Send raw text frames to the write loop
    ws_tx: Sender<String>,

    // Pending requests: id → oneshot sender waiting for response
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>>,

    // Event routing: key is Some(sessionId) for sessions, None for main transport.
    // Multiple listeners can subscribe to the same key.
    listeners: Arc<Mutex<HashMap<Option<String>, Vec<Sender<CdpEvent>>>>>,

    // Monotonically increasing request ID
    next_id: Arc<AtomicU64>,
}

impl CdpTransport {
    /// Connect to a CDP WebSocket endpoint and spawn read/write loops.
    pub async fn connect(ws_url: &str) -> anyhow::Result<Self> {
        Self::connect_with_headers(ws_url, None).await
    }

    pub async fn connect_with_headers(
        ws_url: &str,
        headers: Option<HashMap<String, String>>,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        use async_tungstenite::tungstenite::client::IntoClientRequest;

        let mut request = ws_url.into_client_request()?;
        if let Some(headers) = headers {
            let h = request.headers_mut();
            for (k, v) in headers {
                h.insert(
                    k.parse::<http::header::HeaderName>()?,
                    v.parse::<http::HeaderValue>()?,
                );
            }
        }

        let (ws_stream, _response) = async_tungstenite::smol::connect_async(request)
            .await
            .context(format!("Failed to connect WebSocket to {}", ws_url))?;

        let (ws_write, ws_read) = ws_stream.split();

        // Channel for outgoing WebSocket frames
        let (ws_tx, ws_rx) = async_channel::unbounded::<String>();

        // Pending request map shared between caller and read loop
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<anyhow::Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let listeners: Arc<Mutex<HashMap<Option<String>, Vec<Sender<CdpEvent>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn write loop — drains ws_rx and writes frames to WebSocket
        {
            let mut ws_write = ws_write;
            smol::spawn(async move {
                while let Ok(msg) = ws_rx.recv().await {
                    if ws_write.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        // Spawn read loop — routes incoming messages to pending or event listeners.
        {
            let pending = Arc::clone(&pending);
            let listeners = Arc::clone(&listeners);
            let mut ws_read = ws_read;

            smol::spawn(async move {
                let mut disconnect_error = None;

                while let Some(Ok(frame)) = ws_read.next().await {
                    let text = match frame {
                        Message::Text(t) => t.to_string(),
                        Message::Close(_) => {
                            disconnect_error = Some("WebSocket closed".to_string());
                            break;
                        }
                        _ => continue,
                    };

                    let parsed: JsonRpcMessage = match serde_json::from_str(&text) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };

                    match parsed {
                        JsonRpcMessage::Response {
                            id, result, error, ..
                        } => {
                            let sender = pending.lock().unwrap().remove(&id);
                            if let Some(tx) = sender {
                                let value = if let Some(err) = error {
                                    Err(anyhow::anyhow!(
                                        "CDP protocol error {}: {}",
                                        err.code,
                                        err.message
                                    ))
                                } else {
                                    Ok(result.unwrap_or(Value::Null))
                                };
                                let _ = tx.send(value);
                            }
                        }
                        JsonRpcMessage::Notification {
                            method,
                            params,
                            session_id,
                        } => {
                            let event = CdpEvent {
                                method,
                                params: params.unwrap_or(Value::Null),
                                session_id: session_id.clone(),
                            };

                            let list = {
                                let mut guard = listeners.lock().unwrap();
                                if let Some(list) = guard.get_mut(&session_id) {
                                    // Clean up closed listeners
                                    list.retain(|tx| !tx.is_closed());
                                    list.clone()
                                } else {
                                    Vec::new()
                                }
                            };

                            for tx in list {
                                let _ = tx.send(event.clone()).await;
                            }
                        }
                    }
                }

                let err =
                    disconnect_error.unwrap_or_else(|| "WebSocket read loop ended".to_string());
                let pending = {
                    let mut guard = pending.lock().unwrap();
                    std::mem::take(&mut *guard)
                };
                for (_, tx) in pending {
                    let _ = tx.send(Err(anyhow::anyhow!(err.clone())));
                }
            })
            .detach();
        }

        Ok(Self {
            ws_tx,
            pending,
            listeners,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Send a CDP command and wait for its JSON-RPC response.
    pub async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.call_inner(method, params, None).await
    }

    pub async fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        self.call_inner(method, params, Some(session_id)).await
    }

    async fn call_inner(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        self.pending.lock().unwrap().insert(id, tx);

        let mut request = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session_id) = session_id {
            request["sessionId"] = Value::String(session_id.to_string());
        }

        if self.ws_tx.send(request.to_string()).await.is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(anyhow::anyhow!("WebSocket write channel closed"));
        }

        rx.await
            .map_err(|_| anyhow::anyhow!("Response channel dropped before reply"))?
    }

    /// Wait for a CDP event matching the given name and predicate.
    pub async fn wait_event<F>(&self, event_name: &str, predicate: F) -> anyhow::Result<CdpEvent>
    where
        F: Fn(&Value) -> bool,
    {
        self.wait_event_session(None, event_name, predicate).await
    }

    pub async fn wait_event_session<F>(
        &self,
        session_id: Option<String>,
        event_name: &str,
        predicate: F,
    ) -> anyhow::Result<CdpEvent>
    where
        F: Fn(&Value) -> bool,
    {
        let rx = self.register_listener(session_id);
        loop {
            let event = rx
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("Transport event channel closed"))?;

            if event.method == event_name && predicate(&event.params) {
                return Ok(event);
            }
        }
    }

    pub async fn wait_event_timeout<F>(
        &self,
        event_name: &str,
        timeout: Option<Duration>,
        predicate: F,
    ) -> anyhow::Result<CdpEvent>
    where
        F: Fn(&Value) -> bool,
    {
        self.wait_event_session_timeout(None, event_name, timeout, predicate)
            .await
    }

    pub async fn wait_event_session_timeout<F>(
        &self,
        session_id: Option<String>,
        event_name: &str,
        timeout: Option<Duration>,
        predicate: F,
    ) -> anyhow::Result<CdpEvent>
    where
        F: Fn(&Value) -> bool,
    {
        if let Some(timeout) = timeout {
            smol::future::or(
                self.wait_event_session(session_id, event_name, predicate),
                async move {
                    smol::Timer::after(timeout).await;
                    Err(anyhow::anyhow!(
                        "Timeout waiting for CDP event {} after {}ms",
                        event_name,
                        timeout.as_millis()
                    ))
                },
            )
            .await
        } else {
            self.wait_event_session(session_id, event_name, predicate)
                .await
        }
    }

    pub fn register_listener(&self, session_id: Option<String>) -> Receiver<CdpEvent> {
        let (tx, rx) = async_channel::unbounded();
        self.listeners
            .lock()
            .unwrap()
            .entry(session_id)
            .or_default()
            .push(tx);
        rx
    }

    pub fn register_session_listener(&self, session_id: &str) -> Receiver<CdpEvent> {
        self.register_listener(Some(session_id.to_string()))
    }

    pub fn unregister_session_listener(&self, session_id: &str) {
        self.listeners
            .lock()
            .unwrap()
            .remove(&Some(session_id.to_string()));
    }

    pub fn close(&self) {
        self.ws_tx.close();
        let pending = {
            let mut guard = self.pending.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for (_, tx) in pending {
            let _ = tx.send(Err(anyhow::anyhow!("CDP transport closed")));
        }
    }
}
