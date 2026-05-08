use mlua::{Lua, Result as LuaResult};

pub fn register(lua: &Lua) -> LuaResult<()> {
    let http_get = lua.create_async_function(
        |_lua, (url, headers): (String, Option<std::collections::HashMap<String, String>>)| async move {
            let result: LuaResult<String> = smol::unblock(move || {
                let mut req = ureq::get(&url);
                if let Some(h) = headers {
                    for (k, v) in h {
                        req = req.header(&k, &v);
                    }
                }
                let mut response = req
                    .call()
                    .map_err(|e|format!("http_get failed: {e}"))?;
                response
                    .body_mut()
                    .read_to_string()
                    .map_err(|e|format!("http_get read failed: {e}"))
            })
            .await
            .map_err(mlua::Error::runtime);
            result
        },
    )?;

    let http_post = lua.create_async_function(
        |_lua,
         (url, body, headers): (
            String,
            String,
            Option<std::collections::HashMap<String, String>>,
        )| async move {
            let result: LuaResult<String> = smol::unblock(move || {
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
                response
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| format!("http_post read failed: {e}"))
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
