use crate::lua::conversions::error_kind;
use crate::services::http::{self, MultipartField, RawResponse};
use crate::services::tls;
use mlua::{Lua, Result as LuaResult, Value};
use std::collections::HashMap;

fn raw_response_to_table(lua: &Lua, raw: RawResponse) -> Result<mlua::Table, String> {
    let headers_table = lua.create_table().map_err(|e| format!("lua error: {e}"))?;
    for (k, v) in &raw.headers {
        headers_table
            .set(k.as_str(), v.as_str())
            .map_err(|e| format!("lua error: {e}"))?;
    }

    let table = lua.create_table().map_err(|e| format!("lua error: {e}"))?;
    table
        .set("url", raw.url.as_str())
        .map_err(|e| format!("lua error: {e}"))?;
    table
        .set("status", raw.status)
        .map_err(|e| format!("lua error: {e}"))?;
    table
        .set("headers", headers_table)
        .map_err(|e| format!("lua error: {e}"))?;
    table
        .set("body", raw.body.as_str())
        .map_err(|e| format!("lua error: {e}"))?;
    if let Some(ref proxy) = raw.proxy {
        table
            .set("proxy", proxy.as_str())
            .map_err(|e| format!("lua error: {e}"))?;
    }
    table
        .set("time_ms", raw.time_ms)
        .map_err(|e| format!("lua error: {e}"))?;
    table
        .set("size", raw.size)
        .map_err(|e| format!("lua error: {e}"))?;
    Ok(table)
}

fn make_success(lua: &Lua, raw: RawResponse) -> Result<(Option<Value>, Option<Value>), String> {
    let table = raw_response_to_table(lua, raw)?;
    Ok((Some(Value::Table(table)), None))
}

fn make_error(
    lua: &Lua,
    msg: String,
    proxy: Option<&str>,
) -> Result<(Option<Value>, Option<Value>), String> {
    let err_table = lua.create_table().map_err(|e| format!("lua error: {e}"))?;
    err_table
        .set("error", msg.as_str())
        .map_err(|e| format!("lua error: {e}"))?;
    err_table
        .set("kind", error_kind(&msg))
        .map_err(|e| format!("lua error: {e}"))?;
    if let Some(p) = proxy {
        err_table
            .set("proxy", p)
            .map_err(|e| format!("lua error: {e}"))?;
    }
    Ok((None, Some(Value::Table(err_table))))
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    let http_request = lua.create_async_function(|lua, args: mlua::Table| async move {
        let method: String = args
            .get::<Option<String>>("method")?
            .unwrap_or_else(|| "GET".into());
        let url: String = args
            .get::<Option<String>>("url")?
            .ok_or_else(|| mlua::Error::runtime("http_request: 'url' is required"))?;
        let body: Option<mlua::String> = args.get("body").ok().flatten();
        let body_bytes = body.as_ref().map(|s| s.as_bytes().to_vec());
        let headers: Option<HashMap<String, String>> = args.get("headers").ok().flatten();
        let proxy: Option<String> = args.get("proxy").ok().flatten();
        let proxy_for_error = proxy.clone();
        let timeout: Option<u64> = args.get("timeout").ok().flatten();
        let max_body_mb: Option<u64> = args.get("max_body_size").ok().flatten();
        let max_body_bytes = max_body_mb.map(|mb| mb * 1024 * 1024);

        let result = smol::unblock(move || {
            http::do_http(
                &method,
                &url,
                body_bytes.as_deref(),
                headers.as_ref(),
                proxy.as_deref(),
                timeout,
                max_body_bytes,
            )
        })
        .await;

        match result {
            Ok(raw) => make_success(&lua, raw).map_err(mlua::Error::runtime),
            Err(msg) => {
                make_error(&lua, msg, proxy_for_error.as_deref()).map_err(mlua::Error::runtime)
            }
        }
    })?;

    let http_get = lua.create_async_function(
        |lua, (url, headers): (String, Option<HashMap<String, String>>)| async move {
            let result = smol::unblock(move || {
                http::do_http("GET", &url, None, headers.as_ref(), None, None, None)
            })
            .await;

            match result {
                Ok(raw) => make_success(&lua, raw).map_err(mlua::Error::runtime),
                Err(msg) => make_error(&lua, msg, None).map_err(mlua::Error::runtime),
            }
        },
    )?;

    let http_post = lua.create_async_function(
        |lua, (url, body, headers): (String, String, Option<HashMap<String, String>>)| async move {
            let body_bytes = body.into_bytes();
            let result = smol::unblock(move || {
                http::do_http(
                    "POST",
                    &url,
                    Some(&body_bytes),
                    headers.as_ref(),
                    None,
                    None,
                    None,
                )
            })
            .await;

            match result {
                Ok(raw) => make_success(&lua, raw).map_err(mlua::Error::runtime),
                Err(msg) => make_error(&lua, msg, None).map_err(mlua::Error::runtime),
            }
        },
    )?;

    let http_multipart = lua.create_async_function(
        |lua, (url, fields, headers): (String, mlua::Table, Option<HashMap<String, String>>)| async move {
            let mut field_list: Vec<(String, MultipartField)> = Vec::new();
            for pair in fields.pairs::<String, mlua::Value>() {
                let (name, value) = pair?;
                match value {
                    mlua::Value::String(s) => {
                        let text = s.to_str().map(|s| s.to_string()).unwrap_or_else(|_| String::new());
                        field_list.push((name, MultipartField::Text(text)));
                    }
                    mlua::Value::Table(t) => {
                        let content: mlua::String = t.get("content").map_err(|_| {
                            mlua::Error::runtime("http_multipart: file field must have 'content' key")
                        })?;
                        let content_bytes = content.as_bytes().to_vec();
                        let filename: Option<String> = t.get("filename").ok().flatten();
                        let mime_type: Option<String> = t.get("type").ok().flatten();
                        field_list.push((name, MultipartField::File {
                            content: content_bytes,
                            filename,
                            mime_type,
                        }));
                    }
                    _ => {
                        return Err(mlua::Error::runtime(
                            "http_multipart: field values must be strings or tables"
                        ));
                    }
                }
            }

            let lua_for_unblock = lua.clone();
            let result: Result<(Option<Value>, Option<Value>), String> = smol::unblock(move || {
                use ureq::unversioned::multipart::{Form, Part};

                let agent = http::build_agent(None, std::time::Duration::from_secs(http::DEFAULT_TIMEOUT_SECS))?;

                let mut form = Form::new();
                for (name, field) in &field_list {
                    match field {
                        MultipartField::Text(text) => {
                            form = form.text(name, text);
                        }
                        MultipartField::File { content, filename, mime_type } => {
                            let mut part = Part::bytes(content);
                            if let Some(fname) = filename {
                                part = part.file_name(fname);
                            }
                            if let Some(mime) = mime_type {
                                part = part.mime_str(mime)
                                    .map_err(|e| format!("http_multipart: invalid mime type: {e}"))?;
                            }
                            form = form.part(name, part);
                        }
                    }
                }

                let mut req = agent.post(&url);
                if let Some(h) = headers.as_ref() {
                    for (k, v) in h {
                        req = req.header(k, v);
                    }
                }

                let mut response = req
                    .send(form)
                    .map_err(|e| format!("http_multipart failed: {e}"))?;
                let status = response.status().as_u16();
                let headers = http::collect_headers(response.headers());
                let body = http::read_response_body(response.body_mut(), http::MAX_RESPONSE_BODY)?;
                let elapsed = std::time::Instant::now().elapsed().as_millis() as u64;
                let size = body.len() as u64;
                let raw = RawResponse {
                    url: url.clone(),
                    status,
                    headers,
                    body,
                    proxy: None,
                    time_ms: elapsed,
                    size,
                };
                raw_response_to_table(&lua_for_unblock, raw).map(|t| (Some(Value::Table(t)), None))
            })
            .await;

            match result {
                Ok(success) => Ok(success),
                Err(msg) => make_error(&lua, msg, None)
                    .map_err(mlua::Error::runtime),
            }
        },
    )?;

    lua.globals().set("http_request", http_request)?;
    lua.globals().set("http_get", http_get)?;
    lua.globals().set("http_post", http_post)?;
    lua.globals().set("http_multipart", http_multipart)?;

    // tls_probe(host, port?) -> table | (nil, error)
    let tls_probe =
        lua.create_async_function(|_lua, (host, port): (String, Option<u16>)| async move {
            let port = port.unwrap_or(443);
            let result = smol::unblock(move || tls::do_tls_probe(&host, port)).await;
            match result {
                Ok(info) => {
                    let table = _lua.create_table()?;
                    table.set("subject", info.subject)?;
                    table.set("issuer", info.issuer)?;
                    table.set("serial", info.serial)?;
                    table.set("not_before", info.not_before)?;
                    table.set("not_after", info.not_after)?;
                    table.set("days_left", info.days_left)?;
                    table.set("fingerprint", info.fingerprint)?;
                    Ok((Value::Table(table), Value::Nil))
                }
                Err(msg) => Ok((Value::Nil, Value::String(_lua.create_string(&msg)?))),
            }
        })?;
    lua.globals().set("tls_probe", tls_probe)?;

    Ok(())
}
