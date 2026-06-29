use crate::lua::conversions::error_kind;
use mlua::{Lua, Result as LuaResult, Value};
use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};
use ureq::{Agent, Proxy};

const MAX_RESPONSE_BODY: u64 = 10 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;

struct RawResponse {
    url: String,
    status: u16,
    headers: HashMap<String, String>,
    body: String,
    proxy: Option<String>,
    time_ms: u64,
    size: u64,
}

impl RawResponse {
    fn into_lua_table(self, lua: &Lua) -> Result<mlua::Table, String> {
        let headers_table = lua.create_table().map_err(|e| format!("lua error: {e}"))?;
        for (k, v) in &self.headers {
            headers_table
                .set(k.as_str(), v.as_str())
                .map_err(|e| format!("lua error: {e}"))?;
        }

        let table = lua.create_table().map_err(|e| format!("lua error: {e}"))?;
        table
            .set("url", self.url.as_str())
            .map_err(|e| format!("lua error: {e}"))?;
        table
            .set("status", self.status)
            .map_err(|e| format!("lua error: {e}"))?;
        table
            .set("headers", headers_table)
            .map_err(|e| format!("lua error: {e}"))?;
        table
            .set("body", self.body.as_str())
            .map_err(|e| format!("lua error: {e}"))?;
        if let Some(ref proxy) = self.proxy {
            table
                .set("proxy", proxy.as_str())
                .map_err(|e| format!("lua error: {e}"))?;
        }
        table
            .set("time_ms", self.time_ms)
            .map_err(|e| format!("lua error: {e}"))?;
        table
            .set("size", self.size)
            .map_err(|e| format!("lua error: {e}"))?;
        Ok(table)
    }
}

enum MultipartField {
    Text(String),
    File {
        content: Vec<u8>,
        filename: Option<String>,
        mime_type: Option<String>,
    },
}

fn collect_headers(headers: &ureq::http::HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value) in headers.iter() {
        let v = value.to_str().unwrap_or_default();
        map.entry(name.as_str().to_owned())
            .and_modify(|existing: &mut String| {
                existing.push_str(", ");
                existing.push_str(v);
            })
            .or_insert_with(|| v.to_owned());
    }
    map
}

fn read_response_body(body: &mut ureq::Body, max_bytes: u64) -> Result<String, String> {
    let mut buf = Vec::new();
    body.as_reader()
        .take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read response body failed: {e}"))?;
    if buf.len() as u64 > max_bytes {
        return Err(format!("response body exceeds {} byte limit", max_bytes));
    }
    String::from_utf8(buf).map_err(|e| format!("response body is not valid UTF-8: {e}"))
}

fn build_agent(proxy_url: Option<&str>, timeout: Duration) -> Result<Agent, String> {
    let mut config = Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();

    if let Some(proxy_url) = proxy_url {
        let proxy = Proxy::new(proxy_url)
            .map_err(|e| format!("invalid proxy url '{}': {}", proxy_url, e))?;
        config = Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .proxy(Some(proxy))
            .build();
    }

    Ok(config.into())
}

fn do_http(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    headers: Option<&HashMap<String, String>>,
    proxy: Option<&str>,
    timeout_secs: Option<u64>,
    max_body_bytes: Option<u64>,
) -> Result<RawResponse, String> {
    let start = Instant::now();
    let effective_timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let effective_max = max_body_bytes.unwrap_or(MAX_RESPONSE_BODY);
    let agent = build_agent(proxy, effective_timeout)?;

    let method_upper = method.to_uppercase();
    let has_body = matches!(method_upper.as_str(), "POST" | "PUT" | "PATCH");

    if has_body {
        let body_data = body.ok_or_else(|| format!("http method {method} requires a body"))?;
        let mut req = match method_upper.as_str() {
            "POST" => agent.post(url),
            "PUT" => agent.put(url),
            "PATCH" => agent.patch(url),
            _ => return Err(format!("unsupported http method: {method}")),
        };

        let mut content_type_set = false;
        if let Some(h) = headers {
            for (k, v) in h {
                if k.eq_ignore_ascii_case("content-type") {
                    content_type_set = true;
                }
                req = req.header(k, v);
            }
        }
        if !content_type_set {
            req = req.header("Content-Type", "application/x-www-form-urlencoded");
        }

        let mut response = req
            .send(body_data)
            .map_err(|e| format!("http_{} failed: {e}", method.to_lowercase()))?;
        let status = response.status().as_u16();
        let headers = collect_headers(response.headers());
        let body = read_response_body(response.body_mut(), effective_max)?;
        let elapsed = start.elapsed().as_millis() as u64;
        let size = body.len() as u64;
        Ok(RawResponse {
            url: url.to_string(),
            status,
            headers,
            body,
            proxy: proxy.map(String::from),
            time_ms: elapsed,
            size,
        })
    } else {
        let mut req = match method_upper.as_str() {
            "GET" => agent.get(url),
            "HEAD" => agent.head(url),
            "DELETE" => agent.delete(url),
            _ => return Err(format!("unsupported http method: {method}")),
        };

        if let Some(h) = headers {
            for (k, v) in h {
                req = req.header(k, v);
            }
        }

        let mut response = req
            .call()
            .map_err(|e| format!("http_{} failed: {e}", method.to_lowercase()))?;
        let status = response.status().as_u16();
        let headers = collect_headers(response.headers());
        let body = read_response_body(response.body_mut(), effective_max)?;
        let elapsed = start.elapsed().as_millis() as u64;
        let size = body.len() as u64;
        Ok(RawResponse {
            url: url.to_string(),
            status,
            headers,
            body,
            proxy: proxy.map(String::from),
            time_ms: elapsed,
            size,
        })
    }
}

fn make_success(lua: &Lua, raw: RawResponse) -> Result<(Option<Value>, Option<Value>), String> {
    let table = raw.into_lua_table(lua)?;
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
            do_http(
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
            Ok(raw) => make_success(&lua, raw).map_err(|e| mlua::Error::runtime(e)),
            Err(msg) => make_error(&lua, msg, proxy_for_error.as_deref())
                .map_err(|e| mlua::Error::runtime(e)),
        }
    })?;

    let http_get = lua.create_async_function(
        |lua, (url, headers): (String, Option<HashMap<String, String>>)| async move {
            let result = smol::unblock(move || {
                do_http("GET", &url, None, headers.as_ref(), None, None, None)
            })
            .await;

            match result {
                Ok(raw) => make_success(&lua, raw).map_err(|e| mlua::Error::runtime(e)),
                Err(msg) => make_error(&lua, msg, None).map_err(|e| mlua::Error::runtime(e)),
            }
        },
    )?;

    let http_post = lua.create_async_function(
        |lua, (url, body, headers): (String, String, Option<HashMap<String, String>>)| async move {
            let body_bytes = body.into_bytes();
            let result = smol::unblock(move || {
                do_http(
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
                Ok(raw) => make_success(&lua, raw).map_err(|e| mlua::Error::runtime(e)),
                Err(msg) => make_error(&lua, msg, None).map_err(|e| mlua::Error::runtime(e)),
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

                let start = Instant::now();
                let agent = build_agent(None, Duration::from_secs(DEFAULT_TIMEOUT_SECS))?;

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
                let headers = collect_headers(response.headers());
                let body = read_response_body(response.body_mut(), MAX_RESPONSE_BODY)?;
                let elapsed = start.elapsed().as_millis() as u64;
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
                raw.into_lua_table(&lua_for_unblock).map(|t| (Some(Value::Table(t)), None))
            })
            .await;

            match result {
                Ok(success) => Ok(success),
                Err(msg) => make_error(&lua, msg, None)
                    .map_err(|e| mlua::Error::runtime(e)),
            }
        },
    )?;

    lua.globals().set("http_request", http_request)?;
    lua.globals().set("http_get", http_get)?;
    lua.globals().set("http_post", http_post)?;
    lua.globals().set("http_multipart", http_multipart)?;

    Ok(())
}
