mod helpers;

use helpers::{DbVariant, MockServer, TestEnv};
use spyweb::lua::server::ApiServer;
use spyweb::lua::test_runner::{discover_tests, filter_tests_by_pattern, run_single_test};

use std::path::Path;

// ---------------------------------------------------------------------------
// Pipeline hooks
// ---------------------------------------------------------------------------

#[test]
fn test_pipeline_hooks_execution_order() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);
    // filter_item fires per-item (2 items from override_extract + after_extract)
    let mut expected = vec![
        "before_fetch",
        "override_fetch",
        "after_fetch",
        "override_extract",
        "after_extract",
    ];
    // 2 items: "extracted-item" + "enriched"
    expected.push("filter_item");
    expected.push("filter_item");
    expected.push("before_store");
    expected.push("before_notify");
    expected.push("before_webhook");

    assert_eq!(log, expected);
}

#[test]
fn test_pipeline_hooks_on_finished() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.run_cycle(&job, 0).unwrap();
    env.run_on_finished(&job);

    let log = env.read_log(&job);
    assert!(log.contains(&"on_finished".to_string()));
}

#[test]
fn test_pipeline_before_fetch_skip() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.set_global_bool(&job, "skip_cycle", true);
    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);
    assert!(
        log.is_empty(),
        "skipped cycle should produce no hook log entries: {:?}",
        log
    );
}

#[test]
fn test_pipeline_before_fetch_method_override() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.set_global_string(&job, "use_method", "HEAD");
    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);
    assert!(
        log.contains(&"method:HEAD".to_string()),
        "method should be overridden to HEAD: {:?}",
        log
    );
}

#[test]
fn test_pipeline_after_fetch_error_recovery() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.set_global_bool(&job, "make_fetch_error", true);
    env.set_global_bool(&job, "recover_from_error", true);
    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);
    assert!(
        log.contains(&"before_store".to_string()),
        "pipeline should recover from fetch error and continue: {:?}",
        log
    );
}

#[test]
fn test_pipeline_filter_item_drop() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.set_global_bool(&job, "drop_item", true);
    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);
    assert!(
        log.contains(&"before_store".to_string()),
        "remaining items should reach before_store after filtering: {:?}",
        log
    );
}

#[test]
fn test_pipeline_before_notify_skip() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.set_global_bool(&job, "skip_notify", true);
    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);
    assert!(
        log.contains(&"before_webhook".to_string()),
        "webhook should still fire after notify skip: {:?}",
        log
    );
}

#[test]
fn test_pipeline_before_webhook_reshape() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.set_global_bool(&job, "reshape_webhook", true);
    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);
    assert!(
        log.contains(&"before_webhook".to_string()),
        "webhook reshape should not cause errors: {:?}",
        log
    );
}

#[test]
fn test_pipeline_ctx_state_read() {
    let env = TestEnv::new("pipeline-hooks", DbVariant::Kv);
    let job = env.load_job("pipeline-hooks");
    env.set_global_bool(&job, "read_ctx_state", true);
    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);
    assert!(
        log.iter().any(|s| s.starts_with("selector_matches:")),
        "ctx.selector_matches should be readable: {:?}",
        log
    );
    assert!(
        log.iter().any(|s| s.starts_with("last_fetch_ok:")),
        "ctx.last_fetch should be readable: {:?}",
        log
    );
    assert!(
        log.iter().any(|s| s.starts_with("last_fetch_status:")),
        "ctx.last_fetch.response.status should be readable: {:?}",
        log
    );
}

// ---------------------------------------------------------------------------
// Defer lifecycle
// ---------------------------------------------------------------------------

#[test]
fn test_defer_lifo_order() {
    let env = TestEnv::new("defer-lifecycle", DbVariant::Kv);
    let job = env.load_job("defer-lifecycle");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);

    // LIFO: "defer:first" pushed last but runs first
    let first_pos = log.iter().position(|s| s == "defer:first");
    let second_pos = log.iter().position(|s| s == "defer:second");
    let err_pos = log.iter().position(|s| s == "defer:error");
    let re_pos = log.iter().position(|s| s == "defer:re-entrant");

    assert!(first_pos.is_some());
    assert!(second_pos.is_some());
    assert!(err_pos.is_some());
    assert!(re_pos.is_some());

    // All core defers run before lifecycle hooks
    let success_pos = log.iter().position(|s| s == "on_success");
    let finally_pos = log.iter().position(|s| s == "on_finally");

    assert!(re_pos.unwrap() < success_pos.unwrap_or(usize::MAX));
    assert!(finally_pos.is_some() && finally_pos.unwrap() > success_pos.unwrap_or(0));
}

#[test]
fn test_defer_error_safety() {
    let env = TestEnv::new("defer-lifecycle", DbVariant::Kv);
    let job = env.load_job("defer-lifecycle");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);

    // "defer:second" should be in the log even though "defer:error" errors
    assert!(
        log.contains(&"defer:second".to_string()),
        "defer:second should fire despite error in defer:error"
    );

    // "defer:re-entrant" should be in the log (re-entrant defer fires in loop drain)
    assert!(
        log.contains(&"defer:re-entrant".to_string()),
        "re-entrant defer should fire"
    );
}

#[test]
fn test_defer_ctx_shared() {
    let env = TestEnv::new("defer-lifecycle", DbVariant::Kv);
    let job = env.load_job("defer-lifecycle");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);
    assert!(
        log.iter().any(|s| s.starts_with("shared_items:")),
        "ctx.shared should be passed from before_store to defer.lua"
    );
}

#[test]
fn test_defer_on_error_from_pipeline() {
    let env = TestEnv::new("defer-lifecycle", DbVariant::Kv);
    let job = env.load_job("defer-lifecycle");
    env.set_global_bool(&job, "trigger_on_error", true);
    let result = env.run_cycle(&job, 0);
    assert!(
        result.is_err(),
        "pipeline should fail when trigger_on_error is set"
    );

    let log = env.read_log(&job);
    assert!(
        log.iter()
            .any(|s| s.starts_with("on_error:simulated pipeline error")),
        "on_error should have been called: {:?}",
        log
    );
    assert!(
        log.contains(&"on_finally".to_string()),
        "on_finally should fire after on_error: {:?}",
        log
    );
    assert!(
        !log.contains(&"on_success".to_string()),
        "on_success should not fire when pipeline errors: {:?}",
        log
    );
}

// ---------------------------------------------------------------------------
// Async functions inside defer() — verify they error at runtime
// ---------------------------------------------------------------------------

#[test]
fn test_defer_async_bindings_error() {
    let env = TestEnv::new("defer-lifecycle", DbVariant::Kv);
    let job = env.load_job("defer-lifecycle");
    env.set_global_bool(&job, "test_defer_async", true);
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);

    // Async bindings inside sync defer() error with "attempt to yield across
    // metamethod/C-call boundary". The table.insert() after the async call
    // never runs because the Lua function throws before reaching it.
    assert!(
        !log.iter().any(|s| s.starts_with("defer:sleep_elapsed:")),
        "sleep should error before reaching table.insert: {:?}",
        log
    );

    // The existing deferred functions (non-async) still work
    assert!(log.contains(&"defer:first".to_string()));
    assert!(log.contains(&"defer:second".to_string()));
    assert!(log.contains(&"defer:re-entrant".to_string()));
}

// ---------------------------------------------------------------------------
// KV storage
// ---------------------------------------------------------------------------

#[test]
fn test_storage_kv_operations() {
    let env = TestEnv::new("storage-kv", DbVariant::Kv);
    let job = env.load_job("storage-kv");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);
    assert!(
        log.iter().any(|s| s.contains("counter:")),
        "counter should be set"
    );

    let tested = env.read_global_bool(&job, "store_tested");
    assert_eq!(tested, Some(true));
}

// ---------------------------------------------------------------------------
// SQL storage (only with -sql variant)
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "sqlite"), ignore)]
#[test]
fn test_storage_sql_operations() {
    if !cfg!(feature = "sqlite") {
        eprintln!("run with --features sqlite to enable this test");
        return;
    }

    let env = TestEnv::new("storage-sql", DbVariant::Sql);
    let job = env.load_job("storage-sql");

    env.run_cycle(&job, 0).unwrap();
    let log = env.read_log(&job);

    assert!(
        !log.is_empty(),
        "SQL log should not be empty with sqlite feature enabled — log: {:?}",
        log
    );
    assert!(log.contains(&"insert_affected:1".to_string()));
    assert!(log.contains(&"rows_count:2".to_string()));
    assert!(log.contains(&"select_found:1".to_string()));
    assert!(log.contains(&"update_affected:1".to_string()));
    assert!(log.contains(&"delete_affected:1".to_string()));
    assert!(log.contains(&"final_count:2".to_string()));
} // end test_storage_sql_operations

// ---------------------------------------------------------------------------
// Network HTTP
// ---------------------------------------------------------------------------

#[test]
fn test_network_http_bindings() {
    let mock = MockServer::start();
    let env = TestEnv::new("network-http", DbVariant::Kv);

    // Bake the mock port into hooks.lua via template substitution
    let hooks_path = env.jobs_dir.join("network-http").join("hooks.lua");
    let hooks_src = std::fs::read_to_string(&hooks_path).unwrap();
    let hooks_src = hooks_src.replace("MOCK_PORT", &mock.port.to_string());
    std::fs::write(&hooks_path, hooks_src).unwrap();

    let job = env.load_job("network-http");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);

    assert!(
        log.contains(&"get_query:q=hello".to_string()),
        "log={:?}",
        log
    );
    assert!(log.contains(&"get_status:200".to_string()));
    assert!(log.contains(&"post_method:POST".to_string()));
    assert!(log.contains(&"req_status:200".to_string()));
    assert!(log.contains(&"put_method:PUT".to_string()));
    assert!(log.contains(&"patch_method:PATCH".to_string()));
    assert!(log.contains(&"head_status:200".to_string()));
    assert!(log.contains(&"mp_response:200".to_string()));
    assert!(log.contains(&"error_status:500".to_string()));

    let tested = env.read_global_bool(&job, "http_tested");
    assert_eq!(tested, Some(true));

    // Verify webhook was recorded by mock server
    let webhooks = mock.recorded_webhooks.lock().unwrap();
    assert!(!webhooks.is_empty(), "webhook should have been recorded");
    assert!(
        webhooks.iter().any(|w| w.contains("cycle_complete")),
        "webhook payload should contain cycle_complete"
    );
}

// ---------------------------------------------------------------------------
// override_fetch retry pattern
// ---------------------------------------------------------------------------

#[test]
fn test_override_fetch_retry_pattern() {
    let env = TestEnv::new("override-fetch-retry", DbVariant::Kv);
    let job = env.load_job("override-fetch-retry");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);

    // 3 attempts x 2 proxies = 6 retry entries
    let retries: Vec<_> = log
        .iter()
        .filter(|l| l.starts_with("retry_fail:"))
        .collect();
    assert_eq!(retries.len(), 6, "expected 6 retries, log={:?}", log);

    // Each retry records a non-empty error kind
    for r in &retries {
        let parts: Vec<&str> = r.split(':').collect();
        assert_eq!(parts.len(), 4, "bad retry format: {r}");
        assert!(!parts[3].is_empty(), "empty error kind in {r}");
    }

    // Pipeline receives the error from override_fetch
    assert!(
        log.contains(&"after_fetch_ok:false".to_string()),
        "log={:?}",
        log
    );
    assert!(
        log.contains(&"after_fetch_error:all retries exhausted".to_string()),
        "log={:?}",
        log
    );
}

// ---------------------------------------------------------------------------
// Multi-worker
// ---------------------------------------------------------------------------

#[test]
fn test_multi_worker_worker_id() {
    let env = TestEnv::new("multi-worker", DbVariant::Kv);

    for wid in 1..=3 {
        let job = env.load_job("multi-worker");
        env.run_cycle(&job, wid).unwrap();
    }

    let job = env.load_job("multi-worker");

    let total_workers_raw = env.read_global_raw_string(&job, "final_log");
    if let Some(raw) = total_workers_raw {
        for wid in 1..=3 {
            assert!(
                raw.contains(&format!("w{}:", wid)),
                "worker {} should have entries in final_log",
                wid
            );
        }
        // Each worker should have at least "start:" and "readonly:" entries
        for wid in 1..=3 {
            assert!(
                raw.contains(&format!("w{}:start:", wid)),
                "worker {} should have a start entry",
                wid
            );
        }
    }
}

#[test]
fn test_multi_worker_worker_id_readonly() {
    let env = TestEnv::new("multi-worker", DbVariant::Kv);
    let job = env.load_job("multi-worker");
    env.run_cycle(&job, 1).unwrap();

    let log = env.read_log(&job);
    assert!(
        log.contains(&"readonly:true".to_string()),
        "worker_id should be read-only"
    );
}

// ---------------------------------------------------------------------------
// Lua testing system
// ---------------------------------------------------------------------------

#[test]
fn test_lua_testing_discovery() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    let tests = discover_tests(&job_dir, "lua-testing").unwrap();

    assert!(tests.contains(&"test_discovered_from_tests_lua".to_string()));
    assert!(tests.contains(&"test_discovered_from_hooks_lua".to_string()));
    assert!(tests.contains(&"test_discovered_from_defer_lua".to_string()));
    assert!(tests.contains(&"test_uses_defer_lua_helper".to_string()));
    assert!(tests.contains(&"test_uses_hooks_lua_helper".to_string()));

    // Non-test-prefixed functions should NOT be discovered
    assert!(!tests.contains(&"hooks_helper".to_string()));
    assert!(!tests.contains(&"defer_helper".to_string()));
}

#[test]
fn test_lua_testing_assertions() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    smol::block_on(async {
        run_single_test(&job_dir, "lua-testing", "test_discovered_from_tests_lua")
            .await
            .unwrap();

        run_single_test(&job_dir, "lua-testing", "test_assert_fail_detected")
            .await
            .unwrap();
    });
}

#[test]
fn test_lua_testing_helpers_across_files() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    smol::block_on(async {
        run_single_test(&job_dir, "lua-testing", "test_uses_defer_lua_helper")
            .await
            .unwrap();

        run_single_test(&job_dir, "lua-testing", "test_uses_hooks_lua_helper")
            .await
            .unwrap();
    });
}

#[test]
fn test_lua_testing_bindings_available() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    smol::block_on(async {
        run_single_test(&job_dir, "lua-testing", "test_discovered_from_hooks_lua")
            .await
            .unwrap();

        run_single_test(&job_dir, "lua-testing", "test_discovered_from_defer_lua")
            .await
            .unwrap();
    });
}

#[test]
fn test_lua_testing_pattern_filter() {
    let tests = vec![
        "test_price_cleanup".to_string(),
        "test_name_cleanup".to_string(),
        "test_price_parse".to_string(),
    ];

    let filtered = filter_tests_by_pattern(tests.clone(), Some("price"));
    assert_eq!(filtered.len(), 2);
    assert!(filtered.contains(&"test_price_cleanup".to_string()));
    assert!(filtered.contains(&"test_price_parse".to_string()));

    let all = filter_tests_by_pattern(tests, None);
    assert_eq!(all.len(), 3);
}

#[test]
fn test_lua_testing_deliberate_failure() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    smol::block_on(async {
        let result = run_single_test(&job_dir, "lua-testing", "test_deliberately_fails").await;
        assert!(
            result.is_err(),
            "deliberate failure test should return error"
        );
    });
}

#[test]
fn test_lua_testing_sleep() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    smol::block_on(async {
        run_single_test(&job_dir, "lua-testing", "test_sleep_works")
            .await
            .unwrap();
    });
}

#[test]
fn test_lua_testing_notify() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    smol::block_on(async {
        run_single_test(&job_dir, "lua-testing", "test_notify_available")
            .await
            .unwrap();
    });
}

#[test]
fn test_lua_testing_log() {
    let env = TestEnv::new("lua-testing", DbVariant::Kv);
    let job_dir = env.jobs_dir.join("lua-testing");

    smol::block_on(async {
        run_single_test(&job_dir, "lua-testing", "test_log_works")
            .await
            .unwrap();
    });

    let hooks_log = job_dir.join("hooks.log");
    assert!(
        hooks_log.exists(),
        "hooks.log should exist after log() call"
    );
    let content = std::fs::read_to_string(&hooks_log).unwrap_or_default();
    assert!(
        content.contains("e2e log test"),
        "hooks.log should contain log message, got: {}",
        content
    );
}

// ---------------------------------------------------------------------------
// File system
// ---------------------------------------------------------------------------

#[test]
fn test_filesystem_operations() {
    let env = TestEnv::new("filesystem", DbVariant::Kv);

    // Create shared/ directory and file for fallback test
    let shared_dir = std::env::current_dir().unwrap().join("shared");
    std::fs::create_dir_all(&shared_dir).unwrap();
    std::fs::write(shared_dir.join("global.txt"), "shared content").unwrap();

    let job = env.load_job("filesystem");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);
    assert!(log.iter().any(|s| s.starts_with("read:hello world")));
    assert!(log.iter().any(|s| s.starts_with("appended:")));
    assert!(log.iter().any(|s| s.starts_with("binary_len:")));
    assert!(log.contains(&"traversal_rejected:true".to_string()));
    assert!(log.contains(&"absolute_rejected:true".to_string()));
    assert!(log.iter().any(|s| s.starts_with("json:")));
    assert!(
        log.contains(&"shared:shared content".to_string()),
        "shared/ fallback should work, log={:?}",
        log
    );
    assert!(
        log.contains(&"shared_write:from shared write".to_string()),
        "write to shared/ should work"
    );

    // Clean up shared/
    let _ = std::fs::remove_dir_all(&shared_dir);
}

// ---------------------------------------------------------------------------
// Require modules
// ---------------------------------------------------------------------------

#[test]
fn test_require_modules() {
    // Create root module on the fly
    let root_module = std::env::current_dir().unwrap().join("root_module.lua");
    std::fs::write(&root_module, "return { val = \"root\" }").unwrap();

    let env = TestEnv::new("require-modules", DbVariant::Kv);
    let job = env.load_job("require-modules");
    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);
    assert!(
        log.contains(&"local:local".to_string()),
        "require from job dir"
    );
    assert!(
        log.contains(&"deep:deep".to_string()),
        "require with deep dotted path"
    );
    assert!(
        log.contains(&"root:root".to_string()),
        "require from project root"
    );

    let tested = env.read_global_bool(&job, "require_tested");
    assert_eq!(tested, Some(true));

    // Clean up root module
    let _ = std::fs::remove_file(&root_module);
}

// ---------------------------------------------------------------------------
// Require modules — security
// ---------------------------------------------------------------------------

#[test]
fn test_require_security() {
    let env = TestEnv::new("require-modules", DbVariant::Kv);
    let job = env.load_job("require-modules");

    let job_dir = env.jobs_dir.join("require-modules");
    let symlink = job_dir.join("escape_link.lua");
    let secret_dir = tempfile::tempdir().unwrap();
    let target = secret_dir.path().join("stolen.lua");
    std::fs::write(&target, "return { val = \"stolen\" }").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &symlink).unwrap();

    env.run_cycle(&job, 0).unwrap();

    let log = env.read_log(&job);

    assert!(log.contains(&"blocked:traversal".to_string()));
    assert!(log.contains(&"blocked:absolute".to_string()));
    assert!(log.contains(&"blocked:dotdot".to_string()));
    assert!(log.contains(&"blocked:doubledot".to_string()));
    assert!(log.contains(&"blocked:nullbyte".to_string()));

    assert!(
        log.contains(&"blocked:symlink".to_string()),
        "symlink escape must be blocked, got: {:?}",
        log
    );

    let _ = std::fs::remove_file(&symlink);
}

// ---------------------------------------------------------------------------
// Server API
// ---------------------------------------------------------------------------

fn server_req(method: &str, url: &str) -> spyweb::services::server::types::Request {
    spyweb::services::server::types::Request::fake(method, url)
}

fn server_req_with_body(
    method: &str,
    url: &str,
    body: &str,
) -> spyweb::services::server::types::Request {
    spyweb::services::server::types::Request::fake_with(
        method,
        url,
        vec![],
        body.as_bytes().to_vec(),
    )
}

fn read_body(resp: &spyweb::services::server::types::Response) -> String {
    resp.body_string()
}

#[test]
fn test_server_api_ping() {
    let env = TestEnv::new("server-api", DbVariant::Kv);
    let init_source = std::fs::read_to_string(env.jobs_dir.join("server-api").join("init.lua"))
        .expect("failed to read init.lua");

    let server = ApiServer::new(
        env.db.clone(),
        init_source,
        std::path::PathBuf::from("/dev/null"),
    );

    let resp = server.handle("GET", "ping", vec![], &server_req("GET", "/api/v/ping"));
    assert_eq!(resp.status, 200);
}

#[test]
fn test_server_api_post() {
    let env = TestEnv::new("server-api", DbVariant::Kv);
    let init_source = std::fs::read_to_string(env.jobs_dir.join("server-api").join("init.lua"))
        .expect("failed to read init.lua");

    let server = ApiServer::new(
        env.db.clone(),
        init_source,
        std::path::PathBuf::from("/dev/null"),
    );
    let resp = server.handle(
        "POST",
        "data",
        vec![],
        &server_req_with_body("POST", "/api/v/data", r#"{"foo":"bar"}"#),
    );
    assert_eq!(resp.status, 201);
    let body = read_body(&resp);
    assert!(
        body.contains("foo"),
        "response should contain received data"
    );
}

#[test]
fn test_server_api_path_args() {
    let env = TestEnv::new("server-api", DbVariant::Kv);
    let init_source = std::fs::read_to_string(env.jobs_dir.join("server-api").join("init.lua"))
        .expect("failed to read init.lua");

    let server = ApiServer::new(
        env.db.clone(),
        init_source,
        std::path::PathBuf::from("/dev/null"),
    );
    let resp = server.handle(
        "GET",
        "user",
        vec!["42".into()],
        &server_req("GET", "/api/v/user/42"),
    );
    assert_eq!(resp.status, 200);
    let body = read_body(&resp);
    assert!(
        body.contains("42"),
        "path arg should be reflected in response"
    );
}

#[test]
fn test_server_api_all_methods() {
    let env = TestEnv::new("server-api", DbVariant::Kv);
    let init_source = std::fs::read_to_string(env.jobs_dir.join("server-api").join("init.lua"))
        .expect("failed to read init.lua");

    let server = ApiServer::new(
        env.db.clone(),
        init_source,
        std::path::PathBuf::from("/dev/null"),
    );

    // GET
    let r = server.handle("GET", "ping", vec![], &server_req("GET", "/api/v/ping"));
    assert_eq!(r.status, 200);

    // PUT
    let r = server.handle(
        "PUT",
        "replace",
        vec![],
        &server_req("PUT", "/api/v/replace"),
    );
    assert_eq!(r.status, 200);

    // PATCH
    let r = server.handle(
        "PATCH",
        "partial",
        vec![],
        &server_req("PATCH", "/api/v/partial"),
    );
    assert_eq!(r.status, 200);

    // DELETE
    let r = server.handle(
        "DELETE",
        "remove",
        vec![],
        &server_req("DELETE", "/api/v/remove"),
    );
    assert_eq!(r.status, 204);

    // Method table fallback (all.*)
    let r = server.handle(
        "OPTIONS",
        "catchall",
        vec![],
        &server_req("OPTIONS", "/api/v/catchall"),
    );
    assert_eq!(r.status, 200);
}

#[test]
fn test_server_api_defer() {
    let env = TestEnv::new("server-api", DbVariant::Kv);
    let init_source = std::fs::read_to_string(env.jobs_dir.join("server-api").join("init.lua"))
        .expect("failed to read init.lua");

    let server = ApiServer::new(
        env.db.clone(),
        init_source,
        std::path::PathBuf::from("/dev/null"),
    );
    let resp = server.handle(
        "GET",
        "deferred",
        vec![],
        &server_req("GET", "/api/v/deferred"),
    );
    assert_eq!(resp.status, 200);

    // Verify defer actually ran
    let body = read_body(&resp);
    assert!(body.contains("deferred"));

    // Verify defer with self via closure capture stored the path
    let stored = env.db.lua_get("test_defer_self_path").unwrap();
    assert_eq!(stored, Some("/api/v/deferred".to_string()));
}

// ---------------------------------------------------------------------------
// Sync bindings in server API
// ---------------------------------------------------------------------------

#[test]
fn test_server_api_sync_bindings() {
    let env = TestEnv::new("server-api", DbVariant::Kv);
    let init_source = std::fs::read_to_string(env.jobs_dir.join("server-api").join("init.lua"))
        .expect("failed to read init.lua");

    let server = ApiServer::new(
        env.db.clone(),
        init_source,
        std::path::PathBuf::from("/dev/null"),
    );

    // json_encode
    let resp = server.handle(
        "GET",
        "json_encode_test",
        vec![],
        &server_req("GET", "/api/v/json_encode_test"),
    );
    assert_eq!(resp.status, 200);
    let body = read_body(&resp);
    assert!(body.contains("1"), "json_encode should encode number");
    assert!(body.contains("two"), "json_encode should encode string");
    assert!(body.contains("true"), "json_encode should encode boolean");

    // json_decode
    let resp = server.handle(
        "GET",
        "json_decode_test",
        vec![],
        &server_req("GET", "/api/v/json_decode_test"),
    );
    assert_eq!(resp.status, 200);
    let body = read_body(&resp);
    assert!(body.contains("42"), "json_decode should decode number");
    assert!(body.contains("hello"), "json_decode should decode string");

    // env_get
    unsafe { std::env::set_var("SPYWEB_TEST_VAR", "test_value_123") };
    let resp = server.handle(
        "GET",
        "env_get_test",
        vec![],
        &server_req("GET", "/api/v/env_get_test"),
    );
    assert_eq!(resp.status, 200);
    let body = read_body(&resp);
    assert!(
        body.contains("test_value_123"),
        "env_get should read SPYWEB_ prefixed var"
    );
    unsafe { std::env::remove_var("SPYWEB_TEST_VAR") };

    // dump
    let resp = server.handle(
        "GET",
        "dump_test",
        vec![],
        &server_req("GET", "/api/v/dump_test"),
    );
    assert_eq!(resp.status, 200);
    let body = read_body(&resp);
    assert!(body.contains("x"), "dump should include key names");
    assert!(body.contains("1"), "dump should include values");

    // copy
    let resp = server.handle(
        "GET",
        "copy_test",
        vec![],
        &server_req("GET", "/api/v/copy_test"),
    );
    assert_eq!(resp.status, 200);
    let body = read_body(&resp);
    assert!(body.contains("1"), "copy should preserve original");
    assert!(
        body.contains("99"),
        "copy should allow modification of copy"
    );

    // deep_copy
    let resp = server.handle(
        "GET",
        "deep_copy_test",
        vec![],
        &server_req("GET", "/api/v/deep_copy_test"),
    );
    assert_eq!(resp.status, 200);
    let body = read_body(&resp);
    assert!(
        body.contains("10"),
        "deep_copy should preserve original nested"
    );
    assert!(
        body.contains("99"),
        "deep_copy should allow modification of copy"
    );
}

// ---------------------------------------------------------------------------
// Config TOML options
// ---------------------------------------------------------------------------

#[test]
fn test_config_toml_options() {
    let env = TestEnv::new("config-toml", DbVariant::Kv);
    let job = env.load_job("config-toml");

    // Verify config was parsed correctly
    assert!(job.config.debug, "debug should be true");
    assert_eq!(
        job.config
            .headers
            .as_ref()
            .and_then(|h| h.get("X-Test-Header")),
        Some(&"from-config".to_string()),
        "headers should contain X-Test-Header"
    );
    assert_eq!(
        job.config.keywords,
        Some(vec!["target".to_string()]),
        "keywords should be set"
    );
    assert_eq!(
        job.config.search_fields,
        Some(vec!["title".to_string(), "desc".to_string()]),
        "search_fields should be set"
    );
    assert_eq!(
        job.config.hash_fields,
        Some(vec!["title".to_string()]),
        "hash_fields should be set"
    );
    assert_eq!(
        job.config.webhook.as_ref().map(|w| w.enabled),
        Some(false),
        "webhook.enabled should be false"
    );
    assert_eq!(
        job.config.notification.as_ref().map(|n| n.enabled),
        Some(true),
        "notification.enabled should be true"
    );
    assert_eq!(
        job.config
            .notification
            .as_ref()
            .and_then(|n| n.title.as_deref()),
        Some("Config Test: {job_name}"),
        "notification.title should be set"
    );
    assert_eq!(
        job.config.proxy.as_ref().map(|p| p.enabled),
        Some(true),
        "proxy.enabled should be true"
    );
    assert_eq!(
        job.config
            .proxy
            .as_ref()
            .map(|p| p.urls.first().map(|s| s.as_str())),
        Some(Some("http://127.0.0.1:19999")),
        "proxy.urls should be set"
    );

    // Run a cycle — uses override_fetch so no real network
    let result = env.run_cycle(&job, 0);
    assert!(result.is_ok(), "cycle failed: {:?}", result.err());

    // Verify hooks observed the config options
    let log = env.read_log(&job);
    assert!(
        log.contains(&"headers:ok".to_string()),
        "headers config not verified, log: {:?}",
        log
    );
    assert!(
        log.contains(&"debug_hook_ran".to_string()),
        "debug: after_extract hook did not run, log: {:?}",
        log
    );
    assert!(
        log.contains(&"keywords:ok".to_string()),
        "keywords config not verified, log: {:?}",
        log
    );
    assert!(
        log.contains(&"webhook_hook_called".to_string()),
        "webhook hook not called, log: {:?}",
        log
    );
    assert!(
        log.contains(&"notify_hook_called".to_string()),
        "notify hook not called, log: {:?}",
        log
    );

    // Verify debug=true wrote debug files to disk
    let job_dir = env.jobs_dir.join("config-toml");
    let response_html = job_dir.join("config_toml-response.html");
    let fields_json = job_dir.join("config_toml-fields.json");
    assert!(
        response_html.exists(),
        "debug should create response.html at {:?}",
        response_html
    );
    assert!(
        fields_json.exists(),
        "debug should create fields.json at {:?}",
        fields_json
    );

    assert_eq!(env.read_global_bool(&job, "config_tested"), Some(true));
}

// ---------------------------------------------------------------------------
// CDP browser (optional, skips if no browser)
// ---------------------------------------------------------------------------

fn bake_cdp_hooks(env: &TestEnv, exe: Option<&str>, ws: Option<&str>) {
    let hooks_path = env.jobs_dir.join("cdp-browser").join("hooks.lua");
    let hooks_src = std::fs::read_to_string(&hooks_path).unwrap();
    let ws_val = ws.unwrap_or("CDP_WS_URL");
    let exe_val = exe.unwrap_or("CDP_EXECUTABLE");
    let hooks_src = hooks_src
        .replace("CDP_WS_URL", ws_val)
        .replace("CDP_EXECUTABLE", exe_val);
    std::fs::write(&hooks_path, hooks_src).unwrap();
}

#[test]
fn test_cdp_auto_detect() {
    let env = TestEnv::new("cdp-browser", DbVariant::Kv);
    bake_cdp_hooks(&env, None, None);
    let job = env.load_job("cdp-browser");

    match env.run_cycle(&job, 0) {
        Ok(()) => {
            let tested = env.read_global_bool(&job, "cdp_tested");
            assert_eq!(
                tested,
                Some(true),
                "CDP test: browser should have been used"
            );
        }
        Err(e) => {
            eprintln!("CDP auto-detect test skipped: {}", e);
        }
    }
}

#[test]
fn test_cdp_with_executable() {
    let exe = match std::env::var("SPYWEB_CDP_EXECUTABLE") {
        Ok(path) if Path::new(&path).exists() => path,
        _ => {
            eprintln!("CDP executable test skipped: SPYWEB_CDP_EXECUTABLE not set or not found");
            return;
        }
    };

    let env = TestEnv::new("cdp-browser", DbVariant::Kv);
    bake_cdp_hooks(&env, Some(&exe), None);
    let job = env.load_job("cdp-browser");

    match env.run_cycle(&job, 0) {
        Ok(()) => {
            let tested = env.read_global_bool(&job, "cdp_tested");
            assert_eq!(tested, Some(true));
        }
        Err(e) => {
            panic!(
                "CDP executable test failed with provided executable '{}': {}",
                exe, e
            );
        }
    }
}

#[test]
fn test_cdp_with_ws_url() {
    let url = match std::env::var("SPYWEB_CDP_WS_URL") {
        Ok(u) => u,
        _ => {
            eprintln!("CDP WS URL test skipped: SPYWEB_CDP_WS_URL not set");
            return;
        }
    };

    let env = TestEnv::new("cdp-browser", DbVariant::Kv);
    bake_cdp_hooks(&env, None, Some(&url));
    let job = env.load_job("cdp-browser");

    match env.run_cycle(&job, 0) {
        Ok(()) => {
            let tested = env.read_global_bool(&job, "cdp_tested");
            assert_eq!(tested, Some(true));
        }
        Err(e) => {
            panic!("CDP WS URL test failed with URL '{}': {}", url, e);
        }
    }
}
