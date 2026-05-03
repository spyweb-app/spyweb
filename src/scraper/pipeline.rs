use crate::{
    color,
    config::types::Job, scraper::request::RequestConfig, scraper::runner::Runner, services::db::Db,
    services::notifier, services::webhook,
};
use anyhow::Result;
use smol::Timer;
use std::{sync::Arc, time::Duration};

pub async fn run_job_loop(job: Job, db: Arc<Db>, runner: Arc<Runner>) {
    let interval = Duration::from_secs(job.config.interval as u64);

    loop {
        crate::t_println!("Running job: {}", color::c_job(&job.config.name));
        let started = std::time::Instant::now();

        if let Err(e) = run_once(&job, &db, &runner).await {
            crate::t_eprintln!("Job '{}' error: {}", color::c_job(&job.config.name), e);
        }

        crate::t_println!(
            "Job [{}] finished in {}, sleeping for {}",
            color::c_job(&job.config.name),
            color::c_info(&format!("{:.2}s", started.elapsed().as_secs_f64())),
            color::c_info(&format!("{}s", job.config.interval))
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

    let fetch_attempt = match job.hooks.as_ref().filter(|h| h.has_override_fetch()) {
        Some(h) => h.override_fetch(request.clone()).await?,
        None => {
            smol::unblock({
                let runner = Arc::clone(runner);
                let config = job.config.clone();
                let request = request.clone();
                move || runner.fetch(&config, &request)
            })
            .await
        }
    };

    let response = match job.hooks.as_ref() {
        Some(h) => match h.after_fetch(fetch_attempt.clone()).await? {
            None => return Ok(()),
            Some(r) => r,
        },
        None => fetch_attempt.result.clone().map_err(anyhow::Error::msg)?,
    };

    let status_code = response.status;

    if status_code >= 400 {
        crate::t_warnln!(
            "Job {} fetch returned status {}",
            color::c_job(&job.config.name),
            color::c_warn(&status_code.to_string())
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
        if status_code == 200 {
            crate::t_warnln!(
                "No items extracted for {}, check your config and selectors or hooks",
                color::c_job(&job.config.name)
            );
        }
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
        if status_code == 200 {
            crate::t_println!("Job [{}] no new items found", color::c_job(&job.config.name));
        }

        return Ok(());
    }

    crate::t_println!(
        "Found {} for {}",
        color::c_ok(&format!("{} new items", new_items.len())),
        color::c_job(&job.config.name)
    );

    let notify_items = match job.hooks.as_ref() {
        Some(h) => h.before_notify(new_items.clone()).await?,
        None => Some(new_items.clone()),
    };

    if let Some(items) = notify_items {
        smol::unblock({
            let config = job.config.clone();
            let items = items.clone();
            move || {
                let _ = notifier::trigger_notification(&config, &items);
            }
        })
        .await;
    }

    let payload = webhook::build_default_payload(&job.config.name, &new_items);
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
