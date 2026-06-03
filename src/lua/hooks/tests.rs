use super::*;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};
use crate::services::db::Db;
use indexmap::IndexMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn unique_test_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("spyweb-{name}-{}-{nanos}", std::process::id()))
}

#[test]
fn source_uses_cdp_detects_cdp_usage() {
    assert!(source_uses_cdp("local page = cdp.connect('ws://example')"));
    assert!(source_uses_cdp("return cdp.launch({})"));
    assert!(!source_uses_cdp("return http_get('https://example.com')"));
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
            method: "GET".into(),
            headers: Arc::new(IndexMap::new()),
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

    let err = smol::block_on(async {
        hooks.set_last_fetch(&attempt).await.unwrap();
        let fetch_table = {
            let lua = hooks.lua.lock().await;
            lua.globals().get("last_fetch").unwrap()
        };
        hooks.try_after_fetch(&attempt, fetch_table).await
    })
    .unwrap_err();
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
        method: "GET".into(),
        headers: Default::default(),
    };

    // Test Success
    let attempt = smol::block_on(hooks.override_fetch(req.clone())).unwrap();
    let res = attempt.result.unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.body, "overridden body for https://example.com");

    smol::block_on(async {
        let lua = hooks.lua.lock().await;
        assert!(matches!(
            lua.globals().get::<mlua::Value>("last_fetch").unwrap(),
            mlua::Value::Table(_)
        ));
    });

    // Test Lua-returned error
    let req_fail = RequestConfig {
        url: "https://fail.com".into(),
        method: "GET".into(),
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

#[test]
fn test_defer_binding() {
    let dir = unique_test_dir("defer-binding");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function before_fetch(request)
    _G.log = {}
    defer(function() table.insert(_G.log, "first") end)
    defer(function() table.insert(_G.log, "second") end)
    defer(function() 
        table.insert(_G.log, "third")
        error("simulated error in third") 
    end)
    defer(function() 
        table.insert(_G.log, "fourth")
        defer(function() table.insert(_G.log, "re-entrant") end)
    end)
    return request
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    let req = RequestConfig {
        url: "https://example.com".into(),
        method: "GET".into(),
        headers: Default::default(),
    };

    let _ = smol::block_on(hooks.before_fetch(req)).unwrap();

    smol::block_on(async {
        let lua = hooks.lua.lock().await;
        let log: Vec<String> = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("log")
            .unwrap();
        // LIFO: fourth -> third (errors, logs, continues) -> second -> first
        // Re-entrant: "re-entrant" is pushed during "fourth"'s execution,
        // but the current queue was swapped. It is drained in the next loop iteration.
        assert_eq!(
            log,
            vec!["fourth", "third", "second", "first", "re-entrant"]
        );
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_defer_lua_lifecycle_hooks_share_vm_state() {
    let dir = unique_test_dir("defer-lua-lifecycle");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    let defer_path = dir.join("defer.lua");

    fs::write(
        &hook_path,
        r#"
function before_fetch(request)
    _G.log = { "hook" }
    _G.shared_url = request.url
    return request
end
"#,
    )
    .unwrap();

    fs::write(
        &defer_path,
        r#"
function on_success()
    table.insert(_G.log, "success:" .. _G.shared_url)
end

function on_error(err)
    table.insert(_G.log, "error:" .. err)
end

function on_finally()
    table.insert(_G.log, "finally")
    defer(function() table.insert(_G.log, "deferred-from-finally") end)
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(hooks.has_on_success);
    assert!(hooks.has_on_error);
    assert!(hooks.has_on_finally);

    let req = RequestConfig {
        url: "https://example.com".into(),
        method: "GET".into(),
        headers: Default::default(),
    };

    let _ = smol::block_on(hooks.before_fetch(req)).unwrap();
    smol::block_on(async {
        hooks.run_on_success().await;
        hooks.run_on_finally().await;

        let lua = hooks.lua.lock().await;
        let log: Vec<String> = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("log")
            .unwrap();
        assert_eq!(log, vec!["hook", "success:https://example.com", "finally"]);
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_file(&defer_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_defer_lua_error_hook_and_cycle_cleanup() {
    let dir = unique_test_dir("defer-lua-error");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    let defer_path = dir.join("defer.lua");

    fs::write(&hook_path, "").unwrap();
    fs::write(
        &defer_path,
        r#"
_G.log = {}

function on_error(err)
    table.insert(_G.log, "error:" .. err)
end

function on_finally()
    table.insert(_G.log, "finally")
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        {
            let lua = hooks.lua.lock().await;
            lua.globals().set("last_fetch", "stale").unwrap();
            lua.globals().set("selector_matches", 3).unwrap();
            lua.globals()
                .set("__deferred", lua.create_table().unwrap())
                .unwrap();
        }

        hooks.run_on_error(&anyhow::anyhow!("boom")).await;
        hooks.run_on_finally().await;

        {
            let lua = hooks.lua.lock().await;
            let log: Vec<String> = lua
                .globals()
                .get::<mlua::Table>("_G")
                .unwrap()
                .get("log")
                .unwrap();
            assert_eq!(log, vec!["error:boom", "finally"]);
        }

        hooks.cleanup_cycle_state().await;

        let lua = hooks.lua.lock().await;
        assert!(matches!(
            lua.globals().get::<mlua::Value>("last_fetch").unwrap(),
            mlua::Value::Nil
        ));
        assert!(matches!(
            lua.globals()
                .get::<mlua::Value>("selector_matches")
                .unwrap(),
            mlua::Value::Nil
        ));
        assert!(matches!(
            lua.globals().get::<mlua::Value>("__deferred").unwrap(),
            mlua::Value::Nil
        ));
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_file(&defer_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_lifecycle_hooks_require_defer_lua_file() {
    let dir = unique_test_dir("defer-lua-required");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
_G.log = {}

function on_success()
    table.insert(_G.log, "should-not-run")
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(!hooks.has_on_success);

    smol::block_on(async {
        hooks.run_on_success().await;

        let lua = hooks.lua.lock().await;
        let log: Vec<String> = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("log")
            .unwrap();
        assert!(log.is_empty());
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn telemetry_table_records_active_stages_and_cleans_up() {
    let dir = unique_test_dir("telemetry-table");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(&hook_path, "").unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        hooks.init_telemetry().await.unwrap();
        let sample = hooks.telemetry_stage_start().await.unwrap();
        hooks
            .record_telemetry_stage("fetch", sample, "success", None)
            .await
            .unwrap();
        hooks
            .finalize_telemetry(Duration::from_millis(42))
            .await
            .unwrap();

        {
            let lua = hooks.lua.lock().await;
            let telemetry: mlua::Table = lua.globals().get("spyweb_telemetry").unwrap();
            let stages: mlua::Table = telemetry.get("stages").unwrap();
            let map: mlua::Table = telemetry.get("map").unwrap();

            assert_eq!(telemetry.get::<String>("job_name").unwrap(), "test_job");
            assert_eq!(stages.raw_len(), 14);
            assert_eq!(telemetry.get::<f64>("total_duration_ms").unwrap(), 42.0);

            let fetch: mlua::Table = map.get("fetch").unwrap();
            assert_eq!(fetch.get::<String>("status").unwrap(), "success");
            assert!(fetch.get::<f64>("duration_ms").unwrap() >= 0.0);
            assert!(fetch.get::<f64>("offset_ms").unwrap() >= 0.0);
            assert!(fetch.get::<i64>("lua_mem_bytes").unwrap() > 0);
            assert!(fetch.get::<i64>("browsers").unwrap() >= 0);

            let before_fetch: mlua::Table = map.get("before_fetch").unwrap();
            assert_eq!(before_fetch.get::<String>("status").unwrap(), "inactive");
            assert!(matches!(
                before_fetch.get::<mlua::Value>("duration_ms").unwrap(),
                mlua::Value::Nil
            ));
            assert!(matches!(
                before_fetch.get::<mlua::Value>("offset_ms").unwrap(),
                mlua::Value::Nil
            ));
        }

        hooks.cleanup_cycle_state().await;
        let lua = hooks.lua.lock().await;
        assert!(matches!(
            lua.globals()
                .get::<mlua::Value>("spyweb_telemetry")
                .unwrap(),
            mlua::Value::Nil
        ));
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}
