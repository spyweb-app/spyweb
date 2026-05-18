use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mlua::LuaSerdeExt;
use serde_json::Value;

use crate::cdp::browser::{Browser, find_browser_executable};
use crate::cdp::transport::CdpTransport;
use crate::cdp::types::LaunchOptions;
use crate::config::types::spyweb_job_profile_dir;

#[derive(Clone)]
struct PageHandle {
    inner: Arc<PageHandleInner>,
}

struct PageHandleInner {
    page_transport: Arc<CdpTransport>,
    browser_transport: Arc<CdpTransport>,
    target_id: String,
    closed: AtomicBool,
}

impl PageHandle {
    fn new(
        page_transport: Arc<CdpTransport>,
        browser_transport: Arc<CdpTransport>,
        target_id: String,
    ) -> Self {
        Self {
            inner: Arc::new(PageHandleInner {
                page_transport,
                browser_transport,
                target_id,
                closed: AtomicBool::new(false),
            }),
        }
    }

    fn close_best_effort(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }

        self.inner.page_transport.close();

        let browser_transport = Arc::clone(&self.inner.browser_transport);
        let target_id = self.inner.target_id.clone();
        smol::spawn(async move {
            let _ = browser_transport
                .call(
                    "Target.closeTarget",
                    serde_json::json!({ "targetId": target_id }),
                )
                .await;
        })
        .detach();
    }

    async fn close_async(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }

        let _ = self
            .inner
            .browser_transport
            .call(
                "Target.closeTarget",
                serde_json::json!({ "targetId": self.inner.target_id.clone() }),
            )
            .await;
        self.inner.page_transport.close();
    }
}

impl Drop for PageHandle {
    fn drop(&mut self) {
        self.close_best_effort();
    }
}

impl mlua::UserData for PageHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("close", |_, this, ()| async move {
            this.close_async().await;
            Ok(())
        });
    }
}

fn decode_base64(input: &str) -> mlua::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;

    for byte in input.bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b'\t' | b' ' => continue,
            _ => {
                return Err(mlua::Error::runtime(format!(
                    "invalid base64 byte: {}",
                    byte
                )));
            }
        } as u32;

        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }

    Ok(out)
}

fn parse_wait_args(
    args: mlua::Variadic<mlua::Value>,
) -> mlua::Result<(Option<u64>, Option<mlua::Function>)> {
    let mut timeout_ms = None;
    let mut predicate = None;

    for arg in args {
        match arg {
            mlua::Value::Nil => {}
            mlua::Value::Integer(ms) if ms >= 0 => timeout_ms = Some(ms as u64),
            mlua::Value::Number(ms) if ms >= 0.0 => timeout_ms = Some(ms as u64),
            mlua::Value::Function(f) => predicate = Some(f),
            mlua::Value::Table(t) => {
                timeout_ms = t
                    .get::<Option<u64>>("timeout_ms")?
                    .or(t.get::<Option<u64>>("timeout")?)
                    .or(timeout_ms);
                predicate = t.get::<Option<mlua::Function>>("predicate")?.or(predicate);
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "invalid wait_event argument: expected timeout, predicate, or options table; got {}",
                    other.type_name()
                )));
            }
        }
    }

    Ok((timeout_ms, predicate))
}

async fn wait_event_for_lua(
    lua: &mlua::Lua,
    transport: Arc<CdpTransport>,
    event: String,
    timeout_ms: Option<u64>,
    predicate: Option<mlua::Function>,
) -> mlua::Result<mlua::Table> {
    let deadline = timeout_ms.map(|ms| std::time::Instant::now() + Duration::from_millis(ms));

    loop {
        let remaining = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(mlua::Error::external(anyhow::anyhow!(
                        "Timeout waiting for CDP event {} after {}ms",
                        event,
                        timeout_ms.unwrap_or_default()
                    )));
                }
                Some(remaining)
            }
            None => None,
        };

        let result_event = transport
            .wait_event_timeout(&event, remaining, |_| true)
            .await
            .map_err(|e| {
                crate::t_eprintln!("wait_event failed: {}", e);
                mlua::Error::external(e)
            })?;

        if let Some(predicate) = &predicate {
            let params = lua.to_value(&result_event.params)?;
            if !predicate.call::<bool>(params)? {
                continue;
            }
        }

        let event_table = lua.create_table()?;
        event_table.set("method", result_event.method)?;
        event_table.set("params", lua.to_value(&result_event.params)?)?;
        event_table.set("session_id", result_event.session_id)?;
        return Ok(event_table);
    }
}

async fn create_page_table(
    lua: &mlua::Lua,
    browser_transport: Arc<CdpTransport>,
    browser_ws_url: String,
    target_id: String,
) -> mlua::Result<mlua::Table> {
    let origin = browser_ws_url
        .find("/devtools/")
        .map(|pos| &browser_ws_url[..pos])
        .unwrap_or(browser_ws_url.as_str());

    let page_ws_url = format!("{}/devtools/page/{}", origin, target_id);
    let page_transport = Arc::new(
        CdpTransport::connect(&page_ws_url)
            .await
            .map_err(mlua::Error::external)?,
    );
    let handle = lua.create_userdata(PageHandle::new(
        Arc::clone(&page_transport),
        Arc::clone(&browser_transport),
        target_id.clone(),
    ))?;

    let page = lua.create_table()?;
    page.set("_target_id", target_id.clone())?;
    page.set("__handle", handle)?;

    let ct = page_transport.clone();
    page.set(
        "call",
        lua.create_async_function(
            move |lua, (_self, method, params): (mlua::Value, String, mlua::Value)| {
                let ct = ct.clone();
                async move {
                    let params_json: Value = lua.from_value(params)?;
                    let result = ct
                        .call(&method, params_json)
                        .await
                        .map_err(mlua::Error::external)?;
                    lua.to_value(&result).map_err(mlua::Error::runtime)
                }
            },
        )?,
    )?;

    let ct_save = page_transport.clone();
    page.set(
        "call_save",
        lua.create_async_function(
            move |lua,
                (_self, method, params, path): (mlua::Value, String, mlua::Value, String)| {
                let ct = ct_save.clone();
                async move {
                    let params_json: Value = lua.from_value(params)?;
                    let mut result = ct
                        .call(&method, params_json)
                        .await
                        .map_err(mlua::Error::external)?;

                    if let Some(base64_data) = result.get("data").and_then(|v| v.as_str()) {
                        let base64_data = base64_data.to_string();
                        let saved_path = path.clone();
                        smol::unblock(move || {
                            let bytes = decode_base64(&base64_data)?;
                            std::fs::write(path, bytes).map_err(mlua::Error::external)
                        })
                        .await?;

                        if let Some(obj) = result.as_object_mut() {
                            obj.remove("data");
                            obj.insert("saved_to".to_string(), Value::String(saved_path) ); 
                        }
                    }

                    lua.to_value(&result).map_err(mlua::Error::runtime)
                }
            },
        )?,
    )?;

    let wt = page_transport.clone();
    page.set(
        "wait_event",
        lua.create_async_function(
            move |lua, (_self, event, args): (mlua::Value, String, mlua::Variadic<mlua::Value>)| {
                let wt = wt.clone();
                async move {
                    let (timeout_ms, predicate) = parse_wait_args(args)?;
                    wait_event_for_lua(&lua, wt, event, timeout_ms, predicate).await
                }
            },
        )?,
    )?;

    let close_handle = lua.create_async_function(move |_, page: mlua::Table| async move {
        let handle: mlua::AnyUserData = page.get("__handle")?;
        let handle = handle.borrow::<PageHandle>()?.clone();
        handle.close_async().await;
        Ok(())
    })?;
    page.set("close", close_handle)?;

    let cdp_table: mlua::Table = lua.globals().get("cdp")?;
    let inject: mlua::Function = cdp_table.get("_inject_page")?;
    inject.call::<()>(page.clone())?;

    Ok(page)
}

async fn create_page_in_target(
    lua: &mlua::Lua,
    browser_transport: Arc<CdpTransport>,
    browser_ws_url: String,
    options: Value,
) -> mlua::Result<mlua::Table> {
    let result = browser_transport
        .call("Target.createTarget", options)
        .await
        .map_err(mlua::Error::external)?;
    let target_id = result["targetId"]
        .as_str()
        .ok_or_else(|| {
            mlua::Error::runtime("Missing targetId in Target.createTarget response".to_string())
        })?
        .to_string();

    create_page_table(lua, browser_transport, browser_ws_url, target_id).await
}

impl mlua::UserData for Browser {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // browser:call("Page.navigate", { url = "https://example.com" })
        methods.add_async_method(
            "call",
            |lua, this, (method, params): (String, mlua::Value)| async move {
                let params_json: Value = lua.from_value(params)?;

                let result_json = this
                    .transport
                    .call(&method, params_json)
                    .await
                    .map_err(|e| {
                        crate::t_eprintln!("Method call failed: {}", e);
                        mlua::Error::external(e)
                    })?;

                lua.to_value(&result_json).map_err(mlua::Error::runtime)
            },
        );
        // methods.add_async_method(
        //     "call",
        //     |lua, this, (method, params): (String, mlua::Value)| async move {
        //         let params_json: Value = lua.from_value(params)?;
        //         let result_json = match this.transport.call(&method, params_json).await {
        //             Ok(r) => r,
        //             Err(e) => {
        //                 crate::t_eprintln!("Method call failed: {e}");
        //                 return Err(mlua::Error::external(anyhow::anyhow!("{e}")));
        //             }
        //         };

        //         lua.to_value(&result_json).map_err(mlua::Error::external)
        //     },
        // );

        // browser:wait_event("Page.loadEventFired", timeout_ms?, predicate?)
        methods.add_async_method(
            "wait_event",
            |lua, this, (event, args): (String, mlua::Variadic<mlua::Value>)| async move {
                let (timeout_ms, predicate) = parse_wait_args(args)?;
                wait_event_for_lua(
                    &lua,
                    Arc::clone(&this.transport),
                    event,
                    timeout_ms,
                    predicate,
                )
                .await
            },
        );

        // browser:close()
        methods.add_method_mut("close", |_, this, ()| {
            this.keep_alive = false;
            this.close();
            Ok(())
        });

        // browser:get_user_data_dir() -> string | nil
        methods.add_method("get_user_data_dir", |_, this, ()| {
            Ok(this
                .get_user_data_dir()
                .map(|p| p.to_string_lossy().to_string()))
        });

        // browser:attach() -> table { call, wait_event }
        // Creates a new page target and returns a Lua table with page-level
        // call/wait_event methods. Users can add their own methods to this table.
        methods.add_async_method(
            "attach",
            |lua, this, args: mlua::Variadic<mlua::Value>| async move {
                let opts = match args.first() {
                    Some(mlua::Value::Table(t)) => Some(t.clone()),
                    _ => None,
                };
                let browser_url = this
                    .browser_ws_url
                    .as_ref()
                    .ok_or_else(|| mlua::Error::runtime("No browser WS URL".to_string()))?
                    .to_string();
                let context_id = opts
                    .as_ref()
                    .and_then(|t| t.get::<Option<String>>("browserContextId").ok().flatten());
                let url = opts
                    .as_ref()
                    .and_then(|t| t.get::<Option<String>>("url").ok().flatten())
                    .unwrap_or_else(|| "about:blank".to_string());
                let reuse = opts
                    .as_ref()
                    .and_then(|t| t.get::<Option<bool>>("reuse").ok().flatten())
                    .unwrap_or(context_id.is_none());

                // Reuse existing blank page tab only for the default browser context.
                if reuse {
                    let targets = this
                        .transport
                        .call("Target.getTargets", serde_json::Value::Null)
                        .await
                        .map_err(mlua::Error::external)?;
                    let infos = targets["targetInfos"].as_array();
                    if let Some(existing) = infos.and_then(|arr| {
                        arr.iter().find(|t| {
                            t["type"].as_str() == Some("page")
                                && t["url"].as_str() == Some("about:blank")
                        })
                    }) {
                        let target_id = existing["targetId"]
                            .as_str()
                            .ok_or_else(|| {
                                mlua::Error::runtime(
                                    "missing or invalid targetId in CDP response".to_string(),
                                )
                            })?
                            .to_string();
                        return create_page_table(
                            &lua,
                            Arc::clone(&this.transport),
                            browser_url,
                            target_id,
                        )
                        .await;
                    }
                }

                let mut params = serde_json::json!({ "url": url });
                if let Some(context_id) = context_id {
                    params["browserContextId"] = Value::String(context_id);
                }

                create_page_in_target(&lua, Arc::clone(&this.transport), browser_url, params).await
            },
        );

        methods.add_async_method("new_context", |lua, this, _: ()| async move {
            let browser_url = this
                .browser_ws_url
                .as_ref()
                .ok_or_else(|| mlua::Error::runtime("No browser WS URL".to_string()))?
                .to_string();
            let result = this
                .transport
                .call("Target.createBrowserContext", serde_json::json!({}))
                .await
                .map_err(mlua::Error::external)?;
            let context_id = result["browserContextId"]
                .as_str()
                .ok_or_else(|| {
                    mlua::Error::runtime(
                        "Missing browserContextId in Target.createBrowserContext response"
                            .to_string(),
                    )
                })?
                .to_string();
            let context = lua.create_table()?;
            context.set("id", context_id.clone())?;

            let attach_transport = Arc::clone(&this.transport);
            let attach_browser_url = browser_url.clone();
            let attach_context_id = context_id.clone();
            context.set(
                "attach",
                lua.create_async_function(
                    move |lua, (_self, url): (mlua::Value, Option<String>)| {
                        let attach_transport = attach_transport.clone();
                        let attach_browser_url = attach_browser_url.clone();
                        let attach_context_id = attach_context_id.clone();
                        async move {
                            create_page_in_target(
                                &lua,
                                attach_transport,
                                attach_browser_url,
                                serde_json::json!({
                                    "url": url.unwrap_or_else(|| "about:blank".to_string()),
                                    "browserContextId": attach_context_id,
                                }),
                            )
                            .await
                        }
                    },
                )?,
            )?;

            let close_transport = Arc::clone(&this.transport);
            context.set(
                "close",
                lua.create_async_function(move |_, _self: mlua::Value| {
                    let close_transport = close_transport.clone();
                    let context_id = context_id.clone();
                    async move {
                        close_transport
                            .call(
                                "Target.disposeBrowserContext",
                                serde_json::json!({ "browserContextId": context_id }),
                            )
                            .await
                            .map_err(mlua::Error::external)?;
                        Ok(())
                    }
                })?,
            )?;

            Ok(context)
        });

        methods.add_async_method(
            "attach_session",
            |lua, this, target_id: String| async move {
                let result = this
                    .transport
                    .call(
                        "Target.attachToTarget",
                        serde_json::json!({ "targetId": target_id, "flatten": true }),
                    )
                    .await
                    .map_err(mlua::Error::external)?;
                lua.to_value(&result).map_err(mlua::Error::runtime)
            },
        );

        methods.add_async_method(
            "call_session",
            |lua, this, (session_id, method, params): (String, String, mlua::Value)| async move {
                let params_json: Value = lua.from_value(params)?;
                let result = this
                    .transport
                    .call_session(&session_id, &method, params_json)
                    .await
                    .map_err(mlua::Error::external)?;
                lua.to_value(&result).map_err(mlua::Error::runtime)
            },
        );

        methods.add_async_method(
            "wait_session_event",
            |lua,
             this,
             (session_id, event, args): (String, String, mlua::Variadic<mlua::Value>)| async move {
                let (timeout_ms, predicate) = parse_wait_args(args)?;
                let session_for_filter = session_id.clone();
                let deadline =
                    timeout_ms.map(|ms| std::time::Instant::now() + Duration::from_millis(ms));
                loop {
                    let remaining = match deadline {
                        Some(deadline) => {
                            let remaining =
                                deadline.saturating_duration_since(std::time::Instant::now());
                            if remaining.is_zero() {
                                return Err(mlua::Error::external(anyhow::anyhow!(
                                    "Timeout waiting for session event {} after {}ms",
                                    event,
                                    timeout_ms.unwrap_or_default()
                                )));
                            }
                            Some(remaining)
                        }
                        None => None,
                    };
                    let ev = this
                        .transport
                        .wait_event_timeout(&event, remaining, |params| {
                            if let Some(predicate) = &predicate {
                                let Ok(params) = lua.to_value(params) else {
                                    return false;
                                };
                                predicate.call::<bool>(params).unwrap_or(false)
                            } else {
                                true
                            }
                        })
                        .await
                        .map_err(mlua::Error::external)?;
                    if ev.session_id.as_deref() != Some(session_for_filter.as_str()) {
                        continue;
                    }
                    let event_table = lua.create_table()?;
                    event_table.set("method", ev.method)?;
                    event_table.set("params", lua.to_value(&ev.params)?)?;
                    event_table.set("session_id", ev.session_id)?;
                    return Ok(event_table);
                }
            },
        );
    }
}

pub fn register(lua: &mlua::Lua, job_dir: Option<PathBuf>) -> mlua::Result<()> {
    let cdp_table = lua.create_table()?;

    // cdp.connect("ws://127.0.0.1:9222/...", { ["Authorization"] = "Bearer ..." })
    cdp_table.set(
        "connect",
        lua.create_async_function(
            |_, (ws_url, headers): (String, Option<std::collections::HashMap<String, String>>)| async move {
                match Browser::connect_with_headers(&ws_url, headers).await {
                    Ok(browser) => Ok(browser),
                    Err(e) => {
                        crate::t_eprintln!("WS connection failed: {}", e);
                        Err(mlua::Error::external(anyhow::anyhow!("{e}")))
                    }
                }
            },
        )?,
    )?;

    // cdp.get_browser() -> string
    cdp_table.set(
        "get_browser",
        lua.create_async_function(|_, ()| async move {
            Ok(smol::unblock(find_browser_executable).await)
        })?,
    )?;

    cdp_table.set(
        "sleep",
        lua.create_async_function(|_, ms: u64| async move {
            smol::Timer::after(Duration::from_millis(ms)).await;
            Ok(())
        })?,
    )?;

    cdp_table.set(
        "_write_base64",
        lua.create_async_function(|_, (path, data): (String, String)| async move {
            smol::unblock(move || {
                let bytes = decode_base64(&data)?;
                std::fs::write(path, bytes).map_err(mlua::Error::external)
            })
            .await
        })?,
    )?;

    cdp_table.set(
        "_log_terminal",
        lua.create_function(|_, message: String| {
            crate::t_eprintln!("{}", message);
            Ok(())
        })?,
    )?;

    // cdp.launch({ executable = "...", headless = true, ... })
    cdp_table.set(
        "launch",
        lua.create_async_function(move |_, opts: mlua::Table| {
            let job_dir = job_dir.clone();
            async move {
                let default = LaunchOptions::default();

                // If no user_data_dir is provided by Lua, use ~/.spyweb/<job-folder>.
                let mut user_data_dir = opts
                    .get::<Option<String>>("user_data_dir")?
                    .map(PathBuf::from);

                if user_data_dir.is_none()
                    && let Some(ref dir) = job_dir
                {
                    user_data_dir = spyweb_job_profile_dir(dir).or_else(|| {
                        let fallback = dir.join(".browser");
                        Some(fallback)
                    });
                }

                let options = LaunchOptions {
                    executable: opts.get("executable").unwrap_or(default.executable),
                    headless: opts.get("headless").unwrap_or(default.headless),
                    keep_alive: opts.get("keep_alive").unwrap_or(default.keep_alive),
                    user_data_dir,
                    args: opts.get("args").unwrap_or(default.args),
                };

                match Browser::launch(options).await {
                    Ok(browser) => Ok(browser),
                    Err(e) => {
                        crate::t_eprintln!("Browser launch failed: {}", e);
                        Err(mlua::Error::external(anyhow::anyhow!("{e}")))
                    }
                }
            }
        })?,
    )?;

    lua.globals().set("cdp", cdp_table)?;

    lua.load(include_str!("../globals/cdp.lua"))
        .set_name("cdp.lua")
        .exec()
        .map_err(|e| mlua::Error::runtime(format!("Failed to load cdp.lua: {e}")))?;

    Ok(())
}
