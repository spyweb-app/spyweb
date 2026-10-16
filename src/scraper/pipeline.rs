use crate::scraper::coordinator::{Message, QueueState};
use crate::{
    color, config::types::Job, lua::hooks::TelemetryHandle, scraper::request::RequestConfig,
    scraper::runner::Runner, services::db::Db, services::notifier, services::webhook,
};
use anyhow::Result;
use smol::channel::{Receiver, Sender, unbounded};
use smol::{Executor, Timer};
use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

pub(crate) type ThreadProbe = Option<Arc<Mutex<HashSet<ThreadId>>>>;

const WORKER_STAGGER_MS: u64 = 200;

pub struct CycleStats {
    processed: AtomicUsize,
    failed: AtomicUsize,
    pushed: AtomicUsize,
    duplicates: AtomicUsize,
}

impl CycleStats {
    pub fn new() -> Self {
        Self {
            processed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            pushed: AtomicUsize::new(0),
            duplicates: AtomicUsize::new(0),
        }
    }

    pub(crate) fn record(&self, ok: bool) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_pushed(&self) {
        self.pushed.fetch_add(1, Ordering::Relaxed);
    }

    fn record_duplicate(&self) {
        self.duplicates.fetch_add(1, Ordering::Relaxed);
    }

    pub fn processed(&self) -> usize {
        self.processed.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn pushed(&self) -> usize {
        self.pushed.load(Ordering::Relaxed)
    }

    pub fn duplicates(&self) -> usize {
        self.duplicates.load(Ordering::Relaxed)
    }
}

impl Default for CycleStats {
    fn default() -> Self {
        Self::new()
    }
}

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

pub(crate) async fn compute_cycle_batch(job: &Job) -> Option<Vec<String>> {
    let seed = if job.config.has_urls() {
        job.config.options.urls.clone()
    } else {
        None
    };
    match job.hooks.as_ref() {
        Some(hooks) if hooks.has_on_start() => hooks.run_on_start(seed).await,
        _ => seed,
    }
}

pub struct JobCtx<'a> {
    job: &'a Job,
    db: &'a Arc<Db>,
    runner: &'a Arc<Runner>,
}

impl<'a> JobCtx<'a> {
    pub fn new(job: &'a Job, db: &'a Arc<Db>, runner: &'a Arc<Runner>) -> Self {
        Self { job, db, runner }
    }

    // One request lifecycle: ctx → telemetry → stages → finalize → on_success/on_error/on_finally → cleanup.
    pub async fn run_once(&self, base_request: &RequestConfig, worker_id: usize) -> Result<()> {
        let job = self.job;
        let ctx = match job.hooks.as_ref() {
            Some(h) => Some(h.new_cycle_context(worker_id).await?),
            None => None,
        };
        let tel = TelemetryHandle::new(job.hooks.as_ref(), ctx.as_ref());
        let started = std::time::Instant::now();
        tel.init().await?;
        let result = self.stages(base_request, &tel).await;
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

    // Stage pipeline: prepare → fetch → extract → filter → store → emit.
    // Each `None` return == a stage skipping the rest of the cycle (`Ok(())`).
    pub(crate) async fn stages(
        &self,
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
                    let runner = Arc::clone(self.runner);
                    let config = self.job.config.clone();
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
                color::c_job(&self.job.config.name),
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
                        let runner = Arc::clone(self.runner);
                        let config = self.job.config.clone();
                        let dir = self.job.dir.clone();
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
                            color::c_job(&self.job.config.name),
                        );
                    } else {
                        crate::t_warnln!(
                            "Job [{}]: Selector '{}' matched 0 elements. The site may have changed or you have wrong selector",
                            color::c_job(&self.job.config.name),
                            self.job.config.selector
                        );
                    }
                } else {
                    crate::t_println!(
                        "Job [{}]: {} items found by selector, but none matched your keywords",
                        color::c_job(&self.job.config.name),
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
                            self.job.config.options.keywords.as_deref(),
                            self.job.config.options.search_fields.as_deref(),
                        );
                        filtered.push(kept);
                    }
                }
                filtered
            }
            _ => self.runner.keyword_filter(&self.job.config, items),
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
                    let db = Arc::clone(self.db);
                    let config = self.job.config.clone();
                    move || db.batch_check_and_insert(&config, items)
                }),
            )
            .await?;

        if new_items.is_empty() {
            if status_code == 200 {
                crate::t_println!(
                    "Job [{}] no new items found",
                    color::c_job(&self.job.config.name)
                );
            }

            return Ok(());
        }

        crate::t_println!(
            "Found {} for {}",
            color::c_ok(&format!("{} new items", new_items.len())),
            color::c_job(&self.job.config.name)
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
                    let config = self.job.config.clone();
                    let items = items.clone();
                    move || notifier::trigger_notification(&config, &items)
                }),
            )
            .await?;
        }

        let payload = webhook::build_default_payload(&self.job.config.name, &new_items);
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

        tel.stage(
            "webhook",
            webhook::trigger_webhook(&self.job.config, payload),
        )
        .await?;

        Ok(())
    }
}

// All scheduling decisions for one cycle, mode derived from batch presence (empty batch = zero work).
pub(crate) enum Work {
    Queue(Vec<String>),
    Multi,
    Inline(RequestConfig),
    Idle,
}

impl Work {
    pub(crate) async fn plan(job: &Job, worker_count: usize) -> Work {
        match compute_cycle_batch(job).await {
            Some(b) if !b.is_empty() => Work::Queue(b),
            Some(_) => Work::Idle,
            _ if worker_count > 1 => Work::Multi,
            None => Work::Inline(RequestConfig::from_job(&job.config)),
        }
    }
}

enum ErrStyle {
    Inline,
    Worker,
}

async fn run_recorded(
    job: &Job,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    request: RequestConfig,
    worker_id: usize,
    stats: &CycleStats,
    style: ErrStyle,
) -> bool {
    let ctx = JobCtx::new(job, db, runner);
    match ctx.run_once(&request, worker_id).await {
        Ok(()) => {
            stats.record(true);
            true
        }
        Err(e) => {
            stats.record(false);
            match style {
                ErrStyle::Inline => {
                    crate::t_eprintln!("Job '{}' error: {}", color::c_job(&job.config.name), e);
                }
                ErrStyle::Worker => {
                    crate::t_eprintln!("Job '{}' worker error: {}", job.config.name, e);
                }
            }
            false
        }
    }
}

// One cycle's dispatch mechanics: plan → workers → stats.
pub(crate) struct CycleRunner<'a> {
    job: &'a Arc<Job>,
    db: &'a Arc<Db>,
    runner: &'a Arc<Runner>,
    ex: &'a Arc<Executor<'static>>,
    worker_count: usize,
    thread_probe: ThreadProbe,
}

impl<'a> CycleRunner<'a> {
    pub(crate) fn new(
        job: &'a Arc<Job>,
        db: &'a Arc<Db>,
        runner: &'a Arc<Runner>,
        ex: &'a Arc<Executor<'static>>,
        worker_count: usize,
    ) -> Self {
        Self {
            job,
            db,
            runner,
            ex,
            worker_count,
            thread_probe: None,
        }
    }

    // Test seam: record which executor threads workers run on.
    #[cfg(test)]
    pub(crate) fn with_probe(mut self, probe: ThreadProbe) -> Self {
        self.thread_probe = probe;
        self
    }

    // Run exactly one job iteration: plan → dispatch → `on_finished`.
    pub(crate) async fn run_iteration(&self) -> Arc<CycleStats> {
        // Every cycle starts with no active queue cycle — covers all modes
        // and makes a cancelled (dropped) previous cycle safe: its leftover
        // state is cleared before anything user-visible runs.
        if let Some(hooks) = self.job.hooks.as_ref() {
            hooks.queue.deactivate();
        }
        let work = Work::plan(self.job, self.worker_count).await;
        let stats = self.dispatch(work).await;

        if let Some(hooks) = self.job.hooks.as_ref() {
            hooks.run_on_finished(&stats).await;
        }

        stats
    }

    // Total match on `Work`; stats always created here so `on_finished` fires (with zeros) on every path, including Idle.
    pub(crate) async fn dispatch(&self, work: Work) -> Arc<CycleStats> {
        let stats = Arc::new(CycleStats::new());
        match work {
            Work::Idle => {}
            Work::Inline(request) => {
                run_recorded(
                    self.job,
                    self.db,
                    self.runner,
                    request,
                    1,
                    &stats,
                    ErrStyle::Inline,
                )
                .await;
            }
            Work::Queue(batch) => {
                let queue = self.job.hooks.as_ref().map(|h| Arc::clone(&h.queue));
                self.run_queue_cycle(batch, stats.clone(), queue).await;
            }
            Work::Multi => {
                self.spawn_multi_workers(&stats).await;
            }
        }
        stats
    }

    // Stagger → spawn → join for `Multi`: each worker runs the base request once.
    async fn spawn_multi_workers(&self, stats: &Arc<CycleStats>) {
        let mut handles = Vec::with_capacity(self.worker_count);
        for i in 1..=self.worker_count {
            if i > 1 {
                Timer::after(Duration::from_millis(WORKER_STAGGER_MS)).await;
            }
            let worker_id = i;
            let job = Arc::clone(self.job);
            let db = Arc::clone(self.db);
            let runner = Arc::clone(self.runner);
            let stats = Arc::clone(stats);
            handles.push(spawn_worker(
                self.ex,
                async move {
                    let request = RequestConfig::from_job(&job.config);
                    run_recorded(
                        &job,
                        &db,
                        &runner,
                        request,
                        worker_id,
                        &stats,
                        ErrStyle::Worker,
                    )
                    .await;
                },
                &self.thread_probe,
            ));
        }

        for handle in handles {
            handle.await;
        }
    }

    // Queue-cycle engine: pending deque, visited set, in-flight count, worker
    // handles. Inline in the cycle future — dropping it cancels workers.
    async fn run_queue_cycle(
        &self,
        batch: Vec<String>,
        stats: Arc<CycleStats>,
        queue: Option<Arc<QueueState>>,
    ) {
        if self.worker_count == 0 {
            return;
        }

        let (event_tx, event_rx) = unbounded::<Message>();
        let mut pending: VecDeque<String> = batch.clone().into();
        // Seed visited with the initial batch: re-pushing an initial URL is a
        // duplicate; duplicates *within* the initial batch keep processing
        // twice (today's behavior — the visited set gates pushes only).
        let mut visited: HashSet<String> = batch.iter().cloned().collect();
        if let Some(q) = &queue {
            q.begin_cycle(event_tx.clone(), visited.len());
        }

        let mut replies: Vec<Sender<Reply>> = Vec::with_capacity(self.worker_count);
        let mut parked: VecDeque<usize> = VecDeque::new();
        let mut in_flight: usize = 0;
        let mut handles = Vec::with_capacity(self.worker_count);

        for i in 1..=self.worker_count {
            if i > 1 {
                Timer::after(Duration::from_millis(WORKER_STAGGER_MS)).await;
            }
            let (reply_tx, reply_rx) = unbounded::<Reply>();
            replies.push(reply_tx);
            handles.push(spawn_worker(
                self.ex,
                worker_loop(
                    Arc::clone(self.job),
                    Arc::clone(self.db),
                    Arc::clone(self.runner),
                    Arc::clone(&stats),
                    event_tx.clone(),
                    reply_rx,
                    i,
                ),
                &self.thread_probe,
            ));
        }

        while let Ok(msg) = event_rx.recv().await {
            match msg {
                Message::RequestWork { worker_id } => {
                    if let Some(url) = pending.pop_front() {
                        in_flight += 1;
                        let _ = replies[worker_id - 1].try_send(Reply::Work(url));
                    } else if in_flight == 0 {
                        // Quiescence: nothing pending, nothing running.
                        break;
                    } else {
                        // No work now, but requests are in flight — the
                        // worker waits; a Push will assign directly, or the
                        // last Completed re-checks quiescence.
                        parked.push_back(worker_id);
                    }
                }
                Message::Push { url } => {
                    if !visited.insert(url.clone()) {
                        stats.record_duplicate();
                        if let Some(q) = &queue {
                            q.release_reservation();
                        }
                        continue;
                    }
                    stats.record_pushed();
                    if let Some(worker_id) = parked.pop_front() {
                        in_flight += 1;
                        let _ = replies[worker_id - 1].try_send(Reply::Work(url));
                    } else {
                        pending.push_back(url);
                    }
                }
                Message::Completed | Message::Failed => {
                    in_flight -= 1;
                    if pending.is_empty() && in_flight == 0 {
                        break;
                    }
                }
            }
        }

        for tx in &replies {
            let _ = tx.try_send(Reply::Shutdown);
        }
        for handle in handles {
            handle.await;
        }
        if let Some(q) = &queue {
            q.deactivate();
        }
    }
}

// Queue-cycle→worker replies, one unbounded channel per worker.
enum Reply {
    Work(String),
    Shutdown,
}

async fn worker_loop(
    job: Arc<Job>,
    db: Arc<Db>,
    runner: Arc<Runner>,
    stats: Arc<CycleStats>,
    event_tx: Sender<Message>,
    reply_rx: Receiver<Reply>,
    worker_id: usize,
) {
    loop {
        if event_tx
            .try_send(Message::RequestWork { worker_id })
            .is_err()
        {
            return;
        }
        match reply_rx.recv().await {
            Ok(Reply::Work(url)) => {
                let request = RequestConfig {
                    url,
                    ..RequestConfig::from_job(&job.config)
                };
                let ok = run_recorded(
                    &job,
                    &db,
                    &runner,
                    request,
                    worker_id,
                    &stats,
                    ErrStyle::Worker,
                )
                .await;
                let msg = if ok {
                    Message::Completed
                } else {
                    Message::Failed
                };
                if event_tx.try_send(msg).is_err() {
                    return;
                }
            }
            Ok(Reply::Shutdown) | Err(_) => return,
        }
    }
}

pub async fn run_job_loop(job: Job, db: Arc<Db>, runner: Arc<Runner>, ex: &Arc<Executor<'static>>) {
    let interval = Duration::from_secs(job.config.options.interval as u64);
    let worker_count = job.config.worker_count();
    let job = Arc::new(job);
    let cycle = CycleRunner::new(&job, &db, &runner, ex, worker_count);

    loop {
        crate::t_println!("Running job: {}", color::c_job(&job.config.name));
        let started = std::time::Instant::now();

        cycle.run_iteration().await;

        let pipeline_elapsed = started.elapsed();
        crate::t_println!(
            "Job [{}] finished in {}, sleeping for {}",
            color::c_job(&job.config.name),
            color::c_info(&crate::services::utils::format_duration(pipeline_elapsed)),
            color::c_info(&format!("{}s", job.config.options.interval))
        );
        Timer::after(interval).await;
    }
}
