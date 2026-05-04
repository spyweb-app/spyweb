use anyhow::Result;
use mlua::{Lua, LuaSerdeExt, Table, Value};
use std::collections::HashMap;

use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};

// Lua-shaped request: { url = "...", headers = { ["X-Foo"] = "bar" } }

pub fn request_to_lua(lua: &Lua, req: &RequestConfig) -> Result<Table> {
    let table = lua.create_table()?;
    table.set("url", req.url.as_str())?;

    let headers = lua.create_table()?;
    for (k, v) in &req.headers {
        headers.set(k.as_str(), v.as_str())?;
    }
    table.set("headers", headers)?;

    Ok(table)
}

pub fn lua_to_request(table: Table, original: RequestConfig) -> Result<RequestConfig> {
    let url: String = table.get::<Option<String>>("url")?.unwrap_or(original.url);

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

    Ok(RequestConfig { url, headers })
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

    Ok(table)
}

fn error_kind(message: &str) -> &'static str {
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
    })
}

pub fn lua_table_to_response(table: Table) -> Result<RequestResult> {
    let original = RequestResult {
        url: String::new(),
        status: 0,
        headers: HashMap::new(),
        body: String::new(),
        proxy: None,
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

    Ok(RequestResult {
        body,
        url,
        status,
        headers,
        proxy,
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
        match entry {
            Value::Table(t) => {
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
            _ => {}
        }
    }

    Ok(items)
}

pub fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> Result<Value> {
    lua.to_value(value).map_err(|e| anyhow::anyhow!(e))
}

pub fn lua_to_json(value: &Value) -> Result<serde_json::Value> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        Value::Integer(i) => Ok(serde_json::json!(*i)),
        Value::Number(f) => Ok(serde_json::json!(*f)),
        Value::String(s) => Ok(serde_json::Value::String(s.to_str()?.to_string())),
        Value::Table(t) => {
            let len = t.len()? as usize;
            let raw_len = t.raw_len();

            if raw_len > 0 && len == raw_len {
                let mut arr = Vec::with_capacity(len);
                for i in 1..=len {
                    let v: Value = t.get(i)?;
                    arr.push(lua_to_json(&v)?);
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.pairs::<Value, Value>() {
                    let (k, v) = pair?;
                    let key = match &k {
                        Value::String(s) => s.to_str()?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(f) => f.to_string(),
                        _ => continue,
                    };
                    map.insert(key, lua_to_json(&v)?);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        _ => Ok(serde_json::Value::Null),
    }
}
