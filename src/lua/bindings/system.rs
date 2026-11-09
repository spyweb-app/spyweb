use crate::lua::conversions::{lua_to_json, register_json_empty_array};
use crate::platform::PlatformInfo;
use crate::services::notifier;
use mlua::{Lua, LuaSerdeExt, Result as LuaResult};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) const SHARED_DIR: &str = "shared";

fn resolve_path_read(path_str: &str, job_dir: &Path) -> PathBuf {
    if path_str.starts_with(&format!("{}/", SHARED_DIR)) {
        return Path::new(path_str).to_path_buf();
    }
    let job_path = job_dir.join(path_str);
    if job_path.exists() {
        return job_path;
    }
    Path::new(SHARED_DIR).join(path_str)
}

fn resolve_path_write(path_str: &str, job_dir: &Path) -> PathBuf {
    if path_str.starts_with(&format!("{}/", SHARED_DIR)) {
        return Path::new(path_str).to_path_buf();
    }
    job_dir.join(path_str)
}

pub(crate) fn log_file_name(job_name: &str) -> &'static str {
    match job_name {
        "__server" => "server.log",
        "__startup" => "startup.log",
        _ => "hooks.log",
    }
}

const RANDOM_MAX_BYTES: i64 = 65536;

fn random_buf(name: &str, n: i64) -> LuaResult<Vec<u8>> {
    if !(1..=RANDOM_MAX_BYTES).contains(&n) {
        return Err(mlua::Error::runtime(format!(
            "random.{name}: n must be in 1..={RANDOM_MAX_BYTES}, got {n}"
        )));
    }
    let mut buf = vec![0u8; n as usize];
    getrandom::fill(&mut buf).map_err(|e| mlua::Error::runtime(format!("random.{name}: {e}")))?;
    Ok(buf)
}

pub fn register(lua: &Lua, job_dir: Option<PathBuf>, job_name: &str) -> LuaResult<()> {
    register_common(lua, job_dir, job_name)?;
    register_defer(lua)?;
    Ok(())
}

pub fn register_common(lua: &Lua, job_dir: Option<PathBuf>, job_name: &str) -> LuaResult<()> {
    lua.load("math.randomseed(os.time())").exec()?;

    // helpers.lua
    lua.load(include_str!("../globals/helpers.lua"))
        .set_name("helpers.lua")
        .exec()
        .map_err(|e| mlua::Error::runtime(format!("Failed to load helpers.lua: {e}")))?;

    // Engine info table
    let engine = lua.create_table()?;
    engine.set("os", PlatformInfo::os())?;
    engine.set("arch", PlatformInfo::arch())?;
    engine.set("platform", PlatformInfo::platform())?;
    engine.set("headless", PlatformInfo::is_headless())?;
    engine.set("version", PlatformInfo::version())?;
    engine.set("lua_version", PlatformInfo::lua_version())?;
    engine.set("storage", PlatformInfo::storage())?;
    lua.globals().set("engine", engine)?;

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
        let log_name = log_file_name(job_name);
        lua.globals().set(
            "log",
            lua.create_async_function(move |_, msg: String| {
                let path = log_dir.join(log_name);
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
                    let path = resolve_path_write(&path_str, &fs_dir);
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
                    let path = resolve_path_write(&path_str, &fs_overwrite_dir);
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
                    let path = resolve_path_read(&path_str, &read_dir);
                    crate::services::io::validate_path(&path)
                        .map_err(|e| mlua::Error::runtime(format!("fs_read rejected: {e}")))?;
                    crate::services::io::check_extension(
                        &path,
                        crate::services::io::ALLOWED_READ_EXT,
                        "fs_read",
                    )
                    .map_err(mlua::Error::runtime)?;
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
                    let path = resolve_path_read(&path_str, &read_binary_dir);
                    crate::services::io::validate_path(&path).map_err(|e| {
                        mlua::Error::runtime(format!("fs_read_binary rejected: {e}"))
                    })?;
                    crate::services::io::check_extension(
                        &path,
                        crate::services::io::ALLOWED_BINARY_EXT,
                        "fs_read_binary",
                    )
                    .map_err(mlua::Error::runtime)?;
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
                let candidates = [
                    require_dir.join(format!("{}.lua", rel_path)),
                    require_dir.join(format!("{}/init.lua", rel_path)),
                    std::path::Path::new(".").join(format!("{}.lua", rel_path)),
                    std::path::Path::new(".").join(format!("{}/init.lua", rel_path)),
                ];

                let mut last_err = String::new();
                let mut source = None;
                for path in &candidates {
                    if !path.exists() {
                        continue;
                    }

                    crate::services::io::validate_path(path)
                        .map_err(|e| mlua::Error::runtime(format!("require rejected: {e}")))?;

                    match std::fs::read_to_string(path) {
                        Ok(s) => {
                            source = Some(s);
                            break;
                        }
                        Err(e) => {
                            last_err = format!("failed to read module '{}': {}", name, e);
                        }
                    }
                }

                let source = match source {
                    Some(s) => s,
                    None => {
                        return Err(mlua::Error::runtime(format!(
                            "module '{}' not found in job folder or project root ({})",
                            name, last_err
                        )));
                    }
                };

                let result: mlua::Value = lua.load(&source).set_name(&name).call(())?;
                loaded.set(name, result.clone())?;
                Ok(result)
            })?;
            lua.globals().set("require", require_fn)?;
        }
    }

    register_json_empty_array(lua).map_err(mlua::Error::external)?;
    lua.globals().set(
        "json_encode",
        lua.create_function(|lua, (val, array): (mlua::Value, Option<bool>)| {
            let json = lua_to_json(lua, &val, array).map_err(mlua::Error::external)?;
            serde_json::to_string(&json).map_err(mlua::Error::external)
        })?,
    )?;

    lua.globals().set(
        "json_decode",
        lua.create_function(|lua, s: String| {
            if s.len() > 10_000_000 {
                return Ok((
                    mlua::Value::Nil,
                    mlua::Value::String(lua.create_string("json_decode input exceeds 10MB limit")?),
                ));
            }
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(val) => match lua.to_value(&val) {
                    Ok(lua_val) => Ok((lua_val, mlua::Value::Nil)),
                    Err(e) => Ok((
                        mlua::Value::Nil,
                        mlua::Value::String(lua.create_string(format!("json_decode: {e}"))?),
                    )),
                },
                Err(e) => Ok((
                    mlua::Value::Nil,
                    mlua::Value::String(lua.create_string(format!("json_decode: {e}"))?),
                )),
            }
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

    let random = lua.create_table()?;

    random.set(
        "bytes",
        lua.create_function(|lua, n: i64| lua.create_string(&random_buf("bytes", n)?))?,
    )?;

    random.set(
        "hex",
        lua.create_function(|lua, n: i64| {
            let buf = random_buf("hex", n)?;
            let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
            lua.create_string(&hex)
        })?,
    )?;

    random.set(
        "int",
        lua.create_function(|_, (min, max): (i64, i64)| {
            if min > max {
                return Err(mlua::Error::runtime(format!(
                    "random.int: min ({min}) must be <= max ({max})"
                )));
            }

            let range = max as i128 - min as i128 + 1; // 1..=2^64
            let threshold = (1i128 << 64) - ((1i128 << 64) % range);
            loop {
                let mut buf = [0u8; 8];
                getrandom::fill(&mut buf)
                    .map_err(|e| mlua::Error::runtime(format!("random.int: {e}")))?;
                let x = u64::from_le_bytes(buf) as i128;
                if x < threshold {
                    return Ok((min as i128 + (x % range)) as i64);
                }
            }
        })?,
    )?;

    lua.globals().set("random", random)?;

    Ok(())
}

pub fn register_defer(lua: &Lua) -> LuaResult<()> {
    lua.globals().set(
        "defer",
        lua.create_function(|lua, f: mlua::Function| {
            let ctx: mlua::Table = lua.named_registry_value("active_ctx")?;
            let deferred: mlua::Table = ctx.get("__deferred")?;
            if deferred.raw_len() >= 10000 {
                return Err(mlua::Error::external("defer queue full (max 10000 items)"));
            }
            deferred.push(f)?;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::log_file_name;

    #[test]
    fn test_log_file_names() {
        assert_eq!(log_file_name("__startup"), "startup.log");
        assert_eq!(log_file_name("__server"), "server.log");
        assert_eq!(log_file_name("demo_job"), "hooks.log");
    }
}
