use std::collections::HashMap;

use anyhow::Result;
use mlua::{Lua, Table, Value};

use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{RequestConfig, RequestResult};

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

// Lua-shaped fetch result:
// success: { ok = true, body = "...", status = 200, url = "...", headers = {...}, proxy = "..."? }
// error:   { ok = false, error = "..." }
pub fn fetch_result_to_lua(lua: &Lua, res: &Result<RequestResult>) -> Result<Table> {
    let table = lua.create_table()?;
    match res {
        Ok(res) => {
            table.set("ok", true)?;
            table.set("body", res.body.as_str())?;
            table.set("status", res.status)?;
            table.set("url", res.url.as_str())?;

            let headers = lua.create_table()?;
            for (k, v) in &res.headers {
                headers.set(k.as_str(), v.as_str())?;
            }
            table.set("headers", headers)?;

            match &res.proxy {
                Some(proxy) => table.set("proxy", proxy.as_str())?,
                None => table.set("proxy", Value::Nil)?,
            }
        }
        Err(err) => {
            table.set("ok", false)?;
            table.set("error", err.to_string())?;
        }
    }
    Ok(table)
}

pub fn lua_to_response_body_only(table: Table, original: RequestResult) -> Result<RequestResult> {
    let body: String = table
        .get::<Option<String>>("body")?
        .unwrap_or(original.body.clone());

    Ok(RequestResult {
        body,
        url: original.url,
        status: original.status,
        headers: original.headers,
        proxy: original.proxy,
    })
}

pub fn lua_to_response(table: Table, original: Option<RequestResult>) -> Result<RequestResult> {
    let original = original.unwrap_or(RequestResult {
        url: String::new(),
        status: 0,
        headers: HashMap::new(),
        body: String::new(),
        proxy: None,
    });

    let body: String = table
        .get::<Option<String>>("body")?
        .unwrap_or(original.body.clone());
    let url: String = table
        .get::<Option<String>>("url")?
        .unwrap_or(original.url.clone());
    let status: u16 = table.get::<Option<u16>>("status")?.unwrap_or(original.status);
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
            Value::Nil => {}
            _ => {}
        }
    }

    Ok(items)
}

pub fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> Result<Value> {
    match value {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Number(f))
            } else {
                Ok(Value::Nil)
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let table = lua.create_table()?;
            for (i, v) in arr.iter().enumerate() {
                table.set(i + 1, json_to_lua(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
        serde_json::Value::Object(obj) => {
            let table = lua.create_table()?;
            for (k, v) in obj {
                table.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}

pub fn lua_to_json(value: &Value) -> Result<serde_json::Value> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        Value::Integer(i) => Ok(serde_json::json!(*i)),
        Value::Number(f) => Ok(serde_json::json!(*f)),
        Value::String(s) => Ok(serde_json::Value::String(s.to_str()?.to_string())),
        Value::Table(t) => {
            // Check if this looks like an array (sequential integer keys from 1)
            let len = t.len()? as usize;
            let raw_len = t.raw_len();

            if raw_len > 0 && len == raw_len {
                // Treat as array
                let mut arr = Vec::with_capacity(len);
                for i in 1..=len {
                    let v: Value = t.get(i)?;
                    arr.push(lua_to_json(&v)?);
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                // Treat as object
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

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    #[test]
    fn test_request_config_roundtrip() {
        let lua = Lua::new();
        let mut headers = HashMap::new();
        headers.insert("User-Agent".into(), "SpyWeb".into());
        let req = RequestConfig {
            url: "https://example.com".into(),
            headers,
        };

        let t = request_to_lua(&lua, &req).unwrap();

        lua.globals().set("req", t).unwrap();
        lua.load(
            r#"
            req.url = "https://example.com/mutated"
            req.headers["Authorization"] = "Bearer token"
        "#,
        )
        .exec()
        .unwrap();

        let t2: Table = lua.globals().get("req").unwrap();
        let final_req = lua_to_request(t2, req).unwrap();

        assert_eq!(final_req.url, "https://example.com/mutated");
        assert_eq!(final_req.headers.get("User-Agent").unwrap(), "SpyWeb");
        assert_eq!(
            final_req.headers.get("Authorization").unwrap(),
            "Bearer token"
        );
    }

    #[test]
    fn test_response_roundtrip() {
        let lua = Lua::new();
        let res = RequestResult {
            body: "<html></html>".into(),
            url: "https://example.com".into(),
            status: 200,
            headers: HashMap::new(),
            proxy: None,
        };

        let t = fetch_result_to_lua(&lua, &Ok(res.clone())).unwrap();

        lua.globals().set("res", t).unwrap();
        lua.load(
            r#"
            res.body = "MUTATED"
            res.status = 500 -- Should be ignored
            res.url = "http://hacked.com" -- Should be ignored
        "#,
        )
        .exec()
        .unwrap();

        let t2: Table = lua.globals().get("res").unwrap();
        let final_res = lua_to_response_body_only(t2, res).unwrap();

        assert_eq!(final_res.body, "MUTATED");
        assert_eq!(final_res.status, 200);
        assert_eq!(final_res.url, "https://example.com");
    }

    #[test]
    fn test_fetch_error_can_be_turned_into_response() {
        let lua = Lua::new();
        let err = anyhow::anyhow!("dns lookup failed");

        let t = fetch_result_to_lua(&lua, &Err(err)).unwrap();
        lua.globals().set("res", t).unwrap();
        lua.load(
            r#"
            assert(res.ok == false)
            assert(res.error ~= nil)
            res = {
                body = "<html>fallback</html>",
                status = 599,
                url = "https://fallback.example.com",
                headers = { ["content-type"] = "text/html" }
            }
        "#,
        )
        .exec()
        .unwrap();

        let t2: Table = lua.globals().get("res").unwrap();
        let final_res = lua_to_response(t2, None).unwrap();

        assert_eq!(final_res.body, "<html>fallback</html>");
        assert_eq!(final_res.status, 599);
        assert_eq!(final_res.url, "https://fallback.example.com");
        assert_eq!(
            final_res.headers.get("content-type").map(String::as_str),
            Some("text/html")
        );
        assert_eq!(final_res.proxy, None);
    }

    #[test]
    fn test_items_roundtrip_with_reordering_and_filtering() {
        let lua = Lua::new();

        let item1 = ExtractedItem {
            fields: HashMap::from([("title".into(), "Item 1".into())]),
            matches: vec!["match1".into()],
            parent_html: Some("<div1>".into()),
            field_match_html: None,
            ..Default::default()
        };
        let item2 = ExtractedItem {
            fields: HashMap::from([("title".into(), "Item 2".into())]),
            matches: vec!["match2".into()],
            parent_html: Some("<div2>".into()),
            field_match_html: None,
            ..Default::default()
        };
        let item3 = ExtractedItem {
            fields: HashMap::from([("title".into(), "Item 3".into())]),
            matches: vec!["match3".into()],
            parent_html: Some("<div3>".into()),
            field_match_html: None,
            ..Default::default()
        };

        let originals = vec![item1, item2, item3];
        let t = items_to_lua(&lua, &originals).unwrap();

        lua.globals().set("items", t).unwrap();

        lua.load(
            r#"
            local new_items = {}
            new_items[1] = items[3]
            new_items[2] = items[1]
            new_items[1].fields.title = "Item 3 Mutated"
            items = new_items
        "#,
        )
        .exec()
        .unwrap();

        let t2: Table = lua.globals().get("items").unwrap();
        let final_items = lua_to_items(t2, originals).unwrap();

        assert_eq!(final_items.len(), 2);

        assert_eq!(
            final_items[0].fields.get("title").unwrap(),
            "Item 3 Mutated"
        );
        assert_eq!(final_items[0].matches[0], "match3");
        assert_eq!(final_items[0].parent_html.as_deref(), Some("<div3>"));

        assert_eq!(final_items[1].fields.get("title").unwrap(), "Item 1");
        assert_eq!(final_items[1].matches[0], "match1");
        assert_eq!(final_items[1].parent_html.as_deref(), Some("<div1>"));
    }

    #[test]
    fn test_json_conversion() {
        let lua = Lua::new();
        let json_val = serde_json::json!({
            "string": "hello",
            "number": 42,
            "bool": true,
            "array": [1, 2, "three"],
            "obj": { "nested": "value" }
        });

        let lua_val = json_to_lua(&lua, &json_val).unwrap();
        let final_json = lua_to_json(&lua_val).unwrap();

        assert_eq!(json_val, final_json);
    }
}
