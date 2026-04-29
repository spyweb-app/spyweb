use crate::{
    config::types::Job, scraper::request::RequestConfig, scraper::runner::Runner, services::db::Db,
    services::notifier, services::webhook,
};
use anyhow::Result;
use smol::Timer;
use std::{sync::Arc, time::Duration};

pub async fn run_job_loop(job: Job, db: Arc<Db>, runner: Arc<Runner>) {
    let interval = Duration::from_secs(job.config.interval as u64);

    loop {
        crate::t_println!("Running job: \x1b[38;2;162;155;254;1m{}\x1b[0m", job.config.name);
        let started = std::time::Instant::now();

        if let Err(e) = run_once(&job, &db, &runner).await {
            crate::t_eprintln!("Job '{}' error: {}", job.config.name, e);
        }

        crate::t_println!(
            "Job [\x1b[38;2;162;155;254;1m{}\x1b[0m] finished in \x1b[38;2;116;185;255m{:.2}s\x1b[0m, sleeping for \x1b[38;2;116;185;255m{}s\x1b[0m",
            job.config.name,
            started.elapsed().as_secs_f64(),
            job.config.interval
        );
        Timer::after(interval).await;
    }
}

async fn run_once(job: &Job, db: &Arc<Db>, runner: &Arc<Runner>) -> Result<()> {
    let request = RequestConfig::from_job(&job.config);
    let request = match job.hooks.as_ref() {
        Some(h) => match h.before_fetch(request).await? {
            None => return Ok(()),
            Some(r) => r,
        },
        None => request,
    };

    let fetch_result = smol::unblock({
        let runner = Arc::clone(runner);
        let config = job.config.clone();
        let request = request.clone();
        move || runner.fetch(&config, &request)
    })
    .await;

    let response = match job.hooks.as_ref() {
        Some(h) => match h.after_fetch(fetch_result).await? {
            None => return Ok(()),
            Some(r) => r,
        },
        None => fetch_result?,
    };

    if response.status >= 400 {
        crate::t_eprintln!(
            "Warning: Job '{}' fetch returned status {}",
            job.config.name,
            response.status
        );
    }

    let items = smol::unblock({
        let runner = Arc::clone(runner);
        let config = job.config.clone();
        let dir = job.dir.clone();
        move || runner.extract(&config, dir.as_deref(), &response)
    })
    .await?;

    let items = match job.hooks.as_ref() {
        Some(h) => h.after_extract(items).await?,
        None => items,
    };

    let items = match job.hooks.as_ref() {
        Some(h) if h.has_filter_item() => {
            let mut filtered = Vec::new();
            for item in items {
                if let Some(kept) = h.filter_item(item).await? {
                    filtered.push(kept);
                }
            }
            filtered
        }
        _ => runner.keyword_filter(&job.config, items),
    };

    if items.is_empty() {
        crate::t_println!(
            "No items extracted for {}, check your config and selectors or hooks",
            job.config.name
        );
        return Ok(());
    }

    let items = match job.hooks.as_ref() {
        Some(h) => match h.before_store(items).await? {
            None => return Ok(()),
            Some(i) => i,
        },
        None => items,
    };

    let new_items = smol::unblock({
        let db = Arc::clone(db);
        let config = job.config.clone();
        move || db.batch_check_and_insert(&config, items)
    })
    .await?;

    if new_items.is_empty() {
        crate::t_println!("Job [\x1b[38;2;162;155;254;1m{}\x1b[0m] no new items found", job.config.name);
        return Ok(());
    }

    crate::t_println!(
        "Found \x1b[38;2;85;239;196;1m{} new items\x1b[0m for \x1b[38;2;162;155;254;1m{}\x1b[0m",
        new_items.len(),
        job.config.name
    );

    let notifiable = match job.hooks.as_ref() {
        Some(h) => match h.before_notify(new_items).await? {
            None => return Ok(()),
            Some(i) => i,
        },
        None => new_items,
    };

    smol::unblock({
        let config = job.config.clone();
        let items = notifiable.clone();
        move || {
            let _ = notifier::trigger_notification(&config, &items);
        }
    })
    .await;

    let payload = webhook::build_default_payload(&job.config.name, &notifiable);
    let payload = match job.hooks.as_ref() {
        Some(h) => match h.before_webhook(payload).await? {
            None => return Ok(()),
            Some(p) => p,
        },
        None => payload,
    };

    webhook::trigger_webhook(&job.config, payload).await?;

    Ok(())
}
