use crate::services::notifier;
use mlua::{Lua, LuaSerdeExt, Result as LuaResult};
use std::path::PathBuf;

pub fn register(lua: &Lua, job_dir: Option<PathBuf>) -> LuaResult<()> {
    // helpers.lua
    lua.load(include_str!("../globals/helpers.lua"))
        .set_name("helpers.lua")
        .exec()
        .map_err(|e| mlua::Error::runtime(format!("Failed to load helpers.lua: {e}")))?;

    lua.globals().set(
        "notify",
        lua.create_async_function(|_, (title, body, timeout): (String, String, Option<u32>)| async move {
            smol::unblock(move || {
                notifier::send_notification(&title, &body, timeout.unwrap_or(5000))
                    .map_err(|e| mlua::Error::runtime(format!("notify failed: {e}")))
            })
            .await
        })?,
    )?;

    if let Some(dir) = job_dir {
        let log_dir = dir.clone();
        let log_fn = lua.create_async_function(move |_, msg: String| {
            let path = log_dir.join("hook.log");
            async move {
                smol::unblock(move || {
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|e| mlua::Error::runtime(format!("Failed to open log file: {e}")))?;

                    use std::io::Write;
                    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                    writeln!(f, "[{}]: {}", timestamp, msg)
                        .map_err(|e| mlua::Error::runtime(format!("Failed to write to log file: {e}")))?;
                    Ok(())
                })
                .await
            }
        })?;
        lua.globals().set("log", log_fn)?;

        #[cfg(feature = "luau")]
        {
            let require_dir = dir.clone();
            let require_fn = lua.create_function(move |lua, name: String| {
                let loaded: mlua::Table =
                    if let Ok(t) = lua.globals().get::<mlua::Table>("__loaded") {
                        t
                    } else {
                        let t = lua.create_table()?;
                        lua.globals().set("__loaded", t.clone())?;
                        t
                    };

                if let Ok(val) = loaded.get::<mlua::Value>(name.clone())
                    && !matches!(val, mlua::Value::Nil)
                {
                    return Ok(val);
                }

                let rel_path = name.replace('.', "/");
                let local_path = require_dir.join(format!("{}.lua", rel_path));
                let shared_path = std::path::Path::new("shared").join(format!("{}.lua", rel_path));

                let source = if local_path.exists() {
                    std::fs::read_to_string(&local_path)
                } else if shared_path.exists() {
                    std::fs::read_to_string(&shared_path)
                } else {
                    return Err(mlua::Error::runtime(format!(
                        "module '{}' not found in job folder ({:?}) or shared folder ({:?})",
                        name, local_path, shared_path
                    )));
                }
                .map_err(|e| {
                    mlua::Error::runtime(format!("failed to read module '{}': {}", name, e))
                })?;

                let result: mlua::Value = lua.load(&source).set_name(&name).call(())?;
                loaded.set(name, result.clone())?;
                Ok(result)
            })?;
            lua.globals().set("require", require_fn)?;
        }
    }

    lua.globals().set(
        "json_encode",
        lua.create_function(|_, val: mlua::Value| {
            let json = serde_json::to_string(&val).map_err(mlua::Error::external)?;
            Ok(json)
        })?,
    )?;

    lua.globals().set(
        "json_decode",
        lua.create_function(|lua, s: String| {
            let val: serde_json::Value = serde_json::from_str(&s).map_err(mlua::Error::external)?;
            let lua_val = lua.to_value(&val).map_err(mlua::Error::external)?;
            Ok(lua_val)
        })?,
    )?;

    lua.globals().set(
        "env_get",
        lua.create_function(|_, key: String| {
            let prefixed = if key.starts_with("SPYWEB_") {
                key
            } else {
                format!("SPYWEB_{}", key)
            };
            Ok(std::env::var(prefixed).ok())
        })?,
    )?;

    Ok(())
}
