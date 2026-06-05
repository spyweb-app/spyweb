use super::*;
use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};
use crate::services::db::Db;
use indexmap::IndexMap;
use std::collections::HashMap;
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
        let ctx = hooks.new_cycle_context().await.unwrap();
        hooks.set_last_fetch(&ctx, &attempt).await.unwrap();
        let fetch_table = ctx.get("last_fetch").unwrap();
        hooks.try_after_fetch(&attempt, fetch_table, &ctx).await
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
    let ctx = smol::block_on(hooks.new_cycle_context()).unwrap();
    let attempt = smol::block_on(hooks.override_fetch(req.clone(), &ctx)).unwrap();
    let res = attempt.result.unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.body, "overridden body for https://example.com");

    smol::block_on(async {
        assert!(matches!(
            ctx.get::<mlua::Value>("last_fetch").unwrap(),
            mlua::Value::Table(_)
        ));
    });

    // Test Lua-returned error
    let req_fail = RequestConfig {
        url: "https://fail.com".into(),
        method: "GET".into(),
        headers: Default::default(),
    };
    let attempt_fail = smol::block_on(hooks.override_fetch(req_fail, &ctx)).unwrap();
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

    let ctx = smol::block_on(hooks.new_cycle_context()).unwrap();
    let items = smol::block_on(hooks.override_extract(&res, &ctx)).unwrap();
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

    let ctx = smol::block_on(hooks.new_cycle_context()).unwrap();
    let _ = smol::block_on(hooks.before_fetch(req, &ctx)).unwrap();

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
    assert!(hooks.hook_mask & HOOK_ON_SUCCESS != 0);
    assert!(hooks.hook_mask & HOOK_ON_ERROR != 0);
    assert!(hooks.hook_mask & HOOK_ON_FINALLY != 0);

    let req = RequestConfig {
        url: "https://example.com".into(),
        method: "GET".into(),
        headers: Default::default(),
    };

    let ctx = smol::block_on(hooks.new_cycle_context()).unwrap();
    let _ = smol::block_on(hooks.before_fetch(req, &ctx)).unwrap();
    smol::block_on(async {
        hooks.run_on_success(&ctx).await;
        hooks.run_on_finally(&ctx).await;

        let lua = hooks.lua.lock().await;
        let log: Vec<String> = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("log")
            .unwrap();
        assert_eq!(
            log,
            vec![
                "hook",
                "success:https://example.com",
                "finally",
                "deferred-from-finally"
            ]
        );
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
        let ctx = hooks.new_cycle_context().await.unwrap();
        let store = JobHooks::ctx_store(&ctx).unwrap();
        store.raw_set("last_fetch", "stale").unwrap();
        store.raw_set("selector_matches", 3).unwrap();
        let fresh = hooks.lua.lock().await.create_table().unwrap();
        store.raw_set("__deferred", fresh).unwrap();

        hooks.run_on_error(&ctx, &anyhow::anyhow!("boom")).await;
        hooks.run_on_finally(&ctx).await;

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

        hooks.cleanup_cycle_state(&ctx).await;

        assert!(matches!(
            ctx.get::<mlua::Value>("last_fetch").unwrap(),
            mlua::Value::Nil
        ));
        assert!(matches!(
            ctx.get::<mlua::Value>("selector_matches").unwrap(),
            mlua::Value::Nil
        ));
        assert!(matches!(
            ctx.get::<mlua::Value>("__deferred").unwrap(),
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
    assert!(hooks.hook_mask & HOOK_ON_SUCCESS == 0);

    smol::block_on(async {
        let ctx = hooks.new_cycle_context().await.unwrap();
        hooks.run_on_success(&ctx).await;

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
        let ctx = hooks.new_cycle_context().await.unwrap();
        hooks.init_telemetry(&ctx).await.unwrap();
        let sample = hooks.telemetry_stage_start(&ctx).await.unwrap();
        hooks
            .record_telemetry_stage(&ctx, "fetch", sample, "success", None)
            .await
            .unwrap();
        hooks
            .finalize_telemetry(&ctx, Duration::from_millis(42))
            .await
            .unwrap();

        {
            let telemetry: mlua::Table = ctx.get("telemetry").unwrap();
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

        hooks.cleanup_cycle_state(&ctx).await;
        assert!(matches!(
            ctx.get::<mlua::Value>("telemetry").unwrap(),
            mlua::Value::Nil
        ));
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_lifecycle_hooks_context_and_defer() {
    let dir = unique_test_dir("lifecycle-context-defer");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    let defer_path = dir.join("defer.lua");

    fs::write(&hook_path, "").unwrap();
    fs::write(
        &defer_path,
        r#"
function on_success(ctx)
    -- Verify context is passed as an argument
    if type(ctx) == "table" then
        _G.ctx_passed = true
    end
    
    -- Verify defer() works (requires active_ctx in registry)
    defer(function() 
        _G.deferred_ran = true 
    end)
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        let ctx = hooks.new_cycle_context().await.unwrap();

        // Execute the lifecycle hook
        hooks.run_on_success(&ctx).await;

        let lua = hooks.lua.lock().await;
        let globals = lua.globals();

        // 1. Coverage for Context Argument
        assert!(
            globals.get::<bool>("ctx_passed").unwrap_or(false),
            "Lifecycle hook 'on_success' must receive 'ctx' as its first argument"
        );

        // 2. Coverage for Deferred Tasks in Lifecycle
        assert!(
            globals.get::<bool>("deferred_ran").unwrap_or(false),
            "Deferred tasks registered in 'on_success' must be drained before completion"
        );
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_file(&defer_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_context_reserved_keys_protection() {
    let dir = unique_test_dir("context-protection");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function before_fetch(request, ctx)
    local ok, err = pcall(function()
        ctx.worker_id = 999
    end)
    _G.protect_worker_id = not ok
    
    ok, err = pcall(function()
        ctx.telemetry = "hacked"
    end)
    _G.protect_telemetry = not ok

    -- User keys should be allowed
    ctx.my_custom_key = "safe"
    return request
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        let ctx = hooks.new_cycle_context().await.unwrap();
        let req = RequestConfig {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: Default::default(),
        };

        let _ = hooks.before_fetch(req, &ctx).await.unwrap();

        let lua = hooks.lua.lock().await;
        let globals = lua.globals();

        assert!(
            globals.get::<bool>("protect_worker_id").unwrap(),
            "Should protect worker_id"
        );
        assert!(
            globals.get::<bool>("protect_telemetry").unwrap(),
            "Should protect telemetry"
        );
        assert_eq!(
            ctx.get::<String>("my_custom_key").unwrap(),
            "safe",
            "User keys should be allowed"
        );
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_context_persistence_across_stages() {
    let dir = unique_test_dir("context-persistence");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function before_fetch(request, ctx)
    ctx.step = "started"
    return request
end

function filter_item(item, ctx)
    ctx.step = ctx.step .. "->filtering"
    return item
end

function before_store(items, ctx)
    ctx.step = ctx.step .. "->storing"
    return items
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        let ctx = hooks.new_cycle_context().await.unwrap();
        let req = RequestConfig {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: Default::default(),
        };

        // 1. Before Fetch
        let _ = hooks.before_fetch(req, &ctx).await.unwrap();

        // 2. Filter Item
        let item = ExtractedItem {
            fields: HashMap::new(),
            ..Default::default()
        };
        let _ = hooks.filter_item(item, &ctx).await.unwrap();

        // 3. Before Store
        let _ = hooks.before_store(vec![], &ctx).await.unwrap();

        assert_eq!(
            ctx.get::<String>("step").unwrap(),
            "started->filtering->storing"
        );
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}
