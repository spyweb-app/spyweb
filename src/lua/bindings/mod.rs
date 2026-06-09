use crate::services::db::Db;
use mlua::Lua;
use std::path::PathBuf;
use std::sync::Arc;

pub mod cdp;
pub mod network;
pub mod storage;
pub mod system;
pub mod testing;

#[cfg(feature = "sqlite")]
pub mod sql;

#[cfg(not(feature = "sqlite"))]
fn sqlite_variant_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "spyweb-linux-sql"
    } else if cfg!(target_os = "macos") {
        "spyweb-macos-sql"
    } else if cfg!(target_os = "windows") {
        "spyweb-windows-sql"
    } else {
        "spyweb-sql"
    }
}

pub fn register(lua: &Lua, db: Arc<Db>, job_name: &str) -> anyhow::Result<()> {
    storage::register(lua, Arc::clone(&db), job_name)?;
    testing::register(lua)?;
    #[cfg(feature = "sqlite")]
    sql::register(lua, db)?;
    #[cfg(not(feature = "sqlite"))]
    register_sql_stubs(lua)?;
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
fn register_sql_stubs(lua: &Lua) -> anyhow::Result<()> {
    let msg = format!(
        "db_query/db_exec require the {} variant — download it from the releases page",
        sqlite_variant_name(),
    );
    let q_msg = msg.clone();
    lua.globals().set(
        "db_query",
        lua.create_function(move |_, _: ()| -> mlua::Result<()> {
            Err(mlua::Error::external(q_msg.clone()))
        })?,
    )?;
    lua.globals().set(
        "db_exec",
        lua.create_function(move |_, _: ()| -> mlua::Result<()> {
            Err(mlua::Error::external(msg.clone()))
        })?,
    )?;
    Ok(())
}

pub fn register_http_and_fs(lua: &Lua, job_dir: Option<PathBuf>) -> mlua::Result<()> {
    network::register(lua)?;
    system::register(lua, job_dir.clone())?;
    Ok(())
}

pub fn register_cdp(lua: &Lua, job_dir: Option<PathBuf>) -> mlua::Result<()> {
    cdp::register(lua, job_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests;
