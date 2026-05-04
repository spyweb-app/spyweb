use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use mlua::Lua;

use crate::lua::bindings;
use crate::services::db::Db;

pub fn create_engine(job_dir: Option<PathBuf>, db: Arc<Db>, job_name: &str) -> Result<Lua> {
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

    let lua = Lua::new_with(libs, mlua::LuaOptions::default())?;

    bindings::register_http_and_fs(&lua, job_dir)?;
    bindings::register(&lua, db, job_name)?;

    Ok(lua)
}

#[cfg(test)]
mod tests;
