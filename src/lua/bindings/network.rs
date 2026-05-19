use mlua::{Lua, Result as LuaResult};

struct RawResponse {
    status: u16,
    headers: std::collections::HashMap<String, String>,
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

fn collect_headers(headers: &ureq::http::HeaderMap) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
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

pub fn register(lua: &Lua) -> LuaResult<()> {
    let http_get = lua.create_async_function(
        |lua, (url, headers): (String, Option<std::collections::HashMap<String, String>>)| async move {
            let result: LuaResult<mlua::Table> = smol::unblock({
                let lua_clone = lua.clone();
                move || {
                    let mut req = ureq::get(&url);
                    if let Some(h) = headers {
                        for (k, v) in h {
                            req = req.header(&k, &v);
                        }
                    }
                    let mut response = req
                        .call()
                        .map_err(|e| format!("http_get failed: {e}"))?;
                    let raw = RawResponse {
                        status: response.status().as_u16(),
                        headers: collect_headers(response.headers()),
                        body: response
                            .body_mut()
                            .read_to_string()
                            .map_err(|e| format!("http_get read failed: {e}"))?,
                    };
                    raw.into_lua_table(&lua_clone)
                }
            })
            .await
            .map_err(mlua::Error::runtime);
            result
        },
    )?;

    let http_post = lua.create_async_function(
        |lua,
         (url, body, headers): (
            String,
            String,
            Option<std::collections::HashMap<String, String>>,
        )| async move {
            let result: LuaResult<mlua::Table> = smol::unblock({
                let lua_clone = lua.clone();
                move || {
                    let mut req = ureq::post(&url);
                    let mut content_type_set = false;
                    if let Some(h) = headers {
                        for (k, v) in h {
                            if k.eq_ignore_ascii_case("content-type") {
                                content_type_set = true;
                            }
                            req = req.header(&k, &v);
                        }
                    }
                    if !content_type_set {
                        req = req.header("Content-Type", "application/x-www-form-urlencoded");
                    }
                    let mut response = req
                        .send(body.as_bytes())
                        .map_err(|e| format!("http_post failed: {e}"))?;
                    let raw = RawResponse {
                        status: response.status().as_u16(),
                        headers: collect_headers(response.headers()),
                        body: response
                            .body_mut()
                            .read_to_string()
                            .map_err(|e| format!("http_post read failed: {e}"))?,
                    };
                    raw.into_lua_table(&lua_clone)
                }
            })
            .await
            .map_err(mlua::Error::runtime);
            result
        },
    )?;

    lua.globals().set("http_get", http_get)?;
    lua.globals().set("http_post", http_post)?;

    Ok(())
}
