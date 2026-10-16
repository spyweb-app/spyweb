use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::types::{Field, Job, JobConfig, JobOptions, Proxy as JobProxy, Rotate};
use crate::lua::hooks::JobHooks;
use crate::scraper::pipeline::{CycleRunner, CycleStats, JobCtx, Work, compute_cycle_batch};
use crate::scraper::request::{
    RequestConfig, RequestHandler, RequestResult, flatten_headers, format_error_chain,
    read_body_bounded,
};
use crate::scraper::runner::Runner;
use crate::services::db::Db;

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

// ---------------------------------------------------------------------------
// pipeline.rs tests
// ---------------------------------------------------------------------------

#[test]
fn run_once_finalizes_telemetry_before_cycle_cleanup() {
    let dir = unique_test_dir("pipeline-telemetry");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function override_fetch(request)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Job {
        config: JobConfig {
            name: "test job".into(),
            url: "https://example.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            options: JobOptions {
                interval: 60,
                ..Default::default()
            },
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    };
    let runner = Arc::new(Runner::new());

    smol::block_on(async {
        let base_request = RequestConfig::from_job(&job.config);
        let ctx = job
            .hooks
            .as_ref()
            .unwrap()
            .new_cycle_context(0)
            .await
            .unwrap();
        let tel = crate::lua::hooks::TelemetryHandle::new(job.hooks.as_ref(), Some(&ctx));
        tel.init().await.unwrap();
        JobCtx::new(&job, &db, &runner)
            .stages(&base_request, &tel)
            .await
            .unwrap();
        tel.finalize(std::time::Duration::from_millis(1)).await;

        let telemetry: mlua::Table = ctx.get("telemetry").unwrap();
        let map: mlua::Table = telemetry.get("map").unwrap();

        assert!(telemetry.get::<f64>("total_duration_ms").unwrap() >= 0.0);
        assert!(matches!(
            ctx.get::<mlua::Value>("last_fetch").unwrap(),
            mlua::Value::Table(_)
        ));
        assert_eq!(
            map.get::<mlua::Table>("override_fetch")
                .unwrap()
                .get::<String>("status")
                .unwrap(),
            "success"
        );
        assert_eq!(
            map.get::<mlua::Table>("fetch")
                .unwrap()
                .get::<String>("status")
                .unwrap(),
            "inactive"
        );
        assert_eq!(
            map.get::<mlua::Table>("override_extract")
                .unwrap()
                .get::<String>("status")
                .unwrap(),
            "success"
        );
        assert_eq!(
            map.get::<mlua::Table>("filter")
                .unwrap()
                .get::<String>("status")
                .unwrap(),
            "inactive"
        );
        assert!(matches!(
            map.get::<mlua::Table>("store")
                .unwrap()
                .get::<mlua::Value>("duration_ms")
                .unwrap(),
            mlua::Value::Nil
        ));
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn single_worker_single_url_default_path() {
    let dir = unique_test_dir("single-default");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
fetched = false

function override_fetch(request)
    fetched = true
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Job {
        config: JobConfig {
            name: "test job".into(),
            url: "https://example.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            options: JobOptions {
                interval: 60,
                ..Default::default()
            },
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    };
    let runner = Arc::new(Runner::new());

    smol::block_on(async {
        let base_request = RequestConfig::from_job(&job.config);
        JobCtx::new(&job, &db, &runner)
            .run_once(&base_request, 0)
            .await
            .unwrap();
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn url_queue_single_worker_drains_all_urls() {
    let dir = unique_test_dir("url-queue-single");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
fetched_urls = {}

function override_fetch(request)
    table.insert(fetched_urls, request.url)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Arc::new(Job {
        config: JobConfig {
            name: "test job".into(),
            url: "https://example.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            options: JobOptions {
                interval: 60,
                urls: Some(vec![
                    "https://example.com/1".into(),
                    "https://example.com/2".into(),
                    "https://example.com/3".into(),
                ]),
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
        CycleRunner::new(&job, &db, &runner, &ex, 1)
            .dispatch(Work::Queue(job.config.options.urls.clone().unwrap()))
            .await;

        let job_ref = &*job;
        let hooks = job_ref.hooks.as_ref().unwrap();
        let lua = hooks.lua.lock().await;
        let globals = lua.globals();
        let urls: Vec<String> = globals.get("fetched_urls").unwrap();
        assert_eq!(urls.len(), 3);
        assert!(urls.contains(&"https://example.com/1".to_string()));
        assert!(urls.contains(&"https://example.com/2".to_string()));
        assert!(urls.contains(&"https://example.com/3".to_string()));
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn url_queue_multi_worker_drains_all_urls() {
    let dir = unique_test_dir("url-queue-multi");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
fetched_urls = {}

function override_fetch(request)
    table.insert(fetched_urls, request.url)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Arc::new(Job {
        config: JobConfig {
            name: "test job".into(),
            url: "https://example.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            options: JobOptions {
                interval: 60,
                urls: Some(vec![
                    "https://example.com/a".into(),
                    "https://example.com/b".into(),
                    "https://example.com/c".into(),
                    "https://example.com/d".into(),
                ]),
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
        CycleRunner::new(&job, &db, &runner, &ex, 2)
            .dispatch(Work::Queue(job.config.options.urls.clone().unwrap()))
            .await;

        let job_ref = &*job;
        let hooks = job_ref.hooks.as_ref().unwrap();
        let lua = hooks.lua.lock().await;
        let globals = lua.globals();
        let urls: Vec<String> = globals.get("fetched_urls").unwrap();
        assert_eq!(urls.len(), 4);
        assert!(urls.contains(&"https://example.com/a".to_string()));
        assert!(urls.contains(&"https://example.com/b".to_string()));
        assert!(urls.contains(&"https://example.com/c".to_string()));
        assert!(urls.contains(&"https://example.com/d".to_string()));
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn multi_worker_without_urls_runs_all_workers() {
    let dir = unique_test_dir("multi-worker-no-urls");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
worker_count = 0

function override_fetch(request)
    worker_count = worker_count + 1
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Arc::new(Job {
        config: JobConfig {
            name: "test job".into(),
            url: "https://example.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            options: JobOptions {
                interval: 60,
                workers: Some(2),
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
        CycleRunner::new(&job, &db, &runner, &ex, 2)
            .dispatch(Work::Multi)
            .await;

        let job_ref = &*job;
        let hooks = job_ref.hooks.as_ref().unwrap();
        let lua = hooks.lua.lock().await;
        let globals = lua.globals();
        let count: u32 = globals.get("worker_count").unwrap();
        assert_eq!(count, 2);
    });

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn worker_tasks_run_on_supplied_executor() {
    let dir = unique_test_dir("worker-thread-probe");
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(
        &hook_path,
        r#"
function override_fetch(request)
    return { status = 200, url = request.url, headers = {}, body = "<html></html>" }
end

function override_extract(response)
    return {}
end
"#,
    )
    .unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Arc::new(Job {
        config: JobConfig {
            name: "test job".into(),
            url: "https://example.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            options: JobOptions {
                interval: 60,
                workers: Some(16),
                ..Default::default()
            },
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    });
    let runner = Arc::new(Runner::new());

    let ex = Arc::new(smol::Executor::new());
    let (stop_tx, stop_rx) = smol::channel::unbounded::<()>();
    let (executor_thread_tx, executor_thread_rx) = std::sync::mpsc::channel();
    let executor = Arc::clone(&ex);
    let rx = stop_rx.clone();
    std::thread::spawn(move || {
        executor_thread_tx
            .send(std::thread::current().id())
            .unwrap();
        let _ = smol::block_on(executor.run(rx.recv()));
    });
    let executor_thread = executor_thread_rx.recv().unwrap();

    let probe: Arc<std::sync::Mutex<std::collections::HashSet<std::thread::ThreadId>>> =
        Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

    smol::block_on(async {
        CycleRunner::new(&job, &db, &runner, &ex, 16)
            .with_probe(Some(Arc::clone(&probe)))
            .dispatch(Work::Multi)
            .await;
    });
    drop(stop_tx);

    let threads = probe.lock().unwrap();
    eprintln!("executor thread: {executor_thread:?}, worker threads: {threads:?}");
    assert!(
        !threads.is_empty(),
        "workers did not record an execution thread"
    );
    assert!(
        threads.iter().all(|thread| *thread == executor_thread),
        "workers ran outside the supplied application executor: {threads:?}",
    );

    let _ = fs::remove_file(dir.join("test.redb"));
    let _ = fs::remove_file(&hook_path);
    let _ = fs::remove_dir(&dir);
}

// ---------------------------------------------------------------------------
// runner.rs tests
// ---------------------------------------------------------------------------

fn runner_job(enabled: bool, keywords: Option<Vec<&str>>) -> JobConfig {
    JobConfig {
        name: "demo".into(),
        url: "https://example.com/jobs".into(),
        selector: ".job".into(),
        fields: vec![
            Field::Shorthand("title:h2".into()),
            Field::Full {
                name: "link".into(),
                selector: "a".into(),
                att: "href".into(),
            },
        ],
        options: JobOptions {
            keywords: keywords
                .map(|keywords| keywords.into_iter().map(str::to_string).collect::<Vec<_>>()),
            enabled,
            interval: 60,
            debug: false,
            proxy: Some(crate::config::types::Proxy {
                enabled: false,
                rotate: Rotate::RoundRobin,
                urls: vec![],
            }),
            ..Default::default()
        },
    }
}

fn runner_response(body: &str) -> RequestResult {
    RequestResult {
        url: "https://example.com/jobs".into(),
        status: 200,
        headers: Default::default(),
        body: body.into(),
        proxy: None,
        time_ms: None,
        size: None,
    }
}

#[test]
fn process_response_builds_job_result() {
    let runner = Runner::new();
    let job = runner_job(true, None);
    let html = r#"
            <div class="job"><h2>Rust Developer</h2><a href="/rust">Apply</a></div>
            <div class="job"><h2>Go Developer</h2><a href="/go">Apply</a></div>
        "#;

    let Ok(result) = runner.process_response(&job, None, runner_response(html)) else {
        panic!("processing a valid HTML response should succeed");
    };

    assert_eq!(result.status, 200);
    assert_eq!(result.item_count, 2);
}

#[test]
fn process_response_uses_keyword_filtering() {
    let runner = Runner::new();
    let job = runner_job(true, Some(vec!["rust"]));
    let html = r#"
            <div class="job"><h2>Rust Developer</h2><a href="/rust">Apply</a></div>
            <div class="job"><h2>Go Developer</h2><a href="/go">Apply</a></div>
        "#;

    let Ok(result) = runner.process_response(&job, None, runner_response(html)) else {
        panic!("processing a valid HTML response should succeed");
    };

    assert_eq!(result.item_count, 1);
    assert_eq!(
        result.items[0].fields.get("title").map(String::as_str),
        Some("Rust Developer")
    );
}

// ---------------------------------------------------------------------------
// request.rs tests
// ---------------------------------------------------------------------------

fn job_with_proxy(rotate: Rotate, urls: Vec<&str>) -> JobConfig {
    JobConfig {
        name: "test".into(),
        url: "https://example.com".into(),
        selector: ".item".into(),
        fields: vec![Field::Shorthand("title".into())],
        options: JobOptions {
            debug: false,
            interval: 60,
            proxy: Some(JobProxy {
                enabled: true,
                rotate,
                urls: urls.into_iter().map(str::to_string).collect(),
            }),
            ..Default::default()
        },
    }
}

#[test]
fn round_robin_proxy_rotation_advances() {
    let handler = RequestHandler::new();
    let job = job_with_proxy(
        Rotate::RoundRobin,
        vec!["http://proxy-1:8080", "http://proxy-2:8080"],
    );

    assert_eq!(
        handler.select_proxy(&job).as_deref(),
        Some("http://proxy-1:8080")
    );
    assert_eq!(
        handler.select_proxy(&job).as_deref(),
        Some("http://proxy-2:8080")
    );
    assert_eq!(
        handler.select_proxy(&job).as_deref(),
        Some("http://proxy-1:8080")
    );
}

#[test]
fn sticky_proxy_always_uses_first_entry() {
    let handler = RequestHandler::new();
    let job = job_with_proxy(
        Rotate::Sticky,
        vec!["http://proxy-1:8080", "http://proxy-2:8080"],
    );

    assert_eq!(
        handler.select_proxy(&job).as_deref(),
        Some("http://proxy-1:8080")
    );
    assert_eq!(
        handler.select_proxy(&job).as_deref(),
        Some("http://proxy-1:8080")
    );
}

#[test]
fn disabled_or_empty_proxy_configuration_is_ignored() {
    let handler = RequestHandler::new();
    let mut job = job_with_proxy(Rotate::RoundRobin, vec![]);
    assert_eq!(handler.select_proxy(&job), None);

    job.options.proxy = Some(JobProxy {
        enabled: false,
        rotate: Rotate::RoundRobin,
        urls: vec!["http://proxy-1:8080".into()],
    });
    assert_eq!(handler.select_proxy(&job), None);
}

#[test]
fn flatten_headers_preserves_header_names_and_values() {
    let mut headers = ureq::http::HeaderMap::new();
    headers.insert(
        "content-type",
        ureq::http::HeaderValue::from_static("text/html"),
    );
    headers.insert(
        "x-request-id",
        ureq::http::HeaderValue::from_static("abc123"),
    );

    let flattened = flatten_headers(&headers);

    assert_eq!(
        flattened.get("content-type").map(String::as_str),
        Some("text/html")
    );
    assert_eq!(
        flattened.get("x-request-id").map(String::as_str),
        Some("abc123")
    );
}

#[test]
fn build_agent_disables_http_status_as_error() {
    let handler = RequestHandler::new();
    let Ok(agent) = handler.build_agent(None, std::time::Duration::from_secs(30)) else {
        panic!("building an agent without a proxy should succeed");
    };

    assert!(
        !agent.config().http_status_as_error(),
        "HTTP 4xx/5xx responses should be treated as normal responses"
    );
}

#[test]
fn error_chain_formatter_preserves_context_and_cause() {
    let err = anyhow::anyhow!("dns lookup failed").context("request failed for job 'Spyweb'");

    let formatted = format_error_chain(&err);

    assert_eq!(
        formatted,
        "request failed for job 'Spyweb': dns lookup failed"
    );
}

#[test]
fn test_read_body_bounded_ok() {
    let body = b"hello world";
    let result = read_body_bounded(std::io::Cursor::new(body), 1024).unwrap();
    assert_eq!(result, "hello world");
}

#[test]
fn test_read_body_bounded_truncated() {
    let body = vec![b'a'; 100];
    let err = read_body_bounded(std::io::Cursor::new(body), 50).unwrap_err();
    assert!(err.to_string().contains("exceeds 50 byte limit"));
}

#[test]
fn test_read_body_bounded_exact() {
    let body = vec![b'b'; 64];
    let result = read_body_bounded(std::io::Cursor::new(body), 64).unwrap();
    assert_eq!(result.len(), 64);
}

// --- on_start batch derivation (Phase 1) ---

fn make_job_with_hooks(
    name: &str,
    urls: Option<Vec<String>>,
    workers: usize,
    hook_source: &str,
) -> (PathBuf, Job, Arc<Db>) {
    let dir = unique_test_dir(name);
    fs::create_dir_all(&dir).unwrap();
    let hook_path = dir.join("hooks.lua");
    fs::write(&hook_path, hook_source).unwrap();

    let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
    let hooks = JobHooks::load(&hook_path, Arc::clone(&db), "test_job").unwrap();
    let job = Job {
        config: JobConfig {
            name: "test_job".into(),
            url: "https://initial.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            options: JobOptions {
                workers: Some(workers),
                urls,
                interval: 60,
                ..Default::default()
            },
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    };
    (dir, job, db)
}

#[test]
fn compute_cycle_batch_backcompat_without_on_start() {
    // Queue job, no on_start → seed passes through.
    let (dir, job, _db) = make_job_with_hooks(
        "batch-backcompat-queue",
        Some(vec!["https://a.com".into(), "https://b.com".into()]),
        2,
        "function before_fetch(request) return request end\n",
    );
    let batch = smol::block_on(compute_cycle_batch(&job));
    assert_eq!(
        batch,
        Some(vec![
            "https://a.com".to_string(),
            "https://b.com".to_string()
        ])
    );
    drop(job);
    let _ = fs::remove_dir_all(&dir);

    // Legacy job (no urls), no on_start → None (legacy dispatch).
    let (dir, job, _db) = make_job_with_hooks(
        "batch-backcompat-legacy",
        None,
        1,
        "function before_fetch(request) return request end\n",
    );
    let batch = smol::block_on(compute_cycle_batch(&job));
    assert_eq!(batch, None);
    drop(job);
    let _ = fs::remove_dir_all(&dir);

    // urls = [] → has_urls false → seed None → legacy dispatch (today's behavior).
    let (dir, job, _db) = make_job_with_hooks(
        "batch-backcompat-empty",
        Some(vec![]),
        1,
        "function before_fetch(request) return request end\n",
    );
    let batch = smol::block_on(compute_cycle_batch(&job));
    assert_eq!(batch, None);
    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compute_cycle_batch_on_start_derivation() {
    // Legacy job + on_start sets a batch → promoted to queue-loop this cycle.
    let (dir, job, _db) = make_job_with_hooks(
        "batch-promote",
        None,
        2,
        r#"
function on_start(queue)
    queue.urls = { "monitor:1", "monitor:2", "monitor:3" }
end
"#,
    );
    let batch = smol::block_on(compute_cycle_batch(&job));
    assert_eq!(
        batch,
        Some(vec![
            "monitor:1".to_string(),
            "monitor:2".to_string(),
            "monitor:3".to_string()
        ])
    );
    drop(job);
    let _ = fs::remove_dir_all(&dir);

    // Empty batch → Some(vec![]) (explicit zero work; run_job_loop skips spawn).
    let (dir, job, _db) = make_job_with_hooks(
        "batch-empty",
        Some(vec!["https://seed.com".into()]),
        2,
        r#"
function on_start(queue)
    queue.urls = {}
end
"#,
    );
    let batch = smol::block_on(compute_cycle_batch(&job));
    assert_eq!(batch, Some(vec![]));
    drop(job);
    let _ = fs::remove_dir_all(&dir);

    // Queue job + on_start nils urls → no opinion → seed intact.
    let (dir, job, _db) = make_job_with_hooks(
        "batch-nil-no-opinion",
        Some(vec!["https://seed.com".into()]),
        1,
        r#"
function on_start(queue)
    queue.urls = nil
end
"#,
    );
    let batch = smol::block_on(compute_cycle_batch(&job));
    assert_eq!(batch, Some(vec!["https://seed.com".to_string()]));
    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn on_start_batch_drains_via_queue_loop_in_order() {
    // End-to-end Phase 1 shape: legacy job, on_start seeds an opaque batch,
    // single worker drains it in order through the queue-loop.
    let (dir, job, db) = make_job_with_hooks(
        "on-start-queue-loop",
        None,
        1,
        r#"
fetched = {}

function on_start(queue)
    queue.urls = { "monitor:1", "monitor:2", "monitor:3" }
end

function before_fetch(request)
    table.insert(fetched, request.url)
    return request
end

function override_fetch(request)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end
"#,
    );
    let job = Arc::new(job);
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        let batch = compute_cycle_batch(&job).await.unwrap();
        assert_eq!(batch.len(), 3);
        CycleRunner::new(&job, &db, &runner, &ex, 1)
            .dispatch(Work::Queue(batch))
            .await;

        let hooks = job.hooks.as_ref().unwrap();
        let lua = hooks.lua.lock().await;
        let fetched: Vec<String> = lua.globals().get("fetched").unwrap();
        // Pop order preserved for a single worker.
        assert_eq!(
            fetched,
            vec![
                "monitor:1".to_string(),
                "monitor:2".to_string(),
                "monitor:3".to_string()
            ]
        );
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn on_start_batch_larger_than_workers_drains_fully() {
    // 6-entry batch, 2 workers → all 6 processed in one cycle.
    let (dir, job, db) = make_job_with_hooks(
        "on-start-drain-all",
        None,
        2,
        r#"
fetched = {}

function on_start(queue)
    queue.urls = { "u1", "u2", "u3", "u4", "u5", "u6" }
end

function override_fetch(request)
    table.insert(fetched, request.url)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end
"#,
    );
    let job = Arc::new(job);
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        let batch = compute_cycle_batch(&job).await.unwrap();
        CycleRunner::new(&job, &db, &runner, &ex, 2)
            .dispatch(Work::Queue(batch))
            .await;

        let hooks = job.hooks.as_ref().unwrap();
        let lua = hooks.lua.lock().await;
        let fetched: Vec<String> = lua.globals().get("fetched").unwrap();
        assert_eq!(fetched.len(), 6);
        let set: std::collections::HashSet<String> = fetched.into_iter().collect();
        assert_eq!(set.len(), 6);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

// --- run_iteration scheduler lifecycle (Phase 1+2) ---

const LIFECYCLE_HOOKS: &str = r#"
_G.lifecycle = {}

function on_start(queue)
    table.insert(_G.lifecycle, "on_start")
end

function before_fetch(request)
    table.insert(_G.lifecycle, "fetch:" .. request.url)
    return request
end

function override_fetch(request)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    table.insert(_G.lifecycle, "on_finished")
    _G.final_counts = { queue.processed, queue.failed, queue.pushed }
    _G.final_urls_len = #queue.urls
end
"#;

struct LifecycleRun {
    log: Vec<String>,
    counts: Vec<usize>,
    urls_len: usize,
}

fn run_cycle_lifecycle(job: &Arc<Job>, db: &Arc<Db>) -> LifecycleRun {
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();
    let worker_count = job.config.worker_count();
    smol::block_on(async {
        CycleRunner::new(job, db, &runner, &ex, worker_count)
            .run_iteration()
            .await;
        let hooks = job.hooks.as_ref().unwrap();
        let lua = hooks.lua.lock().await;
        let log = lua.globals().get::<Vec<String>>("lifecycle").unwrap();
        let counts = lua.globals().get::<Vec<usize>>("final_counts").unwrap();
        let urls_len = lua.globals().get::<usize>("final_urls_len").unwrap();
        LifecycleRun {
            log,
            counts,
            urls_len,
        }
    })
}

fn assert_lifecycle_order(log: &[String], expected_fetches: usize) {
    assert_eq!(
        log.first().map(String::as_str),
        Some("on_start"),
        "on_start must run first"
    );
    assert_eq!(
        log.last().map(String::as_str),
        Some("on_finished"),
        "on_finished must run last"
    );
    let starts = log.iter().filter(|e| *e == "on_start").count();
    let finishes = log.iter().filter(|e| *e == "on_finished").count();
    assert_eq!(starts, 1, "on_start must run exactly once per cycle");
    assert_eq!(finishes, 1, "on_finished must run exactly once per cycle");
    let fetches: Vec<&String> = log.iter().filter(|e| e.starts_with("fetch:")).collect();
    assert_eq!(fetches.len(), expected_fetches, "unexpected fetch count");
}

#[test]
fn run_iteration_single_worker_on_start_first_on_finished_last() {
    let (dir, job, db) = make_job_with_hooks("lifecycle-single", None, 1, LIFECYCLE_HOOKS);
    let job = Arc::new(job);

    let run = run_cycle_lifecycle(&job, &db);
    assert_lifecycle_order(&run.log, 1);
    assert_eq!(run.log[1], "fetch:https://initial.com");
    // One successful run_once: processed=1, failed=0, pushed=0; urls always {}.
    assert_eq!(run.counts, vec![1, 0, 0]);
    assert_eq!(run.urls_len, 0);

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_iteration_multi_worker_on_start_first_on_finished_last() {
    let (dir, job, db) = make_job_with_hooks("lifecycle-multi", None, 3, LIFECYCLE_HOOKS);
    let job = Arc::new(job);

    let run = run_cycle_lifecycle(&job, &db);
    // Legacy multi mode: one run_once per worker, all on the base URL.
    assert_lifecycle_order(&run.log, 3);
    assert_eq!(run.counts, vec![3, 0, 0]);
    assert_eq!(run.urls_len, 0);

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_iteration_static_queue_on_start_first_on_finished_last() {
    let (dir, job, db) = make_job_with_hooks(
        "lifecycle-static-queue",
        Some(vec!["https://a.com".into(), "https://b.com".into()]),
        2,
        LIFECYCLE_HOOKS,
    );
    let job = Arc::new(job);

    let run = run_cycle_lifecycle(&job, &db);
    // Static queue mode: config seed untouched by on_start; both URLs drain.
    assert_lifecycle_order(&run.log, 2);
    let set: std::collections::HashSet<&str> = run
        .log
        .iter()
        .filter_map(|e| e.strip_prefix("fetch:"))
        .collect();
    assert_eq!(set.len(), 2);
    assert!(set.contains("https://a.com"));
    assert!(set.contains("https://b.com"));
    assert_eq!(run.counts, vec![2, 0, 0]);
    assert_eq!(run.urls_len, 0);

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_iteration_zero_work_batch_all_zero_counters() {
    // on_start commits {} → zero workers spawned, counters all zero,
    // on_finished still runs with the table.
    let (dir, job, db) = make_job_with_hooks(
        "lifecycle-zero-work",
        None,
        2,
        r#"
_G.lifecycle = {}

function on_start(queue)
    table.insert(_G.lifecycle, "on_start")
    queue.urls = {}
end

function before_fetch(request)
    table.insert(_G.lifecycle, "fetch")
    return request
end

function override_fetch(request)
    return { status = 200, url = request.url, headers = {}, body = "" }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    table.insert(_G.lifecycle, "on_finished")
    _G.final_counts = { queue.processed, queue.failed, queue.pushed }
    _G.final_urls_len = #queue.urls
end
"#,
    );
    let job = Arc::new(job);

    let run = run_cycle_lifecycle(&job, &db);
    assert_eq!(
        run.log,
        vec!["on_start".to_string(), "on_finished".to_string()]
    );
    assert_eq!(run.counts, vec![0, 0, 0]);

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_iteration_counters_reset_between_cycles() {
    // Two consecutive run_iteration calls: no counter leakage into the next
    // iteration (stats dropped/recreated each cycle).
    let (dir, job, db) = make_job_with_hooks("lifecycle-no-leak", None, 1, LIFECYCLE_HOOKS);
    let job = Arc::new(job);

    let run1 = run_cycle_lifecycle(&job, &db);
    assert_eq!(run1.counts, vec![1, 0, 0]);
    let run2 = run_cycle_lifecycle(&job, &db);
    assert_eq!(
        run2.counts,
        vec![1, 0, 0],
        "second cycle must not see first cycle's counters"
    );

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_iteration_before_fetch_nil_skip_counts_as_processed() {
    // Engine-level caveat: a worker whose before_fetch returns nil exits Ok
    // and counts as processed (failed stays 0).
    let (dir, job, db) = make_job_with_hooks(
        "lifecycle-nil-skip",
        None,
        1,
        r#"
function before_fetch(request)
    return nil
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed }
end
"#,
    );
    let job = Arc::new(job);

    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();
    let counts = smol::block_on(async {
        CycleRunner::new(&job, &db, &runner, &ex, 1)
            .run_iteration()
            .await;
        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        lua.globals().get::<Vec<usize>>("final_counts").unwrap()
    });
    assert_eq!(counts, vec![1, 0, 0], "nil-skip is processed=1, failed=0");

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_iteration_failed_run_counts_as_failed() {
    // override_fetch returning an invalid response flows as run_once Err →
    // processed=1, failed=1.
    let (dir, job, db) = make_job_with_hooks(
        "lifecycle-failed",
        None,
        1,
        r#"
function before_fetch(request)
    error("boom")
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed }
end
"#,
    );
    let job = Arc::new(job);

    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();
    let counts = smol::block_on(async {
        CycleRunner::new(&job, &db, &runner, &ex, 1)
            .run_iteration()
            .await;
        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        lua.globals().get::<Vec<usize>>("final_counts").unwrap()
    });
    assert_eq!(
        counts,
        vec![1, 1, 0],
        "failed run_once: processed=1, failed=1"
    );

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

// --- Phase 3: queue_push + coordinator ---

#[test]
fn queue_push_grows_cycle_single_worker() {
    let (dir, job, db) = make_job_with_hooks(
        "queue-push-grow",
        None,
        1,
        r#"
fetched = {}

function on_start(queue)
    local ok, err = pcall(queue_push, "too-early")
    _G.start_err = err
    queue.urls = { "u1", "u2" }
end

function override_fetch(request)
    table.insert(fetched, request.url)
    if request.url == "u1" then
        queue_push("u3")
    end
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed, queue.duplicates }
end
"#,
    );
    let job = Arc::new(job);
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        let stats = CycleRunner::new(&job, &db, &runner, &ex, 1)
            .run_iteration()
            .await;
        assert_eq!(stats.processed(), 3);
        assert_eq!(stats.failed(), 0);
        assert_eq!(stats.pushed(), 1);
        assert_eq!(stats.duplicates(), 0);

        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        let fetched: Vec<String> = lua.globals().get("fetched").unwrap();
        assert_eq!(fetched, ["u1", "u2", "u3"]);
        let start_err: mlua::Error = lua.globals().get("start_err").unwrap();
        assert!(
            start_err
                .to_string()
                .starts_with("queue_push requires an active queue cycle"),
            "unexpected gate error: {start_err}"
        );
        let counts: Vec<usize> = lua.globals().get("final_counts").unwrap();
        assert_eq!(counts, vec![3, 0, 1, 0]);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn queue_push_dedup_rejects_repeats_and_seed_repush() {
    let (dir, job, db) = make_job_with_hooks(
        "queue-push-dedup",
        None,
        1,
        r#"
fetched = {}

function on_start(queue)
    queue.urls = { "u1" }
end

function override_fetch(request)
    table.insert(fetched, request.url)
    if request.url == "u1" then
        queue_push("dup")
        queue_push("dup")
        queue_push("u1")
    end
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed, queue.duplicates }
end
"#,
    );
    let job = Arc::new(job);
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        let stats = CycleRunner::new(&job, &db, &runner, &ex, 1)
            .run_iteration()
            .await;
        assert_eq!(stats.processed(), 2);
        assert_eq!(stats.pushed(), 1, "only the first dup is admitted");
        assert_eq!(stats.duplicates(), 2, "repeat push + seed re-push rejected");

        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        let fetched: Vec<String> = lua.globals().get("fetched").unwrap();
        assert_eq!(fetched, ["u1", "dup"]);
        let counts: Vec<usize> = lua.globals().get("final_counts").unwrap();
        assert_eq!(counts, vec![2, 0, 1, 2]);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn queue_push_dedup_identity_exact_strings() {
    let (dir, job, db) = make_job_with_hooks(
        "queue-push-identity",
        None,
        1,
        r#"
fetched = {}

function on_start(queue)
    queue.urls = { "u1" }
end

function override_fetch(request)
    table.insert(fetched, request.url)
    if request.url == "u1" then
        queue_push("monitor:5")
        queue_push("monitor:05")
    end
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed, queue.duplicates }
end
"#,
    );
    let job = Arc::new(job);
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        let stats = CycleRunner::new(&job, &db, &runner, &ex, 1)
            .run_iteration()
            .await;
        assert_eq!(stats.processed(), 3);
        assert_eq!(stats.pushed(), 2, "leading-zero variant is a distinct key");
        assert_eq!(stats.duplicates(), 0);

        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        let fetched: Vec<String> = lua.globals().get("fetched").unwrap();
        assert_eq!(fetched, ["u1", "monitor:5", "monitor:05"]);
        let counts: Vec<usize> = lua.globals().get("final_counts").unwrap();
        assert_eq!(counts, vec![3, 0, 2, 0]);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn queue_push_gate_error_outside_active_cycle() {
    let (dir, job, db) = make_job_with_hooks(
        "queue-push-gate",
        None,
        1,
        r#"
function override_fetch(request)
    local ok, err = pcall(queue_push, "x")
    _G.gate_err = err
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed, queue.duplicates }
end
"#,
    );
    let job = Arc::new(job);
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();

    smol::block_on(async {
        let stats = CycleRunner::new(&job, &db, &runner, &ex, 1)
            .run_iteration()
            .await;
        assert_eq!(stats.processed(), 1);
        assert_eq!(stats.failed(), 0, "pcall absorbs the gate error");

        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        let gate_err: mlua::Error = lua.globals().get("gate_err").unwrap();
        assert!(
            gate_err
                .to_string()
                .starts_with("queue_push requires an active queue cycle"),
            "unexpected gate error: {gate_err}"
        );
        let counts: Vec<usize> = lua.globals().get("final_counts").unwrap();
        assert_eq!(counts, vec![1, 0, 0, 0]);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn queue_push_over_cap_rejects_combined_initial_and_pushed() {
    let (dir, job, _db) = make_job_with_hooks("queue-push-cap", None, 1, "");
    let job = Arc::new(job);
    let hooks = job.hooks.as_ref().unwrap();
    let (tx, _rx) = smol::channel::unbounded::<crate::scraper::coordinator::Message>();
    hooks
        .queue
        .begin_cycle(tx, crate::config::types::MAX_BATCH - 1);

    smol::block_on(async {
        let lua = hooks.lua.lock().await;
        lua.load(
            r#"
local ok1, _ = pcall(queue_push, "a")
local ok2, err2 = pcall(queue_push, "b")
_G.cap_first_ok = ok1
_G.cap_second_err = err2
"#,
        )
        .exec_async()
        .await
        .unwrap();
        let first_ok: bool = lua.globals().get("cap_first_ok").unwrap();
        let second_err: mlua::Error = lua.globals().get("cap_second_err").unwrap();
        assert!(
            first_ok,
            "push filling the last slot of the combined budget is admitted"
        );
        assert!(
            second_err
                .to_string()
                .starts_with("queue_push rejected: cycle queue is full"),
            "unexpected over-cap error: {second_err}"
        );
    });

    hooks.queue.deactivate();
    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

/// Drop the first cycle future mid-flight (cancellation), then run a full
/// second cycle on the same job and assert it is unaffected.
fn run_cancel_then_next_cycle(job: &Arc<Job>, db: &Arc<Db>, drop_ms: u64) -> Arc<CycleStats> {
    let runner = Arc::new(Runner::new());
    let (ex, _keep_alive) = single_thread_executor();
    let workers = job.config.worker_count();
    smol::block_on(async {
        let first = CycleRunner::new(job, db, &runner, &ex, workers);
        let _cancelled = smol::future::or(async { Some(first.run_iteration().await) }, async {
            smol::Timer::after(std::time::Duration::from_millis(drop_ms)).await;
            None
        })
        .await;
        let second = CycleRunner::new(job, db, &runner, &ex, workers);
        second.run_iteration().await
    })
}

#[test]
fn cancel_during_hook_with_queued_work_next_cycle_clean() {
    let (dir, job, db) = make_job_with_hooks(
        "queue-cancel-hook",
        Some(vec!["u1".to_string(), "u2".to_string()]),
        1,
        r#"
fetch_count = 0
fetched = {}

function override_fetch(request)
    fetch_count = fetch_count + 1
    if fetch_count == 1 then
        sleep(3000)
    end
    table.insert(fetched, request.url)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed, queue.duplicates }
end
"#,
    );
    let job = Arc::new(job);
    // Worker 1 is asleep in override_fetch with u2 still queued; drop at
    // 250ms hits mid-hook. Cycle 1's sleep is interrupted before any insert.
    let stats = run_cancel_then_next_cycle(&job, &db, 250);
    assert_eq!(stats.processed(), 2, "next cycle must drain both urls");
    assert_eq!(stats.failed(), 0);

    smol::block_on(async {
        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        let fetched: Vec<String> = lua.globals().get("fetched").unwrap();
        assert_eq!(fetched, ["u1", "u2"], "only cycle 2 recorded fetches");
        let counts: Vec<usize> = lua.globals().get("final_counts").unwrap();
        assert_eq!(counts, vec![2, 0, 0, 0]);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cancel_with_parked_worker_next_cycle_clean() {
    let (dir, job, db) = make_job_with_hooks(
        "queue-cancel-parked",
        Some(vec!["u1".to_string()]),
        2,
        r#"
fetch_count = 0
fetched = {}

function override_fetch(request)
    fetch_count = fetch_count + 1
    if fetch_count == 1 then
        sleep(3000)
    end
    table.insert(fetched, request.url)
    return {
        status = 200,
        url = request.url,
        headers = {},
        body = "<html></html>",
    }
end

function override_extract(response)
    return {}
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed, queue.duplicates }
end
"#,
    );
    let job = Arc::new(job);
    // Worker 1 sleeps on u1; worker 2 spawns after the 200ms stagger, finds
    // no pending work, and parks — the coordinator sits in recv() awaiting
    // events when the drop lands at 400ms.
    let stats = run_cancel_then_next_cycle(&job, &db, 400);
    assert_eq!(stats.processed(), 1);
    assert_eq!(stats.failed(), 0);

    smol::block_on(async {
        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        let fetched: Vec<String> = lua.globals().get("fetched").unwrap();
        assert_eq!(fetched, ["u1"], "only cycle 2 recorded a fetch");
        let counts: Vec<usize> = lua.globals().get("final_counts").unwrap();
        assert_eq!(counts, vec![1, 0, 0, 0]);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cancel_during_inflight_fetch_next_cycle_clean() {
    let (dir, job, db) = make_job_with_hooks(
        "queue-cancel-fetch",
        None,
        1,
        r#"
_G.cycle = 0

function on_start(queue)
    _G.cycle = _G.cycle + 1
    if _G.cycle == 1 then
        queue.urls = { "http://10.255.255.1:81/" }
    else
        queue.urls = { "u2" }
    end
end

function before_fetch(request)
    if _G.cycle > 1 then
        return nil
    end
    return request
end

function on_finished(queue)
    _G.final_counts = { queue.processed, queue.failed, queue.pushed, queue.duplicates }
end
"#,
    );
    let job = Arc::new(job);
    // Cycle 1's real fetch runs via smol::unblock against a blackhole address;
    // dropping the cycle future abandons the fetch future (the blocking
    // closure completes on the pool harmlessly). Cycle 2 nil-skips instead.
    let stats = run_cancel_then_next_cycle(&job, &db, 400);
    assert_eq!(stats.processed(), 1, "cycle 2 nil-skip counts as processed");
    assert_eq!(stats.failed(), 0);

    smol::block_on(async {
        let lua = job.hooks.as_ref().unwrap().lua.lock().await;
        let cycle: usize = lua.globals().get("cycle").unwrap();
        assert_eq!(cycle, 2, "both cycles started");
        let counts: Vec<usize> = lua.globals().get("final_counts").unwrap();
        assert_eq!(counts, vec![1, 0, 0, 0]);
    });

    drop(job);
    let _ = fs::remove_dir_all(&dir);
}
