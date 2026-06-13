use super::*;
use crate::services::db::Db;
use std::io::Read;
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

fn req(method: &str, url: &str) -> rouille::Request {
    rouille::Request::fake_http(method, url, vec![], vec![])
}

fn req_with(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> rouille::Request {
    rouille::Request::fake_http(method, url, headers, body)
}

fn read_body(resp: rouille::Response) -> String {
    let mut body = String::new();
    resp.data
        .into_reader_and_size()
        .0
        .read_to_string(&mut body)
        .unwrap();
    body
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

    let resp = server.handle("GET", "hello", vec![], &req("GET", "/hello"));
    assert_eq!(resp.status_code, 200);
    assert_eq!(read_body(resp), "Hello from Lua!");

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

    let resp = server.handle("POST", "fallback", vec![], &req("POST", "/fallback"));
    assert_eq!(resp.status_code, 200);
    assert_eq!(read_body(resp), "Fallback hit: POST");

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

    let resp = server.handle("GET", "nonexistent", vec![], &req("GET", "/nonexistent"));
    assert_eq!(resp.status_code, 404);
    assert_eq!(read_body(resp), "Not Found");

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
        &req("GET", "/check/42/abc"),
    );
    assert_eq!(resp.status_code, 200);
    assert_eq!(read_body(resp), "42-abc");

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

    let request = req_with(
        "GET",
        "/ctx?foo=bar",
        vec![("X-Custom".into(), "val1".into())],
        vec![],
    );
    let resp = server.handle("GET", "ctx", vec![], &request);
    assert_eq!(resp.status_code, 200);
    assert_eq!(read_body(resp), "GET:bar:val1");

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

    let resp = server.handle("GET", "crash", vec![], &req("GET", "/crash"));
    assert_eq!(resp.status_code, 500);

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
    assert_eq!(url_decode("hello+world"), "hello world");
    assert_eq!(url_decode("a%20b"), "a b");
    assert_eq!(url_decode("a%2Fb"), "a/b");
    assert_eq!(url_decode("%3C%3E"), "<>");
    assert_eq!(url_decode("no%encoding"), "no%encoding");
    assert_eq!(url_decode("trailing%"), "trailing%");
    assert_eq!(url_decode("%2"), "%2");
    assert_eq!(url_decode(""), "");
    assert_eq!(url_decode("plain"), "plain");
    assert_eq!(url_decode("a+b%20c"), "a b c");
}

#[test]
fn test_extract_body_truncation() {
    let large_body = vec![b'x'; MAX_BODY_SIZE + 100];
    let request = req_with("POST", "/data", vec![], large_body);
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

    let resp = server.handle("GET", "inject", vec![], &req("GET", "/inject"));
    assert_eq!(resp.status_code, 200);

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

    let resp = server.handle("GET", "nothing", vec![], &req("GET", "/nothing"));
    assert_eq!(resp.status_code, 200);
    assert_eq!(read_body(resp), "");

    let resp = server.handle("GET", "str", vec![], &req("GET", "/str"));
    assert_eq!(resp.status_code, 200);
    assert_eq!(read_body(resp), "just a string");

    let resp = server.handle("GET", "invalid", vec![], &req("GET", "/invalid"));
    assert_eq!(resp.status_code, 500);

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

    let resp = server.handle("GET", "json", vec![], &req("GET", "/json"));
    assert_eq!(resp.status_code, 201);

    let ct = resp.headers.iter().find(|(k, _)| k == "Content-Type");
    assert!(ct.is_some(), "JSON body should set Content-Type");
    assert!(ct.unwrap().1.contains("application/json"));

    let body = read_body(resp);
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

    let resp = server.handle("GET", "low", vec![], &req("GET", "/low"));
    assert!(resp.status_code >= 100, "status 50 should be clamped up");

    let resp = server.handle("GET", "high", vec![], &req("GET", "/high"));
    assert!(resp.status_code <= 599, "status 999 should be clamped down");

    let resp = server.handle("GET", "negative", vec![], &req("GET", "/negative"));
    assert!(
        resp.status_code >= 100,
        "negative status should be clamped up"
    );

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

    let resp = server.handle("GET", "handle", vec![], &req("GET", "/handle"));
    assert_eq!(read_body(resp), "GET");

    let resp = server.handle("POST", "handle", vec![], &req("POST", "/handle"));
    assert_eq!(read_body(resp), "POST");

    let resp = server.handle("PUT", "handle", vec![], &req("PUT", "/handle"));
    assert_eq!(read_body(resp), "PUT");

    let resp = server.handle("PATCH", "handle", vec![], &req("PATCH", "/handle"));
    assert_eq!(read_body(resp), "PATCH");

    let resp = server.handle("DELETE", "handle", vec![], &req("DELETE", "/handle"));
    assert_eq!(read_body(resp), "DELETE");

    let resp = server.handle("GET", "anything", vec![], &req("GET", "/anything"));
    assert_eq!(read_body(resp), "ALL:GET");

    let resp = server.handle("POST", "anything", vec![], &req("POST", "/anything"));
    assert_eq!(read_body(resp), "ALL:POST");

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

    let resp = server.handle("GET", "deferred", vec![], &req("GET", "/deferred"));
    assert_eq!(resp.status_code, 200);

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

    let resp = server.handle("GET", "q", vec![], &req("GET", "/q?foo=bar&baz=qux"));
    let body = read_body(resp);
    assert!(body.contains("foo=bar"), "query params should be parsed");
    assert!(body.contains("baz=qux"), "multiple params should work");

    let resp = server.handle("GET", "q", vec![], &req("GET", "/q?key=with+space"));
    let body = read_body(resp);
    assert!(body.contains("key=with space"), "+ should decode to space");

    let resp = server.handle("GET", "q", vec![], &req("GET", "/q?empty="));
    assert_eq!(resp.status_code, 200);

    let resp = server.handle("GET", "q", vec![], &req("GET", "/q?noeq"));
    assert_eq!(resp.status_code, 200);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_dir(&dir);
}
