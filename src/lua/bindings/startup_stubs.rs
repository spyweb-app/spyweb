use mlua::{Lua, Table, Value};

const STORE_ERROR: &str =
    "store_* is job-scoped and unavailable in startup.lua; use global_store_* or db_*";
const DEFER_ERROR: &str =
    "defer is only available inside cycle hooks and server; not available in startup.lua";
const QUEUE_ERROR: &str =
    "queue_push is only available inside on_start; not available in startup.lua";
const CDP_ERROR: &str = "cdp is not available in startup.lua";

pub fn register(lua: &Lua) -> mlua::Result<()> {
    for name in ["store_set", "store_get", "store_delete"] {
        lua.globals().set(
            name,
            lua.create_function(move |_, _: mlua::MultiValue| -> mlua::Result<Value> {
                Err(mlua::Error::runtime(STORE_ERROR))
            })?,
        )?;
    }

    lua.globals().set(
        "defer",
        lua.create_function(|_, _: mlua::MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::runtime(DEFER_ERROR))
        })?,
    )?;

    lua.globals().set(
        "queue_push",
        lua.create_function(|_, _: mlua::MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::runtime(QUEUE_ERROR))
        })?,
    )?;

    register_cdp_stub(lua)
}

fn register_cdp_stub(lua: &Lua) -> mlua::Result<()> {
    let cdp = lua.create_table()?;
    let metatable = lua.create_table()?;
    metatable.set(
        "__index",
        lua.create_function(|_, (_table, _key): (Table, Value)| -> mlua::Result<Value> {
            Err(mlua::Error::runtime(CDP_ERROR))
        })?,
    )?;
    cdp.set_metatable(Some(metatable))?;
    lua.globals().set("cdp", cdp)?;
    Ok(())
}
