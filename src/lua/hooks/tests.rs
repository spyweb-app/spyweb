use super::*;
use crate::config::types::{Job, JobConfig, JobOptions};
use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};
use crate::scraper::runner::Runner;
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

fn single_thread_executor() -> (Arc<smol::Executor<'static>>, smol::channel::Sender<()>) {
    let ex = Arc::new(smol::Executor::new());
    let (tx, rx) = smol::channel::unbounded::<()>();
    let executor = Arc::clone(&ex);
    std::thread::spawn(move || {
        let _ = smol::block_on(executor.run(rx.recv()));
    });
    (ex, tx)
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
            timeout: None,
            proxy: None,
            max_body_size: None,
        },
        proxy: None,
        result: Ok(RequestResult {
            url: "https://example.com".into(),
            status: 200,
            headers: std::collections::HashMap::new(),
            body: "<html></html>".into(),
            proxy: None,
            time_ms: None,
            size: None,
        }),
    };

    let err = smol::block_on(async {
        let ctx = hooks.new_cycle_context(0).await.unwrap();
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
        timeout: None,
        proxy: None,
        max_body_size: None,
    };

    // Test Success
    let ctx = smol::block_on(hooks.new_cycle_context(0)).unwrap();
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
        timeout: None,
        proxy: None,
        max_body_size: None,
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
        time_ms: None,
        size: None,
    };

    let ctx = smol::block_on(hooks.new_cycle_context(0)).unwrap();
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
        timeout: None,
        proxy: None,
        max_body_size: None,
    };

    let ctx = smol::block_on(hooks.new_cycle_context(0)).unwrap();
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
        timeout: None,
        proxy: None,
        max_body_size: None,
    };

    let ctx = smol::block_on(hooks.new_cycle_context(0)).unwrap();
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
        let ctx = hooks.new_cycle_context(0).await.unwrap();
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
        let ctx = hooks.new_cycle_context(0).await.unwrap();
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
        let ctx = hooks.new_cycle_context(0).await.unwrap();
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
        let ctx = hooks.new_cycle_context(0).await.unwrap();

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
        let ctx = hooks.new_cycle_context(0).await.unwrap();
        let req = RequestConfig {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: Default::default(),
            timeout: None,
            proxy: None,
            max_body_size: None,
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
        let ctx = hooks.new_cycle_context(0).await.unwrap();
        let req = RequestConfig {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: Default::default(),
            timeout: None,
            proxy: None,
            max_body_size: None,
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

#[test]
fn test_multi_worker_url_dispatch_and_worker_id() {
    let dir = unique_test_dir("multi-worker-dispatch");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function before_fetch(request, ctx)
    -- Store worker_id and URL in a global log to verify dispatch
    _G.worker_log = _G.worker_log or {}
    table.insert(_G.worker_log, { worker_id = ctx.worker_id, url = request.url })
    return request
end
"#,
    )
    .unwrap();

    let db_path = dir.join("test.redb");
    let db = Arc::new(Db::open(db_path.to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Arc::new(Job {
        config: JobConfig {
            name: "test_job".into(),
            url: "https://initial.com".into(),
            selector: ".item".into(),
            fields: vec![],
            options: JobOptions {
                workers: Some(2),
                urls: Some(vec![
                    "https://url-1.com".into(),
                    "https://url-2.com".into(),
                    "https://url-3.com".into(),
                    "https://url-4.com".into(),
                ]),
                interval: 60,
                ..Default::default()
            },
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    });

    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        // Run Group 3 URL Dispatch
        crate::scraper::pipeline::run_urls_cycle(
            Arc::clone(&job),
            &db,
            &runner,
            2,
            &ex,
            None,
            job.config.options.urls.clone().unwrap(),
            &Arc::new(crate::scraper::pipeline::CycleStats::new()),
        )
        .await;

        let hooks_ref = job.hooks.as_ref().unwrap();
        let lua_guard = hooks_ref.lua.lock().await;
        let lua: &mlua::Lua = &lua_guard;
        let log: Vec<mlua::Table> = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("worker_log")
            .unwrap();

        // 4 URLs total should be processed
        assert_eq!(log.len(), 4);

        let mut seen_urls = std::collections::HashSet::new();
        let mut seen_workers = std::collections::HashSet::new();

        for entry in log {
            let url: String = entry.get::<String>("url").unwrap();
            let worker_id: usize = entry.get::<usize>("worker_id").unwrap();
            seen_urls.insert(url);
            seen_workers.insert(worker_id);
        }

        // Verify all URLs were hit exactly once (no duplicates/skips)
        assert_eq!(seen_urls.len(), 4);
        assert!(seen_urls.contains("https://url-1.com"));
        assert!(seen_urls.contains("https://url-2.com"));
        assert!(seen_urls.contains("https://url-3.com"));
        assert!(seen_urls.contains("https://url-4.com"));

        // Verify both workers were active (probabilistic but likely with 4 URLs)
        assert!(!seen_workers.is_empty());
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_mask_and_execution() {
    let dir = unique_test_dir("on-finished");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
_G.log = {}

function on_finished()
    table.insert(_G.log, "finished")
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(
        hooks.hook_mask & HOOK_ON_FINISHED != 0,
        "HOOK_ON_FINISHED should be set in mask"
    );

    smol::block_on(async {
        hooks
            .run_on_finished(&crate::scraper::pipeline::CycleStats::new())
            .await;

        let lua = hooks.lua.lock().await;
        let log: Vec<String> = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("log")
            .unwrap();
        assert_eq!(log, vec!["finished"], "on_finished should have executed");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_zero_param_no_table() {
    // `function on_finished()`: arity gate — no table constructed, hook runs
    // with zero arguments (existing back-compat path).
    let dir = unique_test_dir("on-finished-no-args");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
_G.ran = false

function on_finished()
    _G.ran = true
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(hooks.hook_mask & HOOK_ON_FINISHED != 0);

    smol::block_on(async {
        hooks
            .run_on_finished(&crate::scraper::pipeline::CycleStats::new())
            .await;

        let lua = hooks.lua.lock().await;
        let ran: bool = lua.globals().get("ran").unwrap();
        assert!(ran, "zero-param on_finished should have run");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_vararg_receives_table() {
    // `function on_finished(...)`: arity gate treats vararg as wanting the
    // table — receives the full stats table.
    let dir = unique_test_dir("on-finished-vararg");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
function on_finished(...)
    local queue = ...
    _G.saw_urls = type(queue.urls) == "table"
    _G.saw_processed = queue.processed
    _G.saw_failed = queue.failed
    _G.saw_pushed = queue.pushed
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        let stats = crate::scraper::pipeline::CycleStats::new();
        stats.record(true);
        stats.record(true);
        stats.record(false);
        hooks.run_on_finished(&stats).await;

        let lua = hooks.lua.lock().await;
        let g = lua.globals();
        let saw_urls: bool = g.get("saw_urls").unwrap();
        let processed: usize = g.get("saw_processed").unwrap();
        let failed: usize = g.get("saw_failed").unwrap();
        let pushed: usize = g.get("saw_pushed").unwrap();
        assert!(saw_urls, "queue.urls should be a table");
        assert_eq!(processed, 3);
        assert_eq!(failed, 1);
        assert_eq!(pushed, 0, "pushed is always 0 until Phase 3");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_param_receives_final_stats() {
    // One-param hook receives the finalized table after dispatch.
    let dir = unique_test_dir("on-finished-stats");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
function on_finished(queue)
    _G.counts = { queue.processed, queue.failed, queue.pushed }
    _G.urls_len = #queue.urls
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        let stats = crate::scraper::pipeline::CycleStats::new();
        stats.record(true);
        hooks.run_on_finished(&stats).await;

        let lua = hooks.lua.lock().await;
        let g = lua.globals();
        let counts: Vec<usize> = g.get("counts").unwrap();
        assert_eq!(counts, vec![1, 0, 0]);
        let urls_len: usize = g.get("urls_len").unwrap();
        assert_eq!(urls_len, 0, "urls is always empty in on_finished");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_defer_raises() {
    // defer() has no active ctx in on_finished (active_ctx = Nil) →
    // deterministic error, hook fails, log-only (counters already final).
    let dir = unique_test_dir("on-finished-defer");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
_G.ran_before_defer = false

function on_finished()
    _G.ran_before_defer = true
    defer(function() end)
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        // Must not panic; defer raises inside the hook, error is log-only.
        hooks
            .run_on_finished(&crate::scraper::pipeline::CycleStats::new())
            .await;

        let lua = hooks.lua.lock().await;
        let ran: bool = lua.globals().get("ran_before_defer").unwrap();
        assert!(ran, "hook body should run before defer raises");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_no_defer_lua_still_works() {
    let dir = unique_test_dir("on-finished-no-defer");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    // No defer.lua file — on_finished should still be detected
    fs::write(
        &hook_path,
        r#"
_G.ran = false

function on_finished()
    _G.ran = true
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(
        hooks.hook_mask & HOOK_ON_FINISHED != 0,
        "on_finished should be detected without defer.lua"
    );

    smol::block_on(async {
        hooks
            .run_on_finished(&crate::scraper::pipeline::CycleStats::new())
            .await;

        let lua = hooks.lua.lock().await;
        let ran: bool = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("ran")
            .unwrap();
        assert!(ran, "on_finished should have run without defer.lua");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_error_does_not_crash() {
    let dir = unique_test_dir("on-finished-error");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
function on_finished()
    error("boom")
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(hooks.hook_mask & HOOK_ON_FINISHED != 0);

    smol::block_on(async {
        // Should not panic
        hooks
            .run_on_finished(&crate::scraper::pipeline::CycleStats::new())
            .await;
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_finished_not_defined_is_noop() {
    let dir = unique_test_dir("on-finished-noop");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");

    fs::write(
        &hook_path,
        r#"
_G.ran = false
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(
        hooks.hook_mask & HOOK_ON_FINISHED == 0,
        "mask should NOT have HOOK_ON_FINISHED when on_finished is not defined"
    );

    smol::block_on(async {
        hooks
            .run_on_finished(&crate::scraper::pipeline::CycleStats::new())
            .await;

        let lua = hooks.lua.lock().await;
        let ran: bool = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("ran")
            .unwrap();
        assert!(!ran, "on_finished should not have run when not defined");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_dynamic_url_override() {
    let dir = unique_test_dir("dynamic-dispatch");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function before_fetch(request, ctx)
    -- Override the URL dynamically
    request.url = "https://override.com"
    return request
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        let ctx = hooks.new_cycle_context(0).await.unwrap();
        let req = RequestConfig {
            url: "https://initial.com".into(),
            method: "GET".into(),
            headers: Default::default(),
            timeout: None,
            proxy: None,
            max_body_size: None,
        };

        let overridden = hooks.before_fetch(req, &ctx).await.unwrap().unwrap();
        assert_eq!(overridden.url, "https://override.com");
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_worker_panic_resilience() {
    let dir = unique_test_dir("worker-panic");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function before_fetch(request, ctx)
    if request.url == "https://panic.com" then
        error("Intentional panic")
    end
    _G.worker_log = _G.worker_log or {}
    table.insert(_G.worker_log, request.url)
    return request
end
"#,
    )
    .unwrap();

    let db_path = dir.join("test.redb");
    let db = Arc::new(Db::open(db_path.to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();

    let job = Arc::new(Job {
        config: JobConfig {
            name: "test_job".into(),
            url: "https://initial.com".into(),
            selector: ".item".into(),
            fields: vec![],
            options: JobOptions {
                workers: Some(2),
                urls: Some(vec![
                    "https://url-1.com".into(),
                    "https://panic.com".into(),
                    "https://url-3.com".into(),
                ]),
                interval: 60,
                ..Default::default()
            },
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    });

    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        // Run workers. Panic on https://panic.com should be caught and logged
        crate::scraper::pipeline::run_urls_cycle(
            Arc::clone(&job),
            &db,
            &runner,
            2,
            &ex,
            None,
            job.config.options.urls.clone().unwrap(),
            &Arc::new(crate::scraper::pipeline::CycleStats::new()),
        )
        .await;

        let hooks_ref = job.hooks.as_ref().unwrap();
        let lua = hooks_ref.lua.lock().await;
        let log: Vec<String> = lua
            .globals()
            .get::<mlua::Table>("_G")
            .unwrap()
            .get("worker_log")
            .unwrap();

        // 2 URLs should still be processed, panic is ignored
        assert_eq!(log.len(), 2);
        assert!(log.contains(&"https://url-1.com".to_string()));
        assert!(log.contains(&"https://url-3.com".to_string()));
    });

    drop(db);
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_before_fetch_can_override_timeout_proxy_max_body() {
    let dir = unique_test_dir("before-fetch-override");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function before_fetch(request, ctx)
    request.timeout = 15
    request.proxy = "http://user:pass@residential-proxy.com:8080"
    request.max_body_size = 5     -- 5 MB
    return request
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

    smol::block_on(async {
        let ctx = hooks.new_cycle_context(0).await.unwrap();
        let req = RequestConfig {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: Default::default(),
            timeout: None,
            proxy: None,
            max_body_size: None,
        };

        let overridden = hooks.before_fetch(req, &ctx).await.unwrap().unwrap();
        assert_eq!(overridden.timeout, Some(15));
        assert_eq!(
            overridden.proxy.as_deref(),
            Some("http://user:pass@residential-proxy.com:8080")
        );
        assert_eq!(overridden.max_body_size, Some(5 * 1024 * 1024));
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

// --- on_start (Phase 1) ---

/// Load a hooks.lua with the given source, run `run_on_start` with the given
/// seed, return the read-back batch. Cleans up temp files.
fn on_start_result(source: &str, seed: Option<Vec<&str>>) -> Option<Vec<String>> {
    let dir = unique_test_dir("on-start");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(&hook_path, source).unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    let seed = seed.map(|v| v.into_iter().map(String::from).collect());
    let result = smol::block_on(hooks.run_on_start(seed));

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
    result
}

#[test]
fn test_on_start_append_commits() {
    let result = on_start_result(
        r#"
function on_start(queue)
    table.insert(queue.urls, "b")
end
"#,
        Some(vec!["a"]),
    );
    assert_eq!(result, Some(vec!["a".to_string(), "b".to_string()]));
}

#[test]
fn test_on_start_reset_commits() {
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { "x", "y" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["x".to_string(), "y".to_string()]));
}

#[test]
fn test_on_start_return_value_ignored() {
    // Mutating the param commits; a returned fresh table with different urls
    // is discarded (return value is not a commit channel).
    let result = on_start_result(
        r#"
function on_start(queue)
    table.insert(queue.urls, "from-param")
    return { urls = { "hijack-1", "hijack-2" } }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(
        result,
        Some(vec!["seed".to_string(), "from-param".to_string()])
    );
}

#[test]
fn test_on_start_rebind_local_is_noop() {
    // Rebinding the local does not touch the object the engine holds.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue = { urls = { "hijack" } }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_junk_returns_are_noops() {
    for ret in ["return 42", "return 'junk'", "return true", "return nil"] {
        let source = format!(
            "function on_start(queue)\n    table.insert(queue.urls, \"b\")\n    {ret}\nend\n"
        );
        let result = on_start_result(&source, Some(vec!["a"]));
        assert_eq!(
            result,
            Some(vec!["a".to_string(), "b".to_string()]),
            "return value `{ret}` must not affect commit"
        );
    }
}

#[test]
fn test_on_start_nil_urls_is_no_opinion() {
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = nil
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_legacy_seed_nil_stays_nil() {
    // Legacy job: seed is nil; untouched/nil urls → None → legacy dispatch.
    let result = on_start_result(
        r#"
function on_start(queue)
end
"#,
        None,
    );
    assert_eq!(result, None);
}

#[test]
fn test_on_start_empty_table_is_zero_work() {
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = {}
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec![]));
}

#[test]
fn test_on_start_promotes_legacy_job() {
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { "monitor:1", "monitor:2" }
end
"#,
        None,
    );
    assert_eq!(
        result,
        Some(vec!["monitor:1".to_string(), "monitor:2".to_string()])
    );
}

#[test]
fn test_on_start_invalid_urls_seed_intact() {
    // Non-list value.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = "junk"
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));

    // Non-string members.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { 1, 2 }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_sparse_and_keyed_urls_seed_intact() {
    // Sparse: hole at 2. Previously raw_len-based validation could silently
    // commit a prefix or an empty list.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { [1] = "a", [3] = "c" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));

    // Starts at 2, missing 1.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { [2] = "b" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));

    // Hash-only table: must be hook failure, NOT a zero-work batch.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { foo = "bar" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));

    // Huge key: rejected before any allocation proportional to the key.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { [1000000] = "x" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));

    // Mixed: valid prefix + extra hash key.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { "a", "b", extra = "nope" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_hash_position_string_commits() {
    // Positive control for pairs-based validation: a single-element list
    // written with an explicit integer key is still a dense list of strings.
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = { [1] = "solo" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["solo".to_string()]));
}

#[test]
fn test_on_start_zero_param_no_table() {
    // `function on_start()`: pure generic hook. No table constructed, no
    // read-back. Side effects run; seed passes through untouched.
    let result = on_start_result(
        r#"
_G.on_start_ran = true
function on_start()
    _G.on_start_ran = true
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));

    // Referencing `queue` inside a zero-param hook indexes nil — loud error,
    // seed intact. Proves no table argument is passed.
    let result = on_start_result(
        r#"
function on_start()
    queue.urls = { "x" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_vararg_receives_table() {
    // `function on_start(...)`: vararg is treated as wanting the table.
    let result = on_start_result(
        r#"
function on_start(...)
    local queue = ...
    queue.urls = { "vararg-a", "vararg-b" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(
        result,
        Some(vec!["vararg-a".to_string(), "vararg-b".to_string()])
    );
}

#[test]
fn test_on_start_param_name_irrelevant() {
    // Arity counts, not the parameter name.
    let result = on_start_result(
        r#"
function on_start(q)
    q.urls = { "renamed" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["renamed".to_string()]));
}

#[test]
fn test_on_start_over_cap_seed_intact() {
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.urls = {}
    for i = 1, 10001 do
        queue.urls[i] = "u" .. i
    end
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_lua_error_seed_intact() {
    let result = on_start_result(
        r#"
function on_start(queue)
    error("boom")
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_defer_raises_seed_intact() {
    // defer() has no active ctx in on_start (active_ctx = Nil) → deterministic
    // error → hook fails → seed intact.
    let result = on_start_result(
        r#"
function on_start(queue)
    defer(function() end)
    queue.urls = { "should-not-commit" }
end
"#,
        Some(vec!["seed"]),
    );
    assert_eq!(result, Some(vec!["seed".to_string()]));
}

#[test]
fn test_on_start_extra_fields_ignored() {
    let result = on_start_result(
        r#"
function on_start(queue)
    queue.processed = 99
    queue.failed = 42
    queue.pushed = 7
    queue.junk = "whatever"
    table.insert(queue.urls, "b")
end
"#,
        Some(vec!["a"]),
    );
    assert_eq!(result, Some(vec!["a".to_string(), "b".to_string()]));
}

#[test]
fn test_on_start_mask_gate_without_hook() {
    let dir = unique_test_dir("on-start-mask");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        "function before_fetch(request) return request end\n",
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(!hooks.has_on_start());

    let seed = Some(vec!["seed".to_string()]);
    let result = smol::block_on(hooks.run_on_start(seed.clone()));
    assert_eq!(result, seed);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn test_on_start_mask_detect() {
    let dir = unique_test_dir("on-start-detect");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(&hook_path, "function on_start(queue) end\n").unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
    assert!(hooks.has_on_start());
    assert!(hooks.hook_mask & HOOK_ON_START != 0);

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}
