use crate::services::notifier;
use mlua::{Lua, LuaSerdeExt, Result as LuaResult};
use std::path::PathBuf;
use std::time::Duration;

pub fn register(lua: &Lua, job_dir: Option<PathBuf>) -> LuaResult<()> {
    lua.load("math.randomseed(os.time())").exec()?;

    // helpers.lua
    lua.load(include_str!("../globals/helpers.lua"))
        .set_name("helpers.lua")
        .exec()
        .map_err(|e| mlua::Error::runtime(format!("Failed to load helpers.lua: {e}")))?;

    lua.globals().set(
        "sleep",
        lua.create_async_function(|_, ms: u64| async move {
            smol::Timer::after(Duration::from_millis(ms)).await;
            Ok(())
        })?,
    )?;

    lua.globals().set(
        "notify",
        lua.create_async_function(
            |_, (title, body, timeout): (String, String, Option<u32>)| async move {
                smol::unblock(move || {
                    notifier::send_notification(&title, &body, timeout.unwrap_or(5000))
                        .map_err(|e| mlua::Error::runtime(format!("notify failed: {e}")))
                })
                .await
            },
        )?,
    )?;

    if let Some(dir) = job_dir {
        let log_dir = dir.clone();
        lua.globals().set(
            "log",
            lua.create_async_function(move |_, msg: String| {
                let path = log_dir.join("hooks.log");
                async move {
                    let task = crate::services::io::IoTask {
                        path,
                        content: msg.into_bytes(),
                        op: crate::services::io::IoOp::Append,
                        add_timestamp: true,
                        reply: None,
                    };
                    crate::services::io::send_task(task)
                        .await
                        .map_err(|e| mlua::Error::runtime(format!("log failed: {e}")))
                }
            })?,
        )?;

        let fs_dir = dir.clone();
        lua.globals().set(
            "fs_append",
            lua.create_async_function(move |_, (path_str, content): (String, String)| {
                let fs_dir = fs_dir.clone();
                async move {
                    if std::path::Path::new(&path_str).is_absolute() {
                        return Err(mlua::Error::runtime("Absolute paths are not allowed"));
                    }
                    let path = fs_dir.join(path_str);
                    let task = crate::services::io::IoTask {
                        path,
                        content: content.into_bytes(),
                        op: crate::services::io::IoOp::Append,
                        add_timestamp: false,
                        reply: None,
                    };
                    crate::services::io::send_task(task)
                        .await
                        .map_err(|e| mlua::Error::runtime(format!("fs_append failed: {e}")))
                }
            })?,
        )?;

        let fs_overwrite_dir = dir.clone();
        lua.globals().set(
            "fs_overwrite",
            lua.create_async_function(move |_, (path_str, content): (String, String)| {
                let fs_overwrite_dir = fs_overwrite_dir.clone();
                async move {
                    if std::path::Path::new(&path_str).is_absolute() {
                        return Err(mlua::Error::runtime("Absolute paths are not allowed"));
                    }
                    let path = fs_overwrite_dir.join(path_str);
                    let task = crate::services::io::IoTask {
                        path,
                        content: content.into_bytes(),
                        op: crate::services::io::IoOp::Overwrite,
                        add_timestamp: false,
                        reply: None,
                    };
                    crate::services::io::send_task(task)
                        .await
                        .map_err(|e| mlua::Error::runtime(format!("fs_overwrite failed: {e}")))
                }
            })?,
        )?;

        let read_dir = dir.clone();
        lua.globals().set(
            "fs_read",
            lua.create_async_function(move |_, path_str: String| {
                let read_dir = read_dir.clone();
                async move {
                    if std::path::Path::new(&path_str).is_absolute() {
                        return Err(mlua::Error::runtime("Absolute paths are not allowed"));
                    }
                    let path = read_dir.join(&path_str);
                    crate::services::io::validate_path(&path)
                        .map_err(|e| mlua::Error::runtime(format!("fs_read rejected: {e}")))?;
                    let result = smol::unblock(move || std::fs::read_to_string(&path)).await;
                    match result {
                        Ok(content) => Ok(Some(content)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(e) => Err(mlua::Error::runtime(format!("fs_read failed: {e}"))),
                    }
                }
            })?,
        )?;

        let read_binary_dir = dir.clone();
        lua.globals().set(
            "fs_read_binary",
            lua.create_async_function(move |_, path_str: String| {
                let read_binary_dir = read_binary_dir.clone();
                async move {
                    if std::path::Path::new(&path_str).is_absolute() {
                        return Err(mlua::Error::runtime("Absolute paths are not allowed"));
                    }
                    let path = read_binary_dir.join(&path_str);
                    crate::services::io::validate_path(&path).map_err(|e| {
                        mlua::Error::runtime(format!("fs_read_binary rejected: {e}"))
                    })?;
                    let result = smol::unblock(move || std::fs::read(&path)).await;
                    match result {
                        Ok(content) => Ok(Some(content)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(e) => Err(mlua::Error::runtime(format!("fs_read_binary failed: {e}"))),
                    }
                }
            })?,
        )?;

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

    lua.globals().set(
        "defer",
        lua.create_function(|lua, f: mlua::Function| {
            let ctx: mlua::Table = lua.named_registry_value("active_ctx")?;
            let deferred: mlua::Table = ctx.get("__deferred")?;
            deferred.push(f)?;
            Ok(())
        })?,
    )?;

    Ok(())
}
