use crate::services::db::{Db, LUA_USER_TABLE};
use crate::services::notifier;
use mlua::{Lua, LuaSerdeExt, Result as LuaResult};
use redb::ReadableTable;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

pub fn register(lua: &Lua, db: Arc<Db>, job_name: &str) -> anyhow::Result<()> {
    let prefix = format!("{}:", job_name);
    register_store_set(lua, Arc::clone(&db), prefix.clone())?;
    register_store_get(lua, Arc::clone(&db), prefix.clone())?;
    register_store_delete(lua, Arc::clone(&db), prefix)?;
    register_global_store_set(lua, Arc::clone(&db))?;
    register_global_store_get(lua, Arc::clone(&db))?;
    register_global_store_incr(lua, Arc::clone(&db))?;
    register_global_store_delete(lua, Arc::clone(&db))?;
    Ok(())
}

pub fn register_http_and_fs(lua: &Lua, job_dir: Option<std::path::PathBuf>) -> LuaResult<()> {
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
                    .map_err(|e| mlua::Error::runtime(format!("http_get failed: {e}")))?;
                response
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| mlua::Error::runtime(format!("http_get read failed: {e}")))
            })
            .await;
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
                    .map_err(|e| mlua::Error::runtime(format!("http_post failed: {e}")))?;
                response
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| mlua::Error::runtime(format!("http_post read failed: {e}")))
            })
            .await;
            result
        },
    )?;

    lua.globals().set("http_get", http_get)?;
    lua.globals().set("http_post", http_post)?;
    lua.globals().set(
        "notify",
        lua.create_function(|_, (title, body, timeout): (String, String, Option<u32>)| {
            notifier::send_notification(&title, &body, timeout.unwrap_or(5000))
                .map_err(|e| mlua::Error::runtime(format!("notify failed: {e}")))?;
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "dump",
        lua.create_function(|_, value: mlua::Value| {
            let mut seen = HashSet::new();
            format_lua_value(&value, 0, &mut seen)
        })?,
    )?;

    if let Some(dir) = job_dir {
        let log_dir = dir.clone();
        let log_fn = lua.create_function(move |_, msg: String| {
            let path = log_dir.join("hook.log");
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

fn format_lua_value(
    value: &mlua::Value,
    indent: usize,
    seen: &mut HashSet<usize>,
) -> LuaResult<String> {
    match value {
        mlua::Value::Nil => Ok("nil".to_string()),
        mlua::Value::Boolean(b) => Ok(b.to_string()),
        mlua::Value::Integer(i) => Ok(i.to_string()),
        mlua::Value::Number(n) => Ok(n.to_string()),
        mlua::Value::String(s) => Ok(format!("{:?}", s.to_str()?)),
        mlua::Value::Table(table) => format_lua_table(table, indent, seen),
        mlua::Value::Function(_) => Ok("<function>".to_string()),
        mlua::Value::Thread(_) => Ok("<thread>".to_string()),
        mlua::Value::UserData(_) => Ok("<userdata>".to_string()),
        mlua::Value::LightUserData(_) => Ok("<lightuserdata>".to_string()),
        mlua::Value::Error(err) => Ok(format!("<error: {err}>")),
        #[cfg(feature = "luau")]
        mlua::Value::Vector(v) => Ok(format!("<vector: {v:?}>")),
        #[cfg(feature = "luau")]
        mlua::Value::Buffer(buf) => Ok(format!("<buffer: {} bytes>", buf.len())),
        mlua::Value::Other(_) => Ok("<other>".to_string()),
    }
}

fn format_lua_table(
    table: &mlua::Table,
    indent: usize,
    seen: &mut HashSet<usize>,
) -> LuaResult<String> {
    let table_id = table.to_pointer() as usize;
    if !seen.insert(table_id) {
        return Ok("<cycle>".to_string());
    }

    let mut entries = Vec::new();
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        let key_str = format_lua_key(&key)?;
        let value_str = format_lua_value(&value, indent + 1, seen)?;
        entries.push((key_str, value_str));
    }
    seen.remove(&table_id);

    if entries.is_empty() {
        return Ok("{}".to_string());
    }

    let mut out = String::from("{\n");
    let pad = "  ".repeat(indent + 1);
    for (key, value) in entries {
        for line in value.lines() {
            if line == value.lines().next().unwrap_or_default() {
                writeln!(&mut out, "{pad}{key} = {line},").expect("write to string");
            } else {
                writeln!(&mut out, "{pad}{line}").expect("write to string");
            }
        }
    }
    write!(&mut out, "{}{}", "  ".repeat(indent), "}").expect("write to string");
    Ok(out)
}

fn format_lua_key(key: &mlua::Value) -> LuaResult<String> {
    match key {
        mlua::Value::String(s) => Ok(s.to_str()?.to_string()),
        mlua::Value::Integer(i) => Ok(format!("[{i}]")),
        mlua::Value::Number(n) => Ok(format!("[{n}]")),
        mlua::Value::Boolean(b) => Ok(format!("[{b}]")),
        other => Ok(format!(
            "[{}]",
            format_lua_value(other, 0, &mut HashSet::new())?
        )),
    }
}

fn log_storage_error(op: &str, err: impl std::fmt::Display) {
    crate::t_eprintln!("Lua storage {} failed: {}", op, err);
}

fn register_store_set(lua: &Lua, db: Arc<Db>, prefix: String) -> anyhow::Result<()> {
    lua.globals().set(
        "store_set",
        lua.create_function(move |_, (key, value): (String, String)| {
            let prefixed = format!("{}{}", prefix, key);
            let write = match db.begin_write() {
                Ok(write) => write,
                Err(err) => {
                    log_storage_error("store_set", err);
                    return Ok(());
                }
            };
            {
                let mut table = match write.open_table(LUA_USER_TABLE) {
                    Ok(table) => table,
                    Err(err) => {
                        log_storage_error("store_set", err);
                        return Ok(());
                    }
                };
                if let Err(err) = table.insert(prefixed.as_str(), value.as_str()) {
                    log_storage_error("store_set", err);
                    return Ok(());
                }
            }
            if let Err(err) = write.commit() {
                log_storage_error("store_set", err);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

fn register_store_get(lua: &Lua, db: Arc<Db>, prefix: String) -> anyhow::Result<()> {
    lua.globals().set(
        "store_get",
        lua.create_function(move |_, key: String| {
            let prefixed = format!("{}{}", prefix, key);
            let read = match db.begin_read() {
                Ok(read) => read,
                Err(err) => {
                    log_storage_error("store_get", err);
                    return Ok(None);
                }
            };
            let table = match read.open_table(LUA_USER_TABLE) {
                Ok(table) => table,
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
                Err(err) => {
                    log_storage_error("store_get", err);
                    return Ok(None);
                }
            };
            match table.get(prefixed.as_str()) {
                Ok(value) => Ok(value.map(|v: redb::AccessGuard<&str>| v.value().to_string())),
                Err(err) => {
                    log_storage_error("store_get", err);
                    Ok(None)
                }
            }
        })?,
    )?;
    Ok(())
}

fn register_store_delete(lua: &Lua, db: Arc<Db>, prefix: String) -> anyhow::Result<()> {
    lua.globals().set(
        "store_delete",
        lua.create_function(move |_, key: String| {
            let prefixed = format!("{}{}", prefix, key);
            let write = match db.begin_write() {
                Ok(write) => write,
                Err(err) => {
                    log_storage_error("store_delete", err);
                    return Ok(());
                }
            };
            {
                let mut table = match write.open_table(LUA_USER_TABLE) {
                    Ok(table) => table,
                    Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
                    Err(err) => {
                        log_storage_error("store_delete", err);
                        return Ok(());
                    }
                };
                if let Err(err) = table.remove(prefixed.as_str()) {
                    log_storage_error("store_delete", err);
                    return Ok(());
                }
            }
            if let Err(err) = write.commit() {
                log_storage_error("store_delete", err);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

fn register_global_store_set(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_set",
        lua.create_function(move |_, (key, value): (String, String)| {
            let write = match db.begin_write() {
                Ok(write) => write,
                Err(err) => {
                    log_storage_error("global_store_set", err);
                    return Ok(());
                }
            };
            {
                let mut table = match write.open_table(LUA_USER_TABLE) {
                    Ok(table) => table,
                    Err(err) => {
                        log_storage_error("global_store_set", err);
                        return Ok(());
                    }
                };
                if let Err(err) = table.insert(key.as_str(), value.as_str()) {
                    log_storage_error("global_store_set", err);
                    return Ok(());
                }
            }
            if let Err(err) = write.commit() {
                log_storage_error("global_store_set", err);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

fn register_global_store_get(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_get",
        lua.create_function(move |_, key: String| {
            let read = match db.begin_read() {
                Ok(read) => read,
                Err(err) => {
                    log_storage_error("global_store_get", err);
                    return Ok(None);
                }
            };
            let table = match read.open_table(LUA_USER_TABLE) {
                Ok(table) => table,
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
                Err(err) => {
                    log_storage_error("global_store_get", err);
                    return Ok(None);
                }
            };
            match table.get(key.as_str()) {
                Ok(value) => Ok(value.map(|v: redb::AccessGuard<&str>| v.value().to_string())),
                Err(err) => {
                    log_storage_error("global_store_get", err);
                    Ok(None)
                }
            }
        })?,
    )?;
    Ok(())
}

fn register_global_store_incr(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_incr",
        lua.create_function(move |_, (key, default, delta): (String, i64, i64)| {
            let write = match db.begin_write() {
                Ok(write) => write,
                Err(err) => {
                    log_storage_error("global_store_incr", err);
                    return Ok(default);
                }
            };

            let new_val_num = {
                let mut table = match write.open_table(LUA_USER_TABLE) {
                    Ok(table) => table,
                    Err(err) => {
                        log_storage_error("global_store_incr", err);
                        return Ok(default);
                    }
                };

                let current = match table.get(key.as_str()) {
                    Ok(value) => value
                        .and_then(|v: redb::AccessGuard<&str>| v.value().parse::<i64>().ok())
                        .unwrap_or(default),
                    Err(err) => {
                        log_storage_error("global_store_incr", err);
                        return Ok(default);
                    }
                };

                let new_val_num = current + delta;
                let new_val = new_val_num.to_string();

                if let Err(err) = table.insert(key.as_str(), new_val.as_str()) {
                    log_storage_error("global_store_incr", err);
                    return Ok(default);
                }

                new_val_num
            };

            if let Err(err) = write.commit() {
                log_storage_error("global_store_incr", err);
                return Ok(default);
            }

            Ok(new_val_num)
        })?,
    )?;
    Ok(())
}

fn register_global_store_delete(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_delete",
        lua.create_function(move |_, key: String| {
            let write = match db.begin_write() {
                Ok(write) => write,
                Err(err) => {
                    log_storage_error("global_store_delete", err);
                    return Ok(());
                }
            };
            {
                let mut table = match write.open_table(LUA_USER_TABLE) {
                    Ok(table) => table,
                    Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
                    Err(err) => {
                        log_storage_error("global_store_delete", err);
                        return Ok(());
                    }
                };
                if let Err(err) = table.remove(key.as_str()) {
                    log_storage_error("global_store_delete", err);
                    return Ok(());
                }
            }
            if let Err(err) = write.commit() {
                log_storage_error("global_store_delete", err);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;
    use std::io::Read;
    use std::{fs, sync::Arc};

    struct TestDb {
        path: String,
        db: Option<Arc<Db>>,
    }

    impl TestDb {
        fn new(name: &str) -> Self {
            let path = format!("{}.redb", name);
            if fs::metadata(&path).is_ok() {
                fs::remove_file(&path).unwrap();
            }
            let db = Arc::new(Db::open(&path).unwrap());
            Self { path, db: Some(db) }
        }

        fn db(&self) -> Arc<Db> {
            Arc::clone(self.db.as_ref().unwrap())
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            self.db = None;
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn test_http_bindings() {
        let server = rouille::Server::new("127.0.0.1:0", |request| {
            if request.method() == "GET" {
                if request.header("X-Custom-Auth") == Some("secret-123") {
                    rouille::Response::text("GET-OK-AUTH!")
                } else {
                    rouille::Response::text("GET-OK!")
                }
            } else if request.method() == "POST" {
                let mut body = String::new();
                if let Some(mut reader) = request.data() {
                    let _ = reader.read_to_string(&mut body);
                }
                assert_eq!(body, "my-body");
                if request.header("Content-Type") == Some("application/json") {
                    rouille::Response::text("POST-OK-JSON!")
                } else {
                    rouille::Response::text("POST-OK!")
                }
            } else {
                rouille::Response::empty_404()
            }
        })
        .unwrap();
        let port = server.server_addr().port();
        std::thread::spawn(move || server.run());

        smol::block_on(async {
            let lua = Lua::new();
            register_http_and_fs(&lua, None).unwrap();

            let code_get = format!(r#"http_get("http://127.0.0.1:{}/")"#, port);
            let body: String = lua.load(&code_get).eval_async().await.unwrap();
            assert_eq!(body, "GET-OK!");

            let code_get_headers = format!(
                r#"http_get("http://127.0.0.1:{}/", {{ ["X-Custom-Auth"] = "secret-123" }})"#,
                port
            );
            let body: String = lua.load(&code_get_headers).eval_async().await.unwrap();
            assert_eq!(body, "GET-OK-AUTH!");

            let code_post = format!(r#"http_post("http://127.0.0.1:{}/", "my-body")"#, port);
            let body: String = lua.load(&code_post).eval_async().await.unwrap();
            assert_eq!(body, "POST-OK!");

            let code_post_headers = format!(
                r#"http_post("http://127.0.0.1:{}/", "my-body", {{ ["Content-Type"] = "application/json" }})"#,
                port
            );
            let body: String = lua.load(&code_post_headers).eval_async().await.unwrap();
            assert_eq!(body, "POST-OK-JSON!");
        });
    }

    #[test]
    fn test_lua_storage_bindings_are_exposed_and_work() {
        let tdb = TestDb::new("test_lua_storage_bindings");

        let lua_job_one = Lua::new();
        register(&lua_job_one, tdb.db(), "job_one").unwrap();

        let lua_job_two = Lua::new();
        register(&lua_job_two, tdb.db(), "job_two").unwrap();

        for name in [
            "store_set",
            "store_get",
            "store_delete",
            "global_store_set",
            "global_store_get",
            "global_store_delete",
            "global_store_incr",
        ] {
            let func = lua_job_one
                .globals()
                .get::<Option<mlua::Function>>(name)
                .unwrap();
            assert!(
                func.is_some(),
                "expected Lua global {name} to be registered"
            );
        }

        let missing_job_value: Option<String> = lua_job_one
            .load(r#"return store_get("page")"#)
            .eval()
            .unwrap();
        assert_eq!(missing_job_value, None);

        let missing_global_value: Option<String> = lua_job_one
            .load(r#"return global_store_get("shared_counter")"#)
            .eval()
            .unwrap();
        assert_eq!(missing_global_value, None);

        lua_job_one
            .load(
                r#"
                store_set("page", "3")
                global_store_set("shared_counter", "10")
            "#,
            )
            .exec()
            .unwrap();

        let job_one_page: Option<String> = lua_job_one
            .load(r#"return store_get("page")"#)
            .eval()
            .unwrap();
        assert_eq!(job_one_page.as_deref(), Some("3"));

        let job_two_page: Option<String> = lua_job_two
            .load(r#"return store_get("page")"#)
            .eval()
            .unwrap();
        assert_eq!(job_two_page, None);

        let shared_from_job_two: Option<String> = lua_job_two
            .load(r#"return global_store_get("shared_counter")"#)
            .eval()
            .unwrap();
        assert_eq!(shared_from_job_two.as_deref(), Some("10"));

        lua_job_one.load(r#"store_delete("page")"#).exec().unwrap();
        lua_job_two
            .load(r#"global_store_delete("shared_counter")"#)
            .exec()
            .unwrap();

        let deleted_job_value: Option<String> = lua_job_one
            .load(r#"return store_get("page")"#)
            .eval()
            .unwrap();
        assert_eq!(deleted_job_value, None);

        let deleted_global_value: Option<String> = lua_job_one
            .load(r#"return global_store_get("shared_counter")"#)
            .eval()
            .unwrap();
        assert_eq!(deleted_global_value, None);
    }

    #[test]
    fn test_global_store_incr() {
        let tdb = TestDb::new("test_global_store_incr");

        let lua = Lua::new();
        register(&lua, tdb.db(), "test_job").unwrap();

        let initial: i64 = lua
            .load(r#"return global_store_incr("incr_key", 0, 1)"#)
            .eval()
            .unwrap();
        assert_eq!(initial, 1);

        let incremented: i64 = lua
            .load(r#"return global_store_incr("incr_key", 0, 5)"#)
            .eval()
            .unwrap();
        assert_eq!(incremented, 6);

        let decremented: i64 = lua
            .load(r#"return global_store_incr("incr_key", 0, -3)"#)
            .eval()
            .unwrap();
        assert_eq!(decremented, 3);

        let uses_default: i64 = lua
            .load(r#"return global_store_incr("nonexistent_key", 100, 10)"#)
            .eval()
            .unwrap();
        assert_eq!(uses_default, 110);
    }

    #[test]
    fn test_env_get_binding() {
        let lua = Lua::new();
        register_http_and_fs(&lua, None).unwrap();

        unsafe {
            std::env::set_var("SPYWEB_TEST_SECRET", "12345");
            std::env::set_var("OTHER_SECRET", "hacker");
        }

        // Should work with explicit prefix
        let val1: String = lua
            .load(r#"return env_get("SPYWEB_TEST_SECRET")"#)
            .eval()
            .unwrap();
        assert_eq!(val1, "12345");

        // Should work with auto-prefixing
        let val2: String = lua.load(r#"return env_get("TEST_SECRET")"#).eval().unwrap();
        assert_eq!(val2, "12345");

        // Should NOT find variables without the prefix
        let val3: Option<String> = lua
            .load(r#"return env_get("OTHER_SECRET")"#)
            .eval()
            .unwrap();
        assert!(
            val3.is_none(),
            "Should not access variables without SPYWEB_ prefix"
        );
    }

    #[test]
    fn test_dump_binding_formats_nested_tables_and_cycles() {
        let lua = Lua::new();
        register_http_and_fs(&lua, None).unwrap();

        let dumped: String = lua
            .load(
                r#"
                local t = {
                    status = 200,
                    nested = {
                        ok = true,
                        message = "hello"
                    }
                }
                t.self = t
                return dump(t)
            "#,
            )
            .eval()
            .unwrap();

        assert!(dumped.contains("status = 200"));
        assert!(dumped.contains("nested = {"));
        assert!(dumped.contains("ok = true"));
        assert!(dumped.contains(r#"message = "hello""#));
        assert!(dumped.contains("self = <cycle>"));
    }
}
