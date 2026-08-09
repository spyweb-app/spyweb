use crate::{
    color, config::types::Job, lua::hooks::TelemetryHandle, scraper::request::RequestConfig,
    scraper::runner::Runner, services::db::Db, services::notifier, services::webhook,
};
use anyhow::Result;
use smol::{Executor, Timer};
use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

type UrlQueue = Arc<Mutex<VecDeque<String>>>;

pub(crate) type ThreadProbe = Option<Arc<Mutex<HashSet<ThreadId>>>>;

const WORKER_STAGGER_MS: u64 = 200;

fn spawn_worker<F>(
    ex: &Arc<Executor<'static>>,
    future: F,
    thread_probe: &ThreadProbe,
) -> smol::Task<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let probe = thread_probe.clone();
    ex.spawn(async move {
        if let Some(p) = probe.as_ref() {
            p.lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(std::thread::current().id());
        }
        future.await;
    })
}

pub async fn run_job_loop(job: Job, db: Arc<Db>, runner: Arc<Runner>, ex: &Arc<Executor<'static>>) {
    let interval = Duration::from_secs(job.config.interval as u64);
    let worker_count = job.config.worker_count();
    let has_urls = job.config.has_urls();
    let job = Arc::new(job);

    loop {
        crate::t_println!("Running job: {}", color::c_job(&job.config.name));
        let started = std::time::Instant::now();

        if has_urls {
            run_urls_cycle(Arc::clone(&job), &db, &runner, worker_count, ex, None).await;
        } else if worker_count > 1 {
            run_multi_cycle(Arc::clone(&job), &db, &runner, worker_count, ex, None).await;
        } else {
            let base_request = RequestConfig::from_job(&job.config);
            if let Err(e) = run_once(&job, &db, &runner, &base_request, 1).await {
                crate::t_eprintln!("Job '{}' error: {}", color::c_job(&job.config.name), e);
            }
        }

        if let Some(hooks) = job.hooks.as_ref() {
            hooks.run_on_finished().await;
        }

        let pipeline_elapsed = started.elapsed();
        crate::t_println!(
            "Job [{}] finished in {}, sleeping for {}",
            color::c_job(&job.config.name),
            color::c_info(&crate::services::utils::format_duration(pipeline_elapsed)),
            color::c_info(&format!("{}s", job.config.interval))
        );
        Timer::after(interval).await;
    }
}

pub(crate) async fn run_urls_cycle(
    job: Arc<Job>,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    worker_count: usize,
    ex: &Arc<Executor<'static>>,
    thread_probe: ThreadProbe,
) {
    let queue: UrlQueue = Arc::new(Mutex::new(
        job.config.urls.clone().unwrap().into_iter().collect(),
    ));

    let mut handles = Vec::with_capacity(worker_count);
    for i in 1..=worker_count {
        if i > 1 {
            Timer::after(Duration::from_millis(WORKER_STAGGER_MS)).await;
        }
        let worker_id = i;
        let job = Arc::clone(&job);
        let db = Arc::clone(db);
        let runner = Arc::clone(runner);
        let queue = Arc::clone(&queue);
        handles.push(spawn_worker(
            ex,
            async move {
                loop {
                    let url = { queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front() };
                    let url = match url {
                        Some(u) => u,
                        None => return,
                    };
                    let request = RequestConfig {
                        url,
                        ..RequestConfig::from_job(&job.config)
                    };
                    if let Err(e) = run_once(&job, &db, &runner, &request, worker_id).await {
                        crate::t_eprintln!("Job '{}' worker error: {}", job.config.name, e);
                    }
                }
            },
            &thread_probe,
        ));
    }

    for handle in handles {
        handle.await;
    }
}

pub(crate) async fn run_multi_cycle(
    job: Arc<Job>,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    worker_count: usize,
    ex: &Arc<Executor<'static>>,
    thread_probe: ThreadProbe,
) {
    let base_request = RequestConfig::from_job(&job.config);

    let mut handles = Vec::with_capacity(worker_count);
    for i in 1..=worker_count {
        if i > 1 {
            Timer::after(Duration::from_millis(WORKER_STAGGER_MS)).await;
        }
        let worker_id = i;
        let job = Arc::clone(&job);
        let db = Arc::clone(db);
        let runner = Arc::clone(runner);
        let request = base_request.clone();
        handles.push(spawn_worker(
            ex,
            async move {
                if let Err(e) = run_once(&job, &db, &runner, &request, worker_id).await {
                    crate::t_eprintln!("Job '{}' worker error: {}", job.config.name, e);
                }
            },
            &thread_probe,
        ));
    }

    for handle in handles {
        handle.await;
    }
}

pub async fn run_once(
    job: &Job,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    base_request: &RequestConfig,
    worker_id: usize,
) -> Result<()> {
    let ctx = match job.hooks.as_ref() {
        Some(h) => Some(h.new_cycle_context(worker_id).await?),
        None => None,
    };
    let tel = TelemetryHandle::new(job.hooks.as_ref(), ctx.as_ref());
    let started = std::time::Instant::now();
    tel.init().await?;
    let result = run_once_inner(job, db, runner, base_request, &tel).await;
    tel.finalize(started.elapsed()).await;

    if let (Some(h), Some(ctx)) = (job.hooks.as_ref(), ctx.as_ref()) {
        match &result {
            Ok(()) => h.run_on_success(ctx).await,
            Err(e) => h.run_on_error(ctx, e).await,
        }
        h.run_on_finally(ctx).await;
    }

    tel.cleanup().await;
    result
}

pub(crate) async fn run_once_inner(
    job: &Job,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    base_request: &RequestConfig,
    tel: &TelemetryHandle<'_>,
) -> Result<()> {
    let request = match tel.hooks() {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            match h.before_fetch(base_request.clone(), ctx).await? {
                None => return Ok(()),
                Some(r) => r,
            }
        }
        None => base_request.clone(),
    };

    let fetch_attempt = match tel.hooks().filter(|h| h.has_override_fetch()) {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            h.override_fetch(request.clone(), ctx).await?
        }
        None => {
            let token = tel.start().await;
            let attempt = smol::unblock({
                let runner = Arc::clone(runner);
                let config = job.config.clone();
                let request = request.clone();
                move || runner.fetch(&config, &request)
            })
            .await;
            if let (Some(h), Some(ctx)) = (tel.hooks(), tel.ctx()) {
                h.set_last_fetch(ctx, &attempt).await?;
            }
            let (status, error) = match &attempt.result {
                Ok(_) => ("success", None),
                Err(err) => ("error", Some(err.clone())),
            };
            tel.record("fetch", token, status, error).await;
            attempt
        }
    };

    let response = match tel.hooks() {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            match h.after_fetch(fetch_attempt, ctx).await? {
                None => return Ok(()),
                Some(r) => r,
            }
        }
        None => fetch_attempt.result.map_err(anyhow::Error::msg)?,
    };

    let status_code = response.status;

    if status_code >= 400 {
        crate::t_warnln!(
            "Job {} fetch returned status {}",
            color::c_job(&job.config.name),
            color::c_warn(&status_code.to_string())
        );
    }

    let extraction = match tel.hooks().filter(|h| h.has_override_extract()) {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            let items = h.override_extract(&response, ctx).await?;
            let count = items.len();
            crate::scraper::extractor::ExtractionResult {
                items,
                selector_matches: count,
            }
        }
        None => {
            tel.stage(
                "extract",
                smol::unblock({
                    let runner = Arc::clone(runner);
                    let config = job.config.clone();
                    let dir = job.dir.clone();
                    move || runner.extract(&config, dir.as_deref(), &response)
                }),
            )
            .await?
        }
    };

    if let (Some(h), Some(ctx)) = (tel.hooks(), tel.ctx()) {
        h.set_selector_matches(ctx, extraction.selector_matches)
            .await?;
    }

    let items = match tel.hooks() {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            h.after_extract(extraction.items, ctx).await?
        }
        None => extraction.items,
    };

    if items.is_empty() {
        if status_code == 200 {
            if extraction.selector_matches == 0 {
                if tel.hooks().is_some_and(|h| h.has_override_extract()) {
                    crate::t_println!(
                        "Job [{}]: override_extract returned 0 items",
                        color::c_job(&job.config.name),
                    );
                } else {
                    crate::t_warnln!(
                        "Job [{}]: Selector '{}' matched 0 elements. The site may have changed or you have wrong selector",
                        color::c_job(&job.config.name),
                        job.config.selector
                    );
                }
            } else {
                crate::t_println!(
                    "Job [{}]: {} items found by selector, but none matched your keywords",
                    color::c_job(&job.config.name),
                    extraction.selector_matches
                );
            }
        }
        return Ok(());
    }

    let token = tel.start().await;
    let items = match tel.hooks() {
        Some(h) if h.has_filter_item() => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            let mut filtered = Vec::new();
            for item in items {
                if let Some(mut kept) = h.filter_item(item, ctx).await? {
                    kept.matches = crate::scraper::extractor::matching_keywords(
                        &kept.fields,
                        job.config.keywords.as_deref(),
                        job.config.search_fields.as_deref(),
                    );
                    filtered.push(kept);
                }
            }
            filtered
        }
        _ => runner.keyword_filter(&job.config, items),
    };
    let error = match (tel.hooks(), tel.ctx()) {
        (Some(h), Some(ctx)) => h.take_filter_error(ctx).await,
        _ => None,
    };
    let status = if error.is_some() { "error" } else { "success" };
    tel.record("filter", token, status, error).await;

    let items = match tel.hooks() {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            match h.before_store(items, ctx).await? {
                None => return Ok(()),
                Some(i) => i,
            }
        }
        None => items,
    };

    let new_items = tel
        .stage(
            "store",
            smol::unblock({
                let db = Arc::clone(db);
                let config = job.config.clone();
                move || db.batch_check_and_insert(&config, items)
            }),
        )
        .await?;

    if new_items.is_empty() {
        if status_code == 200 {
            crate::t_println!(
                "Job [{}] no new items found",
                color::c_job(&job.config.name)
            );
        }

        return Ok(());
    }

    crate::t_println!(
        "Found {} for {}",
        color::c_ok(&format!("{} new items", new_items.len())),
        color::c_job(&job.config.name)
    );

    let notify_items = match tel.hooks() {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            h.before_notify(new_items.clone(), ctx).await?
        }
        None => Some(new_items.clone()),
    };

    if let Some(items) = notify_items {
        tel.stage(
            "notify",
            smol::unblock({
                let config = job.config.clone();
                let items = items.clone();
                move || notifier::trigger_notification(&config, &items)
            }),
        )
        .await?;
    }

    let payload = webhook::build_default_payload(&job.config.name, &new_items);
    let payload = match tel.hooks() {
        Some(h) => {
            let Some(ctx) = tel.ctx() else { return Ok(()) };
            match h.before_webhook(payload, ctx).await? {
                None => return Ok(()),
                Some(p) => p,
            }
        }
        None => payload,
    };

    tel.stage("webhook", webhook::trigger_webhook(&job.config, payload))
        .await?;

    Ok(())
}
