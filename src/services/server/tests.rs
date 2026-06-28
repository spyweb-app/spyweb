use super::*;
use crate::services::db::Db;
use std::sync::Arc;

fn test_db() -> Arc<Db> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    Arc::new(Db::open(path.to_str().unwrap()).unwrap())
}

#[test]
fn test_validate_static_path_clean() {
    assert!(validate_static_path("index.html").is_ok());
    assert!(validate_static_path("app.js").is_ok());
    assert!(validate_static_path("style.css").is_ok());
    assert!(validate_static_path("logo.png").is_ok());
    assert!(validate_static_path("font.woff2").is_ok());
    assert!(validate_static_path("data.json").is_ok());
    assert!(validate_static_path("icon.svg").is_ok());
}

#[test]
fn test_validate_static_path_blocks_traversal() {
    assert!(validate_static_path("../etc/passwd").is_err());
    assert!(validate_static_path("ui/../../etc/passwd").is_err());
    assert!(validate_static_path("a/../b/file.html").is_err());
}

#[test]
fn test_validate_static_path_blocks_bad_extensions() {
    assert!(validate_static_path("script.sh").is_err());
    assert!(validate_static_path("binary.exe").is_err());
    assert!(validate_static_path("data.csv").is_err());
    assert!(validate_static_path("file.lua").is_err());
    assert!(validate_static_path("noext").is_err());
}

#[test]
fn test_constant_time_eq_equal() {
    assert!(constant_time_eq(b"hello", b"hello"));
    assert!(constant_time_eq(b"", b""));
}

#[test]
fn test_constant_time_eq_different() {
    assert!(!constant_time_eq(b"hello", b"world"));
    assert!(!constant_time_eq(b"abc", b"abcd"));
    assert!(!constant_time_eq(b"abcd", b"abc"));
    assert!(!constant_time_eq(b"", b"a"));
}

#[test]
fn test_records_query_defaults() {
    let request = types::Request::fake("GET", "/api/records");
    let query = RecordsQuery::from_request(&request);
    assert_eq!(query.limit, 30);
    assert!(query.job_id.is_none());
    assert!(query.after.is_none());
}

#[test]
fn test_records_query_custom_params() {
    let request = types::Request::fake("GET", "/api/records?job_id=abc&limit=50&after=12345");
    let query = RecordsQuery::from_request(&request);
    assert_eq!(query.job_id.as_deref(), Some("abc"));
    assert_eq!(query.limit, 50);
    assert_eq!(query.after, Some(12345));
}

#[test]
fn test_records_query_limit_capped() {
    let request = types::Request::fake("GET", "/api/records?limit=99999");
    let query = RecordsQuery::from_request(&request);
    assert_eq!(query.limit, 1000, "limit should be capped at 1000");
}

#[test]
fn test_records_query_invalid_limit_falls_back() {
    let request = types::Request::fake("GET", "/api/records?limit=notanumber");
    let query = RecordsQuery::from_request(&request);
    assert_eq!(query.limit, 30, "invalid limit should default to 30");
}

#[test]
fn test_handle_records_request_empty() {
    let db = test_db();
    let request = types::Request::fake("GET", "/api/records");
    let resp = handle_records_request(&request, &db).unwrap();
    assert_eq!(resp.status, 200);
}

#[test]
fn test_handle_records_request_with_job_id() {
    let db = test_db();
    let request = types::Request::fake("GET", "/api/records?job_id=nonexistent");
    let resp = handle_records_request(&request, &db).unwrap();
    assert_eq!(resp.status, 200);
}

#[test]
fn test_handle_api_request_jobs() {
    let db = test_db();
    let jobs = Arc::new(std::sync::RwLock::new(vec![JobSummary {
        id: "j1".into(),
        name: "Job1".into(),
        enabled: true,
    }]));
    let request = types::Request::fake("GET", "/api/jobs");
    let resp = handle_api_request(&db, &jobs, &request).unwrap();
    assert_eq!(resp.status, 200);
}

#[test]
fn test_handle_api_request_404() {
    let db = test_db();
    let jobs = Arc::new(std::sync::RwLock::new(vec![]));
    let request = types::Request::fake("GET", "/api/nonexistent");
    let resp = handle_api_request(&db, &jobs, &request).unwrap();
    assert_eq!(resp.status, 404);
}

#[test]
fn test_handle_api_request_v_lua_server_missing_init() {
    let db = test_db();
    let jobs = Arc::new(std::sync::RwLock::new(vec![]));
    let request = types::Request::fake("GET", "/api/v/missing");
    let resp = handle_api_request(&db, &jobs, &request).unwrap();
    assert_eq!(resp.status, 500, "missing init.lua should return 500");
}

#[test]
fn test_handle_static_request_root() {
    let db = test_db();
    let request = types::Request::fake("GET", "/");
    let resp = handle_static_request(&db, &request);
    assert!(
        resp.status == 200 || resp.status == 404,
        "root should return 200 or 404"
    );
}

#[test]
fn test_handle_static_request_blocks_traversal() {
    let db = test_db();
    let request = types::Request::fake("GET", "/../etc/passwd");
    let resp = handle_static_request(&db, &request);
    assert_eq!(resp.status, 404, "traversal should return 404");
}

#[test]
fn test_handle_static_request_blocks_bad_extension() {
    let db = test_db();
    let request = types::Request::fake("GET", "/script.sh");
    let resp = handle_static_request(&db, &request);
    assert_eq!(resp.status, 404, "bad extension should return 404");
}

#[test]
fn test_handle_static_request_records_route() {
    let db = test_db();
    let request = types::Request::fake("GET", "/records");
    let resp = handle_static_request(&db, &request);
    assert!(
        resp.status == 200 || resp.status == 404,
        "/records should return 200 or 404"
    );
}

#[test]
fn test_handle_html_records_empty() {
    let db = test_db();
    let html = handle_html_records(&db).unwrap();
    assert!(
        html.contains("No records found."),
        "empty db should show no records message"
    );
    assert!(html.contains("<!DOCTYPE html>"), "should be valid HTML");
}

#[test]
fn test_auth_no_key_allows_all() {
    let db = test_db();
    let jobs = Arc::new(std::sync::RwLock::new(vec![]));
    let request = types::Request::fake("GET", "/api/jobs");
    let resp = handle_api_request(&db, &jobs, &request).unwrap();
    assert_eq!(resp.status, 200, "no auth key configured should allow all");
}

#[test]
fn test_handle_records_request_with_query_string() {
    let db = test_db();
    let request = types::Request::fake("GET", "/api/records?limit=10");
    let resp =
        handle_api_request(&db, &Arc::new(std::sync::RwLock::new(vec![])), &request).unwrap();
    assert_eq!(
        resp.status, 200,
        "/api/records with query string should not 404"
    );
}

#[test]
fn test_handle_jobs_request_with_query_string() {
    let db = test_db();
    let jobs = Arc::new(std::sync::RwLock::new(vec![JobSummary {
        id: "j1".into(),
        name: "Job1".into(),
        enabled: true,
    }]));
    let request = types::Request::fake("GET", "/api/jobs?foo=bar");
    let resp = handle_api_request(&db, &jobs, &request).unwrap();
    assert_eq!(
        resp.status, 200,
        "/api/jobs with query string should not 404"
    );
}

#[test]
fn test_handle_api_v_request_with_query_string() {
    let db = test_db();
    let jobs = Arc::new(std::sync::RwLock::new(vec![]));
    let request = types::Request::fake("GET", "/api/v/missing?debug=1");
    let resp = handle_api_request(&db, &jobs, &request).unwrap();
    assert_eq!(
        resp.status, 500,
        "/api/v/ with query string should still route to Lua server"
    );
}
