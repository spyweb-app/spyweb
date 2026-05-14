use crate::services::db::{Db, LUA_USER_TABLE};
use mlua::Lua;
use redb::ReadableTable;
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
                    let write = match db.begin_write() {
                        Ok(write) => write,
                        Err(err) => {
                            log_storage_error("store_set", err);
                            return;
                        }
                    };
                    {
                        let mut table = match write.open_table(LUA_USER_TABLE) {
                            Ok(table) => table,
                            Err(err) => {
                                log_storage_error("store_set", err);
                                return;
                            }
                        };
                        if let Err(err) = table.insert(prefixed.as_str(), value.as_str()) {
                            log_storage_error("store_set", err);
                            return;
                        }
                    }
                    if let Err(err) = write.commit() {
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
                    let read = match db.begin_read() {
                        Ok(read) => read,
                        Err(err) => {
                            log_storage_error("store_get", err);
                            return None;
                        }
                    };
                    let table = match read.open_table(LUA_USER_TABLE) {
                        Ok(table) => table,
                        Err(redb::TableError::TableDoesNotExist(_)) => return None,
                        Err(err) => {
                            log_storage_error("store_get", err);
                            return None;
                        }
                    };
                    match table.get(prefixed.as_str()) {
                        Ok(value) => {
                            value.map(|v: redb::AccessGuard<&str>| v.value().to_string())
                        }
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
                    let write = match db.begin_write() {
                        Ok(write) => write,
                        Err(err) => {
                            log_storage_error("store_delete", err);
                            return;
                        }
                    };
                    {
                        let mut table = match write.open_table(LUA_USER_TABLE) {
                            Ok(table) => table,
                            Err(redb::TableError::TableDoesNotExist(_)) => return,
                            Err(err) => {
                                log_storage_error("store_delete", err);
                                return;
                            }
                        };
                        if let Err(err) = table.remove(prefixed.as_str()) {
                            log_storage_error("store_delete", err);
                            return;
                        }
                    }
                    if let Err(err) = write.commit() {
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
                    let write = match db.begin_write() {
                        Ok(write) => write,
                        Err(err) => {
                            log_storage_error("global_store_set", err);
                            return;
                        }
                    };
                    {
                        let mut table = match write.open_table(LUA_USER_TABLE) {
                            Ok(table) => table,
                            Err(err) => {
                                log_storage_error("global_store_set", err);
                                return;
                            }
                        };
                        if let Err(err) = table.insert(key.as_str(), value.as_str()) {
                            log_storage_error("global_store_set", err);
                            return;
                        }
                    }
                    if let Err(err) = write.commit() {
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
                let result = smol::unblock(move || {
                    let read = match db.begin_read() {
                        Ok(read) => read,
                        Err(err) => {
                            log_storage_error("global_store_get", err);
                            return None;
                        }
                    };
                    let table = match read.open_table(LUA_USER_TABLE) {
                        Ok(table) => table,
                        Err(redb::TableError::TableDoesNotExist(_)) => return None,
                        Err(err) => {
                            log_storage_error("global_store_get", err);
                            return None;
                        }
                    };
                    match table.get(key.as_str()) {
                        Ok(value) => {
                            value.map(|v: redb::AccessGuard<&str>| v.value().to_string())
                        }
                        Err(err) => {
                            log_storage_error("global_store_get", err);
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

fn register_global_store_incr(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "global_store_incr",
        lua.create_async_function(move |_, (key, default, delta): (String, i64, i64)| {
            let db = Arc::clone(&db);
            async move {
                let result = smol::unblock(move || {
                    let write = match db.begin_write() {
                        Ok(write) => write,
                        Err(err) => {
                            log_storage_error("global_store_incr", err);
                            return default;
                        }
                    };

                    let new_val_num = {
                        let mut table = match write.open_table(LUA_USER_TABLE) {
                            Ok(table) => table,
                            Err(err) => {
                                log_storage_error("global_store_incr", err);
                                return default;
                            }
                        };

                        let current = match table.get(key.as_str()) {
                            Ok(value) => value
                                .and_then(|v: redb::AccessGuard<&str>| v.value().parse::<i64>().ok())
                                .unwrap_or(default),
                            Err(err) => {
                                log_storage_error("global_store_incr", err);
                                return default;
                            }
                        };

                        let new_val_num = current + delta;
                        let new_val = new_val_num.to_string();

                        if let Err(err) = table.insert(key.as_str(), new_val.as_str()) {
                            log_storage_error("global_store_incr", err);
                            return default;
                        }

                        new_val_num
                    };

                    if let Err(err) = write.commit() {
                        log_storage_error("global_store_incr", err);
                        return default;
                    }

                    new_val_num
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
                    let write = match db.begin_write() {
                        Ok(write) => write,
                        Err(err) => {
                            log_storage_error("global_store_delete", err);
                            return;
                        }
                    };
                    {
                        let mut table = match write.open_table(LUA_USER_TABLE) {
                            Ok(table) => table,
                            Err(redb::TableError::TableDoesNotExist(_)) => return,
                            Err(err) => {
                                log_storage_error("global_store_delete", err);
                                return;
                            }
                        };
                        if let Err(err) = table.remove(key.as_str()) {
                            log_storage_error("global_store_delete", err);
                            return;
                        }
                    }
                    if let Err(err) = write.commit() {
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
