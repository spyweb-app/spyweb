use crate::services::db::Db;
use mlua::Lua;
use std::path::PathBuf;
use std::sync::Arc;

pub mod cdp;
pub mod network;
pub mod storage;
pub mod system;
pub mod testing;

#[cfg(test)]
mod tests;

pub fn register(lua: &Lua, db: Arc<Db>, job_name: &str) -> anyhow::Result<()> {
    storage::register(lua, db, job_name)?;
    testing::register(lua)?;
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
