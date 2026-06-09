use crate::services::db::Db;
use mlua::Lua;
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

fn log_storage_error(op: &str, err: impl std::fmt::Display) {
    crate::t_eprintln!("Lua storage {} failed: {}", op, err);
}

fn register_store_set(lua: &Lua, db: Arc<Db>, prefix: String) -> anyhow::Result<()> {
    lua.globals().set(
        "store_set",
        lua.create_async_function(move |_, (key, value): (String, String)| {
            let db = Arc::clone(&db);
            let prefix = prefix.clone();
            async move {
                smol::unblock(move || {
                    let prefixed = format!("{}{}", prefix, key);
                    if let Err(err) = db.lua_set(&prefixed, &value) {
                        log_storage_error("store_set", err);
                    }
                })
                .await;
                Ok(())
            }
        })?,
    )?;
    Ok(())
}

fn register_store_get(lua: &Lua, db: Arc<Db>, prefix: String) -> anyhow::Result<()> {
    lua.globals().set(
        "store_get",
        lua.create_async_function(move |_, key: String| {
            let db = Arc::clone(&db);
            let prefix = prefix.clone();
            async move {
                let result = smol::unblock(move || {
                    let prefixed = format!("{}{}", prefix, key);
                    match db.lua_get(&prefixed) {
                        Ok(value) => value,
                        Err(err) => {
                            log_storage_error("store_get", err);
                            None
                        }
                    }
                })
                .await;
                Ok(result)
            }
        })?,
    )?;
    Ok(())
}

fn register_store_delete(lua: &Lua, db: Arc<Db>, prefix: String) -> anyhow::Result<()> {
    lua.globals().set(
        "store_delete",
        lua.create_async_function(move |_, key: String| {
            let db = Arc::clone(&db);
            let prefix = prefix.clone();
            async move {
                smol::unblock(move || {
                    let prefixed = format!("{}{}", prefix, key);
                    if let Err(err) = db.lua_delete(&prefixed) {
                        log_storage_error("store_delete", err);
                    }
                })
                .await;
                Ok(())
            }
        })?,
    )?;
    Ok(())
}

fn register_global_store_set(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_set",
        lua.create_async_function(move |_, (key, value): (String, String)| {
            let db = Arc::clone(&db);
            async move {
                smol::unblock(move || {
                    if let Err(err) = db.lua_set(&key, &value) {
                        log_storage_error("global_store_set", err);
                    }
                })
                .await;
                Ok(())
            }
        })?,
    )?;
    Ok(())
}

fn register_global_store_get(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_get",
        lua.create_async_function(move |_, key: String| {
            let db = Arc::clone(&db);
            async move {
                let result = smol::unblock(move || match db.lua_get(&key) {
                    Ok(value) => value,
                    Err(err) => {
                        log_storage_error("global_store_get", err);
                        None
                    }
                })
                .await;
                Ok(result)
            }
        })?,
    )?;
    Ok(())
}

fn register_global_store_incr(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_incr",
        lua.create_async_function(move |_, (key, default, delta): (String, i64, i64)| {
            let db = Arc::clone(&db);
            async move {
                let result = smol::unblock(move || match db.lua_incr(&key, default, delta) {
                    Ok(val) => val,
                    Err(err) => {
                        log_storage_error("global_store_incr", err);
                        default
                    }
                })
                .await;
                Ok(result)
            }
        })?,
    )?;
    Ok(())
}

fn register_global_store_delete(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_delete",
        lua.create_async_function(move |_, key: String| {
            let db = Arc::clone(&db);
            async move {
                smol::unblock(move || {
                    if let Err(err) = db.lua_delete(&key) {
                        log_storage_error("global_store_delete", err);
                    }
                })
                .await;
                Ok(())
            }
        })?,
    )?;
    Ok(())
}
