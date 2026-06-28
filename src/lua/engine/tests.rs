use crate::lua::conversions::*;
use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};
use indexmap::IndexMap;
use mlua::{Lua, Table};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn test_request_config_roundtrip() {
    let lua = Lua::new();
    let mut headers = IndexMap::new();
    headers.insert("User-Agent".into(), "SpyWeb".into());
    let req = RequestConfig {
        url: "https://example.com".into(),
        method: "HEAD".into(),
        headers: Arc::new(headers),
        timeout: None,
        proxy: None,
        max_body_size: None,
    };

    let t = request_to_lua(&lua, &req).unwrap();

    // new fields should not be present in the table when None
    assert!(t.get::<Option<u64>>("timeout").unwrap().is_none());
    assert!(t.get::<Option<String>>("proxy").unwrap().is_none());
    assert!(t.get::<Option<u64>>("max_body_size").unwrap().is_none());

    lua.globals().set("req", t).unwrap();
    lua.load(
        r#"
        req.url = "https://example.com/mutated"
        req.method = "GET"
        req.headers["Authorization"] = "Bearer token"
        req.timeout = 15
        req.proxy = "http://proxy:8080"
        req.max_body_size = 5    -- 5 MB
    "#,
    )
    .exec()
    .unwrap();

    let t2: Table = lua.globals().get("req").unwrap();
    let final_req = lua_to_request(t2, req).unwrap();

    assert_eq!(final_req.url, "https://example.com/mutated");
    assert_eq!(final_req.method, "GET");
    assert_eq!(final_req.headers.get("User-Agent").unwrap(), "SpyWeb");
    assert_eq!(
        final_req.headers.get("Authorization").unwrap(),
        "Bearer token"
    );
    assert_eq!(final_req.timeout, Some(15));
    assert_eq!(final_req.proxy.as_deref(), Some("http://proxy:8080"));
    assert_eq!(final_req.max_body_size, Some(5 * 1024 * 1024));
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
        time_ms: None,
        size: None,
    };

    let attempt = FetchAttempt {
        request: RequestConfig {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: Arc::new(IndexMap::from([("User-Agent".into(), "SpyWeb".into())])),
            timeout: None,
            proxy: None,
            max_body_size: None,
        },
        proxy: None,
        result: Ok(res.clone()),
    };

    let t = fetch_result_to_lua(&lua, &attempt).unwrap();

    lua.globals().set("res", t).unwrap();
    lua.load(
        r#"
        assert(res.request.url == "https://example.com")
        assert(res.response.status == 200)
        res.request.url = "http://hacked.com" -- Should be ignored
        res.response.body = "MUTATED"
        res.response.status = 500 -- Should be ignored
        res.response.url = "http://hacked.com" -- Should be ignored
    "#,
    )
    .exec()
    .unwrap();

    let t2: Table = lua.globals().get("res").unwrap();
    let final_res = lua_to_after_fetch_success_response(t2, res).unwrap();

    assert_eq!(final_res.body, "MUTATED");
    assert_eq!(final_res.status, 200);
    assert_eq!(final_res.url, "https://example.com");
}

#[test]
fn test_fetch_error_envelope_can_be_turned_into_response() {
    let lua = Lua::new();
    let attempt = FetchAttempt {
        request: RequestConfig {
            url: "https://example.com/products".into(),
            method: "GET".into(),
            headers: Arc::new(IndexMap::from([("Accept".into(), "text/html".into())])),
            timeout: None,
            proxy: None,
            max_body_size: None,
        },
        proxy: Some("http://proxy-1:8080".into()),
        result: Err("request failed for job 'test': dns lookup failed".into()),
    };

    let t = fetch_result_to_lua(&lua, &attempt).unwrap();
    lua.globals().set("res", t).unwrap();
    lua.load(
        r#"
        assert(res.ok == false)
        assert(res.response == nil)
        assert(res.request.url == "https://example.com/products")
        assert(res.request.proxy == "http://proxy-1:8080")
        assert(res.request.headers["Accept"] == "text/html")
        assert(res.error.message ~= nil)
        assert(res.error.kind == "dns")
        res.response = {
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
    let final_res = lua_to_after_fetch_error_response(t2).unwrap();

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
fn test_fetch_error_backcompat_top_level_response_still_works() {
    let lua = Lua::new();
    let attempt = FetchAttempt {
        request: RequestConfig {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: Arc::new(IndexMap::new()),
            timeout: None,
            proxy: None,
            max_body_size: None,
        },
        proxy: None,
        result: Err("request failed for job 'test': timed out".into()),
    };

    let t = fetch_result_to_lua(&lua, &attempt).unwrap();
    lua.globals().set("res", t).unwrap();
    lua.load(
        r#"
        res = {
            body = "fallback",
            status = 598,
            url = "https://fallback.example.com"
        }
    "#,
    )
    .exec()
    .unwrap();

    let t2: Table = lua.globals().get("res").unwrap();
    let final_res = lua_to_after_fetch_error_response(t2).unwrap();

    assert_eq!(final_res.body, "fallback");
    assert_eq!(final_res.status, 598);
    assert_eq!(final_res.url, "https://fallback.example.com");
}

#[test]
fn test_http_error_response_has_response_and_not_ok() {
    let lua = Lua::new();
    let attempt = FetchAttempt {
        request: RequestConfig {
            url: "https://example.com/protected".into(),
            method: "GET".into(),
            headers: Arc::new(IndexMap::new()),
            timeout: None,
            proxy: None,
            max_body_size: None,
        },
        proxy: None,
        result: Ok(RequestResult {
            body: "blocked".into(),
            url: "https://example.com/protected".into(),
            status: 403,
            headers: HashMap::from([("content-type".into(), "text/html".into())]),
            proxy: None,
            time_ms: None,
            size: None,
        }),
    };

    let t = fetch_result_to_lua(&lua, &attempt).unwrap();
    lua.globals().set("res", t).unwrap();
    lua.load(
        r#"
        assert(res.ok == false)
        assert(res.response ~= nil)
        assert(res.response.status == 403)
        assert(res.response.body == "blocked")
        assert(res.error.message == "http status: 403")
        assert(res.error.kind == "http")
    "#,
    )
    .exec()
    .unwrap();
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
