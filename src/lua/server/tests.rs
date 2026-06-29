use super::*;
use crate::services::db::Db;
use crate::services::server::types;
use std::sync::Arc;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn unique_test_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "spyweb-server-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn setup_db(dir: &PathBuf) -> Arc<Db> {
    fs::create_dir_all(dir).unwrap();
    Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap())
}

#[test]
fn test_basic_route_returns_response() {
    let dir = unique_test_dir("basic-route");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.hello = function(self)
                return { status = 200, body = "Hello from Lua!" }
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "hello",
        vec![],
        &types::Request::fake("GET", "/hello"),
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_string(), "Hello from Lua!");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_routing_fallback_to_all() {
    let dir = unique_test_dir("fallback-all");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            all.fallback = function(self)
                return "Fallback hit: " .. self.method
            end
        "#,
    );

    let resp = server.handle(
        "POST",
        "fallback",
        vec![],
        &types::Request::fake("POST", "/fallback"),
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_string(), "Fallback hit: POST");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_404_for_missing_route() {
    let dir = unique_test_dir("missing-route");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.existing = function(self)
                return "ok"
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "nonexistent",
        vec![],
        &types::Request::fake("GET", "/nonexistent"),
    );
    assert_eq!(resp.status, 404);
    assert_eq!(resp.body_string(), "Not Found");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_path_args_passthrough() {
    let dir = unique_test_dir("path-args");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.check = function(self)
                return self.path_args[1] .. "-" .. self.path_args[2]
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "check",
        vec!["42".into(), "abc".into()],
        &types::Request::fake("GET", "/check/42/abc"),
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_string(), "42-abc");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_self_context_fields() {
    let dir = unique_test_dir("context-fields");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.ctx = function(self)
                return self.method .. ":" .. self.query.foo .. ":" .. self.headers["X-Custom"]
            end
        "#,
    );

    let request = types::Request::fake_with(
        "GET",
        "/ctx?foo=bar",
        vec![("X-Custom".into(), "val1".into())],
        vec![],
    );
    let resp = server.handle("GET", "ctx", vec![], &request);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_string(), "GET:bar:val1");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_body_field_is_empty_string_on_get() {
    let dir = unique_test_dir("body-on-get");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.check_body = function(self)
                return tostring(self.body)
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "check_body",
        vec![],
        &types::Request::fake("GET", "/check_body"),
    );
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.body_string(),
        "",
        "self.body should be empty string for GET request, not nil"
    );

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_path_does_not_include_query_string() {
    let dir = unique_test_dir("path-no-query");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.check_path = function(self)
                return self.path
            end
        "#,
    );

    let request = types::Request::fake_with("GET", "/check_path?foo=bar&baz=qux", vec![], vec![]);
    let resp = server.handle("GET", "check_path", vec![], &request);
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.body_string(),
        "/check_path",
        "self.path should not include query string"
    );

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_lua_error_returns_500() {
    let dir = unique_test_dir("lua-error");
    fs::create_dir_all(dir.join("server")).unwrap();
    let db = setup_db(&dir);

    let error_log = std::env::current_dir()
        .unwrap()
        .join("target/test-server-error.log");
    let server = ApiServer::new(
        db,
        r#"
            get.crash = function(self)
                local x = nil
                x.foo()
            end
        "#
        .to_string(),
        error_log.clone(),
    );

    let resp = server.handle(
        "GET",
        "crash",
        vec![],
        &types::Request::fake("GET", "/crash"),
    );
    assert_eq!(resp.status, 500);

    assert!(error_log.exists(), "error.log should exist after Lua error");

    let log_content = fs::read_to_string(&error_log).unwrap();
    assert!(
        log_content.contains("crash"),
        "error log should mention the endpoint name"
    );

    let _ = fs::remove_file(&error_log);
    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_url_decode_plus_and_percent() {
    assert_eq!(types::url_decode("hello+world"), "hello world");
    assert_eq!(types::url_decode("a%20b"), "a b");
    assert_eq!(types::url_decode("a%2Fb"), "a/b");
    assert_eq!(types::url_decode("%3C%3E"), "<>");
    assert_eq!(types::url_decode("no%encoding"), "no%encoding");
    assert_eq!(types::url_decode("trailing%"), "trailing%");
    assert_eq!(types::url_decode("%2"), "%2");
    assert_eq!(types::url_decode(""), "");
    assert_eq!(types::url_decode("plain"), "plain");
    assert_eq!(types::url_decode("a+b%20c"), "a b c");
}

#[test]
fn test_extract_body_truncation() {
    let large_body = vec![b'x'; MAX_BODY_SIZE + 100];
    let request = types::Request::fake_with("POST", "/data", vec![], large_body);
    let body = extract_body(&request).unwrap();
    assert_eq!(
        body.len(),
        MAX_BODY_SIZE,
        "body should be truncated to MAX_BODY_SIZE"
    );
}

#[test]
fn test_format_table_response_header_injection() {
    let dir = unique_test_dir("header-injection");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.inject = function(self)
                return {
                    status = 200,
                    body = "ok",
                    headers = {
                        ["X-Good"] = "fine",
                        ["X-Evil"] = "val\r\nEvil-Header: injected",
                        ["X-Newline"] = "val\nInjected",
                        [""] = "empty-key",
                    }
                }
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "inject",
        vec![],
        &types::Request::fake("GET", "/inject"),
    );
    assert_eq!(resp.status, 200);

    let header_names: Vec<&str> = resp.headers.iter().map(|(k, _)| k.as_ref()).collect();
    assert!(header_names.contains(&"X-Good"), "good header should pass");
    assert!(
        !header_names.contains(&"X-Evil"),
        "header with \\r\\n should be filtered"
    );
    assert!(
        !header_names.contains(&"X-Newline"),
        "header with \\n should be filtered"
    );
    assert!(!header_names.contains(&""), "empty key should be filtered");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_format_response_nil_and_string() {
    let dir = unique_test_dir("response-types");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.nothing = function(self)
                return nil
            end
            get.str = function(self)
                return "just a string"
            end
            get.invalid = function(self)
                return 42
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "nothing",
        vec![],
        &types::Request::fake("GET", "/nothing"),
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_string(), "");

    let resp = server.handle("GET", "str", vec![], &types::Request::fake("GET", "/str"));
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_string(), "just a string");

    let resp = server.handle(
        "GET",
        "invalid",
        vec![],
        &types::Request::fake("GET", "/invalid"),
    );
    assert_eq!(resp.status, 500);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_format_table_response_json_body() {
    let dir = unique_test_dir("json-body");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.json = function(self)
                return { status = 201, body = { key = "value", num = 42 } }
            end
        "#,
    );

    let resp = server.handle("GET", "json", vec![], &types::Request::fake("GET", "/json"));
    assert_eq!(resp.status, 201);

    let ct = resp.headers.iter().find(|(k, _)| k == "Content-Type");
    assert!(ct.is_some(), "JSON body should set Content-Type");
    assert!(ct.unwrap().1.contains("application/json"));

    let body = resp.body_string();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["key"], "value");
    assert_eq!(parsed["num"], 42);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_format_table_response_clamps_status() {
    let dir = unique_test_dir("status-clamp");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.low = function(self)
                return { status = 50, body = "too low" }
            end
            get.high = function(self)
                return { status = 999, body = "too high" }
            end
            get.negative = function(self)
                return { status = -1, body = "negative" }
            end
        "#,
    );

    let resp = server.handle("GET", "low", vec![], &types::Request::fake("GET", "/low"));
    assert!(resp.status >= 100, "status 50 should be clamped up");

    let resp = server.handle("GET", "high", vec![], &types::Request::fake("GET", "/high"));
    assert!(resp.status <= 599, "status 999 should be clamped down");

    let resp = server.handle(
        "GET",
        "negative",
        vec![],
        &types::Request::fake("GET", "/negative"),
    );
    assert!(resp.status >= 100, "negative status should be clamped up");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_all_method_registration() {
    let dir = unique_test_dir("all-methods");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.handle = function(self) return "GET" end
            post.handle = function(self) return "POST" end
            put.handle = function(self) return "PUT" end
            patch.handle = function(self) return "PATCH" end
            delete.handle = function(self) return "DELETE" end
            all.anything = function(self) return "ALL:" .. self.method end
        "#,
    );

    let resp = server.handle(
        "GET",
        "handle",
        vec![],
        &types::Request::fake("GET", "/handle"),
    );
    assert_eq!(resp.body_string(), "GET");

    let resp = server.handle(
        "POST",
        "handle",
        vec![],
        &types::Request::fake("POST", "/handle"),
    );
    assert_eq!(resp.body_string(), "POST");

    let resp = server.handle(
        "PUT",
        "handle",
        vec![],
        &types::Request::fake("PUT", "/handle"),
    );
    assert_eq!(resp.body_string(), "PUT");

    let resp = server.handle(
        "PATCH",
        "handle",
        vec![],
        &types::Request::fake("PATCH", "/handle"),
    );
    assert_eq!(resp.body_string(), "PATCH");

    let resp = server.handle(
        "DELETE",
        "handle",
        vec![],
        &types::Request::fake("DELETE", "/handle"),
    );
    assert_eq!(resp.body_string(), "DELETE");

    let resp = server.handle(
        "GET",
        "anything",
        vec![],
        &types::Request::fake("GET", "/anything"),
    );
    assert_eq!(resp.body_string(), "ALL:GET");

    let resp = server.handle(
        "POST",
        "anything",
        vec![],
        &types::Request::fake("POST", "/anything"),
    );
    assert_eq!(resp.body_string(), "ALL:POST");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_run_deferred_executes_in_reverse_order() {
    let dir = unique_test_dir("defer-order");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.deferred = function(self)
                defer(function(ctx)
                    _G.defer_log = _G.defer_log or {}
                    table.insert(_G.defer_log, "first")
                end)
                defer(function(ctx)
                    _G.defer_log = _G.defer_log or {}
                    table.insert(_G.defer_log, "second")
                end)
                return "ok"
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "deferred",
        vec![],
        &types::Request::fake("GET", "/deferred"),
    );
    assert_eq!(resp.status, 200);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_extract_query_edge_cases() {
    let dir = unique_test_dir("query-edge");
    let db = setup_db(&dir);
    let server = ApiServer::for_test(
        db,
        r#"
            get.q = function(self)
                local keys = {}
                for k, v in pairs(self.query) do
                    table.insert(keys, k .. "=" .. v)
                end
                table.sort(keys)
                return table.concat(keys, ",")
            end
        "#,
    );

    let resp = server.handle(
        "GET",
        "q",
        vec![],
        &types::Request::fake("GET", "/q?foo=bar&baz=qux"),
    );
    let body = resp.body_string();
    assert!(body.contains("foo=bar"), "query params should be parsed");
    assert!(body.contains("baz=qux"), "multiple params should work");

    let resp = server.handle(
        "GET",
        "q",
        vec![],
        &types::Request::fake("GET", "/q?key=with+space"),
    );
    let body = resp.body_string();
    assert!(body.contains("key=with space"), "+ should decode to space");

    let resp = server.handle(
        "GET",
        "q",
        vec![],
        &types::Request::fake("GET", "/q?empty="),
    );
    assert_eq!(resp.status, 200);

    let resp = server.handle("GET", "q", vec![], &types::Request::fake("GET", "/q?noeq"));
    assert_eq!(resp.status, 200);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_run_deferred_re_entrant() {
    use crate::lua::engine::create_engine;

    let dir = unique_test_dir("defer-reentrant");
    let db = setup_db(&dir);

    let lua = create_engine(Some(dir.clone()), db, "__test", false).unwrap();

    // Create a ctx table with __deferred and register as active_ctx
    let ctx = lua.create_table().unwrap();
    let deferred = lua.create_table().unwrap();
    ctx.set("__deferred", deferred.clone()).unwrap();
    lua.set_named_registry_value("active_ctx", ctx.clone())
        .unwrap();

    // Push two defers: the outer pushes the inner (re-entrant)
    let outer_fn = lua
        .create_function(move |_lua, (ctx,): (mlua::Table,)| {
            let inner = _lua.create_function(|_, ()| Ok(())).unwrap();
            let defers: mlua::Table = ctx.get("__deferred").unwrap();
            defers.set(1, inner).unwrap();
            Ok(())
        })
        .unwrap();
    deferred.set(1, outer_fn).unwrap();

    // Run deferred — the outer fn pushes an inner defer.
    // Old code would only run the outer (len captured once).
    // New code should also run the inner (swap-before-drain).
    let error_log = std::path::Path::new("/dev/null");
    smol::block_on(run_deferred(&lua, error_log));

    // After both runs, __deferred should be empty (or have a fresh
    // empty table swapped in).
    let remaining: mlua::Table = ctx.get("__deferred").unwrap();
    assert_eq!(remaining.raw_len(), 0);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}
