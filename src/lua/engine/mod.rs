use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use mlua::Lua;

use crate::lua::bindings;
use crate::services::db::Db;

fn create_lua() -> Result<Lua> {
    #[cfg(feature = "luau")]
    let libs = mlua::StdLib::TABLE
        | mlua::StdLib::STRING
        | mlua::StdLib::UTF8
        | mlua::StdLib::MATH
        | mlua::StdLib::OS
        | mlua::StdLib::BIT
        | mlua::StdLib::COROUTINE;

    #[cfg(not(feature = "luau"))]
    let libs = mlua::StdLib::ALL;

    Ok(Lua::new_with(libs, mlua::LuaOptions::default())?)
}

pub fn create_engine(
    job_dir: Option<PathBuf>,
    db: Arc<Db>,
    job_name: &str,
    uses_cdp: bool,
) -> Result<Lua> {
    let lua = create_lua()?;

    bindings::register_http_and_fs(&lua, job_dir.clone(), job_name)?;
    if uses_cdp {
        bindings::register_cdp(&lua, job_dir)?;
    }
    bindings::register(&lua, db, job_name)?;

    Ok(lua)
}

pub fn create_startup_engine(job_dir: Option<PathBuf>, db: Arc<Db>) -> Result<Lua> {
    let lua = create_lua()?;
    bindings::register_startup(&lua, job_dir, db)?;
    Ok(lua)
}

#[cfg(test)]
mod tests;
