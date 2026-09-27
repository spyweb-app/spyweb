use crate::services::db::Db;
use crate::services::db::sqlite::SqlValue;
use mlua::{Lua, Table, Value};
use std::sync::Arc;

pub fn register(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    register_query(lua, Arc::clone(&db))?;
    register_exec(lua, db)?;
    Ok(())
}

fn lua_value_to_sql(val: &Value) -> rusqlite::types::Value {
    match val {
        Value::Nil => rusqlite::types::Value::Null,
        Value::Integer(n) => rusqlite::types::Value::Integer(*n),
        Value::Number(f) => rusqlite::types::Value::Real(*f),
        Value::String(s) => {
            let s: String = s.to_str().map(|s| s.to_string()).unwrap_or_default();
            rusqlite::types::Value::Text(s)
        }
        _ => rusqlite::types::Value::Null,
    }
}

fn table_to_sql_params(params: &Table) -> mlua::Result<Vec<rusqlite::types::Value>> {
    let mut max_key: i64 = 0;
    for pair in params.clone().pairs::<i64, Value>() {
        let (k, _) = pair?;
        if k >= 1 && k > max_key {
            max_key = k;
        }
    }

    let mut out = Vec::with_capacity(max_key as usize);
    for i in 1..=max_key {
        let v: Value = params.get(i)?;
        out.push(lua_value_to_sql(&v));
    }
    Ok(out)
}

fn params_to_sql(params: Option<Table>) -> mlua::Result<Vec<rusqlite::types::Value>> {
    match params {
        Some(table) => table_to_sql_params(&table),
        None => Ok(Vec::new()),
    }
}

fn sql_value_to_lua(lua: &Lua, val: &SqlValue) -> mlua::Result<Value> {
    match val {
        SqlValue::Null => Ok(Value::Nil),
        SqlValue::Integer(n) => Ok(Value::Integer(*n)),
        SqlValue::Real(f) => Ok(Value::Number(*f)),
        SqlValue::Text(s) => Ok(Value::String(lua.create_string(s)?)),
    }
}

fn register_query(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "db_query",
        lua.create_async_function(move |lua, (sql, params): (String, Option<Table>)| {
            let db = Arc::clone(&db);
            async move {
                let params = params_to_sql(params)?;

                let result = smol::unblock(move || db.db_query(&sql, &params)).await;

                match result {
                    Ok(rows) => {
                        let table = lua.create_table()?;
                        for (i, row) in rows.iter().enumerate() {
                            let row_table = lua.create_table()?;
                            for (col, val) in row {
                                row_table.set(col.as_str(), sql_value_to_lua(&lua, val)?)?;
                            }
                            table.set(i + 1, row_table)?;
                        }
                        Ok(table)
                    }
                    Err(err) => Err(mlua::Error::external(err.to_string())),
                }
            }
        })?,
    )?;
    Ok(())
}

fn register_exec(lua: &Lua, db: Arc<Db>) -> anyhow::Result<()> {
    lua.globals().set(
        "db_exec",
        lua.create_async_function(move |_, (sql, params): (String, Option<Table>)| {
            let db = Arc::clone(&db);
            async move {
                let params = params_to_sql(params)?;

                let result = smol::unblock(move || db.db_exec(&sql, &params)).await;

                match result {
                    Ok(changes) => Ok(changes),
                    Err(err) => Err(mlua::Error::external(err.to_string())),
                }
            }
        })?,
    )?;
    Ok(())
}
