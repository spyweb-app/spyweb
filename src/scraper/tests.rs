use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::types::{Field, Job, JobConfig, Proxy as JobProxy, Rotate};
use crate::lua::hooks::JobHooks;
use crate::scraper::pipeline::{run_multi_cycle, run_once, run_once_inner, run_urls_cycle};
use crate::scraper::request::{
    RequestConfig, RequestHandler, RequestResult, flatten_headers, format_error_chain,
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
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 60,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: None,
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
        run_once_inner(&job, &db, &runner, &base_request, &tel)
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
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 60,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: None,
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    };
    let runner = Arc::new(Runner::new());

    smol::block_on(async {
        let base_request = RequestConfig::from_job(&job.config);
        run_once(&job, &db, &runner, &base_request, 0)
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
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 60,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: Some(vec![
                "https://example.com/1".into(),
                "https://example.com/2".into(),
                "https://example.com/3".into(),
            ]),
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    });
    let runner = Arc::new(Runner::new());

    smol::block_on(async {
        run_urls_cycle(Arc::clone(&job), &db, &runner, 1).await;

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
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 60,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: Some(vec![
                "https://example.com/a".into(),
                "https://example.com/b".into(),
                "https://example.com/c".into(),
                "https://example.com/d".into(),
            ]),
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    });
    let runner = Arc::new(Runner::new());

    smol::block_on(async {
        run_urls_cycle(Arc::clone(&job), &db, &runner, 2).await;

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
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 60,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: Some(2),
            urls: None,
        },
        hooks: Some(hooks),
        has_hooks_file: true,
        dir: Some(dir.clone()),
    });
    let runner = Arc::new(Runner::new());

    smol::block_on(async {
        run_multi_cycle(Arc::clone(&job), &db, &runner, 2).await;

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
        keywords: keywords
            .map(|keywords| keywords.into_iter().map(str::to_string).collect::<Vec<_>>()),
        search_fields: None,
        webhook: None,
        enabled,
        interval: 60,
        debug: false,
        proxy: Some(crate::config::types::Proxy {
            enabled: false,
            rotate: Rotate::RoundRobin,
            urls: vec![],
        }),
        notification: None,
        headers: None,
        hash_fields: None,
        workers: None,
        urls: None,
    }
}

fn runner_response(body: &str) -> RequestResult {
    RequestResult {
        url: "https://example.com/jobs".into(),
        status: 200,
        headers: Default::default(),
        body: body.into(),
        proxy: None,
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
        debug: false,
        keywords: None,
        search_fields: None,
        webhook: None,
        enabled: true,
        interval: 60,
        proxy: Some(JobProxy {
            enabled: true,
            rotate,
            urls: urls.into_iter().map(str::to_string).collect(),
        }),
        notification: None,
        headers: None,
        hash_fields: None,
        workers: None,
        urls: None,
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

    job.proxy = Some(JobProxy {
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
    let Ok(agent) = handler.build_agent(None) else {
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
