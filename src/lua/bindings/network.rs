use mlua::{Lua, Result as LuaResult};
use std::collections::HashMap;
use std::io::Read;

const MAX_RESPONSE_BODY: u64 = 10 * 1024 * 1024;

struct RawResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
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
            .set("status", self.status)
            .map_err(|e| format!("lua error: {e}"))?;
        table
            .set("headers", headers_table)
            .map_err(|e| format!("lua error: {e}"))?;
        table
            .set("body", self.body)
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

fn read_response_body(body: &mut ureq::Body) -> Result<String, String> {
    let mut buf = Vec::new();
    body.as_reader()
        .take(MAX_RESPONSE_BODY + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read response body failed: {e}"))?;
    if buf.len() as u64 > MAX_RESPONSE_BODY {
        return Err("response body exceeds 10MB limit".to_string());
    }
    String::from_utf8(buf).map_err(|e| format!("response body is not valid UTF-8: {e}"))
}

fn do_http(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    headers: Option<&HashMap<String, String>>,
) -> Result<RawResponse, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();

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
        let body = read_response_body(response.body_mut())?;
        Ok(RawResponse {
            status,
            headers,
            body,
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
        let body = read_response_body(response.body_mut())?;
        Ok(RawResponse {
            status,
            headers,
            body,
        })
    }
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
        let body_bytes = body.as_ref().map(|s| {
            let bytes: Vec<u8> = s.as_bytes().to_vec();
            bytes
        });
        let headers: Option<HashMap<String, String>> = args.get("headers").ok().flatten();

        let result: LuaResult<mlua::Table> = smol::unblock(move || {
            let raw = do_http(&method, &url, body_bytes.as_deref(), headers.as_ref())?;
            raw.into_lua_table(&lua)
        })
        .await
        .map_err(mlua::Error::runtime);
        result
    })?;

    let http_get = lua.create_async_function(
        |lua, (url, headers): (String, Option<HashMap<String, String>>)| async move {
            let result: LuaResult<mlua::Table> = smol::unblock(move || {
                let raw = do_http("GET", &url, None, headers.as_ref())?;
                raw.into_lua_table(&lua)
            })
            .await
            .map_err(mlua::Error::runtime);
            result
        },
    )?;

    let http_post = lua.create_async_function(
        |lua, (url, body, headers): (String, String, Option<HashMap<String, String>>)| async move {
            let body_bytes = body.into_bytes();
            let result: LuaResult<mlua::Table> = smol::unblock(move || {
                let raw = do_http("POST", &url, Some(&body_bytes), headers.as_ref())?;
                raw.into_lua_table(&lua)
            })
            .await
            .map_err(mlua::Error::runtime);
            result
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

            let result: LuaResult<mlua::Table> = smol::unblock(move || {
                use ureq::unversioned::multipart::{Form, Part};

                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_global(Some(std::time::Duration::from_secs(30)))
                    .http_status_as_error(false)
                    .build()
                    .into();

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
                if let Some(h) = headers {
                    for (k, v) in h {
                        req = req.header(k, v);
                    }
                }

                let mut response = req
                    .send(form)
                    .map_err(|e| format!("http_multipart failed: {e}"))?;
                let status = response.status().as_u16();
                let headers = collect_headers(response.headers());
                let body = read_response_body(response.body_mut())?;
                let raw = RawResponse { status, headers, body };
                raw.into_lua_table(&lua)
            })
            .await
            .map_err(mlua::Error::runtime);
            result
        },
    )?;

    lua.globals().set("http_request", http_request)?;
    lua.globals().set("http_get", http_get)?;
    lua.globals().set("http_post", http_post)?;
    lua.globals().set("http_multipart", http_multipart)?;

    Ok(())
}
