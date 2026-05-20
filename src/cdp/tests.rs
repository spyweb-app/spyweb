use crate::cdp::transport::CdpTransport;
use crate::cdp::types::JsonRpcMessage;
use serde_json::json;

#[test]
fn test_json_rpc_parsing() {
    let response_json = r#"{"id": 1, "result": {"foo": "bar"}}"#;
    let parsed: JsonRpcMessage = serde_json::from_str(response_json).unwrap();
    match parsed {
        JsonRpcMessage::Response { id, result, .. } => {
            assert_eq!(id, 1);
            assert_eq!(result.unwrap(), json!({"foo": "bar"}));
        }
        _ => panic!("Expected Response"),
    }

    let notification_json = r#"{"method": "Target.targetCreated", "params": {"targetInfo": {}}}"#;
    let parsed: JsonRpcMessage = serde_json::from_str(notification_json).unwrap();
    match parsed {
        JsonRpcMessage::Notification { method, params, .. } => {
            assert_eq!(method, "Target.targetCreated");
            assert!(params.is_some());
        }
        _ => panic!("Expected Notification"),
    }
}

#[test]
fn test_transport_connect_invalid_url() {
    smol::block_on(async {
        let result = CdpTransport::connect("ws://127.0.0.1:1").await;
        assert!(result.is_err());
    });
}

#[test]
fn test_is_cdp_compatible() {
    use crate::cdp::browser::is_cdp_compatible;
    assert!(is_cdp_compatible("google-chrome"));
    assert!(is_cdp_compatible("Chromium"));
    assert!(is_cdp_compatible("Brave-Browser"));
    assert!(is_cdp_compatible("microsoft-edge"));
    assert!(is_cdp_compatible("msedge.exe"));
    assert!(!is_cdp_compatible("firefox"));
    assert!(!is_cdp_compatible("safari"));
}

#[test]
fn test_find_browser_executable_not_empty() {
    use crate::cdp::browser::find_browser_executable;
    let bin = find_browser_executable();
    assert!(!bin.is_empty(), "Should at least return a fallback string");
}

#[test]
fn test_flat_session_routing() {
    smol::block_on(async {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local_addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}", local_addr);

        // Spawn mock server
        smol::spawn(async move {
            let (stream, _) = listener.accept().unwrap();
            let async_stream = smol::Async::new(stream).unwrap();
            let mut ws = async_tungstenite::accept_async(async_stream).await.unwrap();

            while let Some(Ok(msg)) = ws.next().await {
                if let Message::Text(text) = msg {
                    let req: serde_json::Value = serde_json::from_str(&text).unwrap();
                    let id = req["id"].as_u64().unwrap();
                    let method = req["method"].as_str().unwrap();

                    if method == "Target.attachToTarget" {
                        let resp = serde_json::json!({
                            "id": id,
                            "result": { "sessionId": "mock-session-id" }
                        });
                        ws.send(Message::Text(resp.to_string().into()))
                            .await
                            .unwrap();
                    } else if method == "Page.navigate" {
                        assert_eq!(req["sessionId"].as_str(), Some("mock-session-id"));
                        let resp = serde_json::json!({
                            "id": id,
                            "result": { "frameId": "mock-frame" }
                        });
                        ws.send(Message::Text(resp.to_string().into()))
                            .await
                            .unwrap();

                        let notification = serde_json::json!({
                            "method": "Page.loadEventFired",
                            "params": { "timestamp": 123.45 },
                            "sessionId": "mock-session-id"
                        });
                        ws.send(Message::Text(notification.to_string().into()))
                            .await
                            .unwrap();
                    }
                }
            }
        })
        .detach();

        use async_tungstenite::tungstenite::Message;
        use futures_lite::StreamExt;

        // Connect CdpTransport client
        let transport = CdpTransport::connect(&ws_url).await.unwrap();

        // 1. Test attach session
        let attach_res = transport
            .call(
                "Target.attachToTarget",
                serde_json::json!({ "targetId": "page-1", "flatten": true }),
            )
            .await
            .unwrap();
        let session_id = attach_res["sessionId"].as_str().unwrap().to_string();
        assert_eq!(session_id, "mock-session-id");

        // 2. Register session listener
        let rx = transport.register_listener(Some(session_id.clone()));

        // 3. Test call_session
        let nav_res = transport
            .call_session(
                &session_id,
                "Page.navigate",
                serde_json::json!({ "url": "http://example.com" }),
            )
            .await
            .unwrap();
        assert_eq!(nav_res["frameId"].as_str(), Some("mock-frame"));

        // 4. Test event routing to session
        let event = rx.recv().await.unwrap();
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session_id.as_deref(), Some("mock-session-id"));

        transport.unregister_session_listener(&session_id);
    });
}
