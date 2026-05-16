use super::*;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};
use crate::services::db::Db;
use indexmap::IndexMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_test_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("spyweb-{name}-{}-{nanos}", std::process::id()))
}

#[test]
fn hook_errors_use_hook_file_path_instead_of_rust_source() {
    let dir = unique_test_dir("hook-traceback");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function after_fetch(fetch_result)
    return fetch_result.response.status.code
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    let attempt = FetchAttempt {
        request: RequestConfig {
            url: "https://example.com".into(),
            headers: IndexMap::new(),
        },
        proxy: None,
        result: Ok(RequestResult {
            url: "https://example.com".into(),
            status: 200,
            headers: std::collections::HashMap::new(),
            body: "<html></html>".into(),
            proxy: None,
        }),
    };

    let err = smol::block_on(async { hooks.try_after_fetch(&attempt).await }).unwrap_err();
    let rendered = hooks.format_hook_error("after_fetch", err);

    assert!(rendered.contains("Lua after_fetch error for job 'test_job'"));
    assert!(rendered.contains(hook_path.to_string_lossy().as_ref()));
    assert!(rendered.contains("attempt to index"));
    assert!(rendered.contains("hooks.lua:"));
    assert!(!rendered.contains("src/lua/hooks.rs"));

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_override_fetch_hook() {
    let dir = unique_test_dir("override-fetch");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function override_fetch(request)
    if request.url == "https://fail.com" then
        return { error = "simulated failure" }
    end
    return {
        status = 200,
        body = "overridden body for " .. request.url,
        url = request.url
    }
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(hooks.has_override_fetch());

    let req = RequestConfig {
        url: "https://example.com".into(),
        headers: Default::default(),
    };

    // Test Success
    let attempt = smol::block_on(hooks.override_fetch(req.clone())).unwrap();
    let res = attempt.result.unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.body, "overridden body for https://example.com");

    // Test Lua-returned error
    let req_fail = RequestConfig {
        url: "https://fail.com".into(),
        headers: Default::default(),
    };
    let attempt_fail = smol::block_on(hooks.override_fetch(req_fail)).unwrap();
    assert_eq!(attempt_fail.result.unwrap_err(), "simulated failure");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_override_extract_hook() {
    let dir = unique_test_dir("override-extract");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function override_extract(response)
    return {
        { fields = { title = "Custom Item 1", body = response.body } },
        { fields = { title = "Custom Item 2", body = "Fixed body" } }
    }
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(hooks.has_override_extract());

    let res = RequestResult {
        url: "https://example.com".into(),
        status: 200,
        headers: Default::default(),
        body: "Raw HTML body".into(),
        proxy: None,
    };

    let items = smol::block_on(hooks.override_extract(&res)).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].fields["title"], "Custom Item 1");
    assert_eq!(items[0].fields["body"], "Raw HTML body");
    assert_eq!(items[1].fields["title"], "Custom Item 2");
    assert_eq!(items[1].fields["body"], "Fixed body");

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}
