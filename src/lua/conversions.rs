use anyhow::Result;
use indexmap::IndexMap;
use mlua::{Lua, LuaSerdeExt, Table, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};

// Lua-shaped request: { url = "...", headers = { ["X-Foo"] = "bar" } }

pub fn request_to_lua(lua: &Lua, req: &RequestConfig) -> Result<Table> {
    let table = lua.create_table()?;
    table.set("url", req.url.as_str())?;
    table.set("method", req.method.as_str())?;

    let headers = lua.create_table()?;
    for (k, v) in req.headers.iter() {
        headers.set(k.as_str(), v.as_str())?;
    }
    table.set("headers", headers)?;

    if let Some(timeout) = req.timeout {
        table.set("timeout", timeout)?;
    }
    if let Some(ref proxy) = req.proxy {
        table.set("proxy", proxy.as_str())?;
    }
    if let Some(bytes) = req.max_body_size {
        // expose to Lua as MB
        table.set("max_body_size", bytes / 1024 / 1024)?;
    }

    Ok(table)
}

pub fn lua_to_request(table: Table, original: RequestConfig) -> Result<RequestConfig> {
    let url: String = table.get::<Option<String>>("url")?.unwrap_or(original.url);
    let method: String = table
        .get::<Option<String>>("method")?
        .unwrap_or(original.method);

    let headers: Arc<IndexMap<String, String>> = match table.get::<Option<Table>>("headers")? {
        Some(h) => {
            let mut map = IndexMap::new();
            for pair in h.pairs::<String, String>() {
                let (k, v) = pair?;
                map.insert(k, v);
            }
            Arc::new(map)
        }
        None => original.headers,
    };

    let timeout = table.get::<Option<u64>>("timeout")?.or(original.timeout);
    let proxy = table.get::<Option<String>>("proxy")?.or(original.proxy);
    let max_body_size = table
        .get::<Option<u64>>("max_body_size")?
        .map(|mb| mb * 1024 * 1024)
        .or(original.max_body_size);

    Ok(RequestConfig {
        url,
        method,
        headers,
        timeout,
        proxy,
        max_body_size,
    })
}

pub fn response_to_lua(lua: &Lua, res: &RequestResult) -> Result<Table> {
    let table = lua.create_table()?;
    table.set("status", res.status)?;
    table.set("url", res.url.as_str())?;
    table.set("body", res.body.as_str())?;

    let headers = lua.create_table()?;
    for (k, v) in &res.headers {
        headers.set(k.as_str(), v.as_str())?;
    }
    table.set("headers", headers)?;

    if let Some(ref proxy) = res.proxy {
        table.set("proxy", proxy.as_str())?;
    }
    if let Some(time_ms) = res.time_ms {
        table.set("time_ms", time_ms)?;
    }
    if let Some(size) = res.size {
        table.set("size", size)?;
    }

    Ok(table)
}

pub(crate) fn error_kind(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("http status:") {
        "http"
    } else if lower.contains("dns") || lower.contains("resolve") {
        "dns"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else if lower.contains("tls") || lower.contains("certificate") {
        "tls"
    } else if lower.contains("proxy") {
        "proxy"
    } else if lower.contains("connect") || lower.contains("connection") {
        "connect"
    } else if lower.contains("exceeds") {
        "size"
    } else {
        "unknown"
    }
}

fn is_success_status(status: u16) -> bool {
    (200..300).contains(&status)
}

// Lua-shaped fetch result:
// {
//   ok = boolean,
//   request = { url = "...", headers = {...}, proxy = "..."? },
//   response = { status = 200, url = "...", body = "...", headers = {...} } | nil,
//   error = { message = "...", kind = "..." } | nil
// }
pub fn fetch_result_to_lua(lua: &Lua, attempt: &FetchAttempt) -> Result<Table> {
    let table = lua.create_table()?;

    table.set("request", request_to_lua(lua, &attempt.request)?)?;
    let request: Table = table.get("request")?;
    match &attempt.proxy {
        Some(proxy) => request.set("proxy", proxy.as_str())?,
        None => request.set("proxy", Value::Nil)?,
    }

    match &attempt.result {
        Ok(res) => {
            table.set("ok", is_success_status(res.status))?;
            table.set("response", response_to_lua(lua, res)?)?;
            if is_success_status(res.status) {
                table.set("error", Value::Nil)?;
            } else {
                let error = lua.create_table()?;
                error.set("message", format!("http status: {}", res.status))?;
                error.set("kind", "http")?;
                table.set("error", error)?;
            }
        }
        Err(message) => {
            table.set("ok", false)?;
            table.set("response", Value::Nil)?;
            let error = lua.create_table()?;
            error.set("message", message.as_str())?;
            error.set("kind", error_kind(message))?;
            table.set("error", error)?;
        }
    }
    Ok(table)
}

pub fn lua_to_after_fetch_success_response(
    table: Table,
    original: RequestResult,
) -> Result<RequestResult> {
    let body = match table.get::<Option<Table>>("response")? {
        Some(response) => response
            .get::<Option<String>>("body")?
            .unwrap_or(original.body.clone()),
        None => original.body.clone(),
    };

    Ok(RequestResult {
        body,
        url: original.url,
        status: original.status,
        headers: original.headers,
        proxy: original.proxy,
        time_ms: original.time_ms,
        size: original.size,
    })
}

pub fn lua_table_to_response(table: Table) -> Result<RequestResult> {
    let original = RequestResult {
        url: String::new(),
        status: 0,
        headers: HashMap::new(),
        body: String::new(),
        proxy: None,
        time_ms: None,
        size: None,
    };

    let body: String = table
        .get::<Option<String>>("body")?
        .unwrap_or(original.body.clone());
    let url: String = table
        .get::<Option<String>>("url")?
        .unwrap_or(original.url.clone());
    let status: u16 = table
        .get::<Option<u16>>("status")?
        .unwrap_or(original.status);
    let headers: HashMap<String, String> = match table.get::<Option<Table>>("headers")? {
        Some(h) => {
            let mut map = HashMap::new();
            for pair in h.pairs::<String, String>() {
                let (k, v) = pair?;
                map.insert(k, v);
            }
            map
        }
        None => original.headers,
    };
    let proxy = table
        .get::<Option<String>>("proxy")?
        .or(original.proxy.clone());
    let time_ms = table.get::<Option<u64>>("time_ms")?.or(original.time_ms);
    let size = table.get::<Option<u64>>("size")?.or(original.size);

    Ok(RequestResult {
        body,
        url,
        status,
        headers,
        proxy,
        time_ms,
        size,
    })
}

pub fn lua_to_after_fetch_error_response(table: Table) -> Result<RequestResult> {
    if let Some(response) = table.get::<Option<Table>>("response")? {
        return lua_table_to_response(response);
    }
    lua_table_to_response(table)
}

// Lua-shaped item: { fields = { title = "...", link = "..." }, matches = { "rust", ... } }

pub fn item_to_lua(lua: &Lua, item: &ExtractedItem) -> Result<Table> {
    let table = lua.create_table()?;

    let fields = lua.create_table()?;
    for (k, v) in &item.fields {
        fields.set(k.as_str(), v.as_str())?;
    }
    table.set("fields", fields)?;

    let matches = lua.create_table()?;
    for (i, m) in item.matches.iter().enumerate() {
        matches.set(i + 1, m.as_str())?;
    }
    table.set("matches", matches)?;

    Ok(table)
}

pub fn lua_to_item(table: Table, original: ExtractedItem) -> Result<ExtractedItem> {
    let fields: HashMap<String, String> = match table.get::<Option<Table>>("fields")? {
        Some(f) => {
            let mut map = HashMap::new();
            for pair in f.pairs::<String, String>() {
                let (k, v) = pair?;
                map.insert(k, v);
            }
            map
        }
        None => original.fields,
    };

    Ok(ExtractedItem {
        fields,
        matches: original.matches,
        parent_html: original.parent_html,
        field_match_html: original.field_match_html,
        ..Default::default()
    })
}

//  Vec<ExtractedItem> ↔ Lua array

pub fn items_to_lua(lua: &Lua, items: &[ExtractedItem]) -> Result<Table> {
    let table = lua.create_table()?;
    for (i, item) in items.iter().enumerate() {
        let item_table = item_to_lua(lua, item)?;
        item_table.set("_meta_idx", i)?;
        table.set(i + 1, item_table)?;
    }
    Ok(table)
}

pub fn lua_to_items(table: Table, originals: Vec<ExtractedItem>) -> Result<Vec<ExtractedItem>> {
    let len = table.len()? as usize;
    let mut items = Vec::with_capacity(len);

    for i in 1..=len {
        let entry: Value = table.get(i)?;
        if let Value::Table(t) = entry {
            let original = match t.get::<Option<usize>>("_meta_idx")? {
                Some(idx) if idx < originals.len() => originals[idx].clone(),
                _ => ExtractedItem {
                    fields: HashMap::new(),
                    matches: vec![],
                    parent_html: None,
                    field_match_html: None,
                    ..Default::default()
                },
            };

            items.push(lua_to_item(t, original)?);
        }
    }

    Ok(items)
}

pub fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> Result<Value> {
    lua.to_value(value).map_err(|e| anyhow::anyhow!(e))
}

const JSON_EMPTY_ARRAY_REGISTRY_KEY: &str = "spyweb.json_empty_array.sentinel";

pub fn register_json_empty_array(lua: &Lua) -> Result<()> {
    if lua
        .named_registry_value::<mlua::Table>(JSON_EMPTY_ARRAY_REGISTRY_KEY)
        .is_ok()
    {
        return Ok(());
    }
    let sentinel = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__name", "JSON_EMPTY_ARRAY")?;
    sentinel.set_metatable(Some(meta))?;
    lua.set_named_registry_value(JSON_EMPTY_ARRAY_REGISTRY_KEY, sentinel.clone())?;
    lua.globals().set("JSON_EMPTY_ARRAY", sentinel)?;
    Ok(())
}

pub fn lua_to_json(lua: &Lua, value: &Value, array: Option<bool>) -> Result<serde_json::Value> {
    let flag = array.unwrap_or(false);
    let sentinel: Table = lua.named_registry_value(JSON_EMPTY_ARRAY_REGISTRY_KEY)?;
    let mut visited = HashSet::new();
    lua_to_json_inner(value, flag, &sentinel, &mut visited)
}

fn lua_to_json_inner(
    value: &Value,
    array: bool,
    sentinel: &Table,
    visited: &mut HashSet<usize>,
) -> Result<serde_json::Value> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(v) => Ok(serde_json::Value::Bool(*v)),
        Value::Integer(v) => Ok(serde_json::json!(*v)),
        Value::Number(v) => Ok(serde_json::json!(*v)),
        Value::String(v) => Ok(serde_json::Value::String(v.to_str()?.to_string())),
        Value::Table(table) => {
            if table.to_pointer() == sentinel.to_pointer() {
                return Ok(serde_json::Value::Array(vec![]));
            }

            let pointer = table.to_pointer() as usize;
            if !visited.insert(pointer) {
                return Err(anyhow::anyhow!("recursive table detected"));
            }

            let pairs: Vec<(Value, Value)> = table
                .pairs::<Value, Value>()
                .collect::<mlua::Result<Vec<_>>>()?;

            let is_sequence = !pairs.is_empty()
                && pairs.len() == table.raw_len()
                && pairs.iter().all(|(key, _)| {
                    matches!(key, Value::Integer(i) if *i >= 1 && (*i as usize) <= pairs.len())
                })
                && {
                    let mut indexes = pairs
                        .iter()
                        .filter_map(|(key, _)| match key {
                            Value::Integer(i) => Some(*i as usize),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    indexes.sort_unstable();
                    indexes == (1..=pairs.len()).collect::<Vec<_>>()
                };

            let result = if is_sequence {
                let mut ordered = pairs;
                ordered.sort_by_key(|(key, _)| match key {
                    Value::Integer(i) => *i,
                    _ => unreachable!(),
                });
                serde_json::Value::Array(
                    ordered
                        .iter()
                        .map(|(_, value)| lua_to_json_inner(value, array, sentinel, visited))
                        .collect::<Result<Vec<_>>>()?,
                )
            } else if pairs.is_empty() && array {
                serde_json::Value::Array(vec![])
            } else {
                let mut object = serde_json::Map::new();
                for (key, value) in &pairs {
                    let key = match key {
                        Value::String(s) => s.to_str()?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => {
                            return Err(anyhow::anyhow!(
                                "cannot serialize Lua table key of type {}",
                                key.type_name()
                            ));
                        }
                    };
                    object.insert(key, lua_to_json_inner(value, array, sentinel, visited)?);
                }
                serde_json::Value::Object(object)
            };

            visited.remove(&pointer);
            Ok(result)
        }
        _ => Err(anyhow::anyhow!(
            "cannot serialize Lua value of type {}",
            value.type_name()
        )),
    }
}
