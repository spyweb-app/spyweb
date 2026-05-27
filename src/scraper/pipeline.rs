use crate::{
    color, config::types::Job, scraper::request::RequestConfig, scraper::runner::Runner,
    services::db::Db, services::notifier, services::webhook,
};
use anyhow::Result;
use smol::Timer;
use std::{sync::Arc, time::Duration};

pub async fn run_job_loop(job: Job, db: Arc<Db>, runner: Arc<Runner>) {
    let interval = Duration::from_secs(job.config.interval as u64);
    let base_request = RequestConfig::from_job(&job.config);

    loop {
        crate::t_println!("Running job: {}", color::c_job(&job.config.name));
        let started = std::time::Instant::now();

        let result = run_once(&job, &db, &runner, &base_request).await;
        let pipeline_elapsed = started.elapsed();
        let cleanup_started = std::time::Instant::now();

        let ran_cycle_cleanup = job.hooks.as_ref().is_some_and(|h| h.has_cycle_cleanup());

        if let Some(h) = job.hooks.as_ref() {
            match &result {
                Ok(()) => h.run_on_success().await,
                Err(e) => h.run_on_error(e).await,
            }
            h.run_on_finally().await;
            h.cleanup_cycle_state().await;
        }

        let cleanup_elapsed = cleanup_started.elapsed();

        if let Err(e) = result {
            crate::t_eprintln!("Job '{}' error: {}", color::c_job(&job.config.name), e);
        }

        let cleanup_msg = if ran_cycle_cleanup {
            format!(
                ", cleanup finished in {}",
                color::c_info(&crate::services::utils::format_duration(cleanup_elapsed))
            )
        } else {
            String::new()
        };

        crate::t_println!(
            "Job [{}] finished in {}{}, sleeping for {}",
            color::c_job(&job.config.name),
            color::c_info(&crate::services::utils::format_duration(pipeline_elapsed)),
            cleanup_msg,
            color::c_info(&format!("{}s", job.config.interval))
        );
        Timer::after(interval).await;
    }
}

async fn telemetry_stage_start(job: &Job) -> Result<Option<crate::lua::hooks::TelemetrySample>> {
    match job.hooks.as_ref() {
        Some(h) => Ok(Some(h.telemetry_stage_start().await?)),
        None => Ok(None),
    }
}

async fn record_telemetry_stage(
    job: &Job,
    name: &str,
    sample: Option<crate::lua::hooks::TelemetrySample>,
    status: &str,
    error: Option<String>,
) {
    if let (Some(h), Some(sample)) = (job.hooks.as_ref(), sample) {
        let _ = h.record_telemetry_stage(name, sample, status, error).await;
    }
}

async fn run_once(
    job: &Job,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    base_request: &RequestConfig,
) -> Result<()> {
    let started = std::time::Instant::now();

    if let Some(h) = job.hooks.as_ref() {
        h.init_telemetry().await?;
    }

    let result = run_once_inner(job, db, runner, base_request).await;

    if let Some(h) = job.hooks.as_ref() {
        let _ = h.finalize_telemetry(started.elapsed()).await;
    }

    result
}

async fn run_once_inner(
    job: &Job,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    base_request: &RequestConfig,
) -> Result<()> {
    let request = match job.hooks.as_ref() {
        Some(h) => match h.before_fetch(base_request.clone()).await? {
            None => return Ok(()),
            Some(r) => r,
        },
        None => base_request.clone(),
    };

    let fetch_attempt = match job.hooks.as_ref().filter(|h| h.has_override_fetch()) {
        Some(h) => h.override_fetch(request.clone()).await?,
        None => {
            let sample = telemetry_stage_start(job).await?;
            let attempt = smol::unblock({
                let runner = Arc::clone(runner);
                let config = job.config.clone();
                let request = request.clone();
                move || runner.fetch(&config, &request)
            })
            .await;
            if let Some(sample) = sample {
                if let Some(h) = job.hooks.as_ref() {
                    h.set_last_fetch(&attempt).await?;
                }
                let (status, error) = match &attempt.result {
                    Ok(_) => ("success", None),
                    Err(err) => ("error", Some(err.clone())),
                };
                record_telemetry_stage(job, "fetch", Some(sample), status, error).await;
            }
            attempt
        }
    };

    let response = match job.hooks.as_ref() {
        Some(h) => match h.after_fetch(fetch_attempt).await? {
            None => return Ok(()),
            Some(r) => r,
        },
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

    let extraction = match job.hooks.as_ref().filter(|h| h.has_override_extract()) {
        Some(h) => {
            let items = h.override_extract(&response).await?;
            let count = items.len();
            crate::scraper::extractor::ExtractionResult {
                items,
                selector_matches: count,
            }
        }
        None => {
            let sample = telemetry_stage_start(job).await?;
            let result = smol::unblock({
                let runner = Arc::clone(runner);
                let config = job.config.clone();
                let dir = job.dir.clone();
                move || runner.extract(&config, dir.as_deref(), &response)
            })
            .await;
            if let Some(sample) = sample {
                match &result {
                    Ok(_) => {
                        record_telemetry_stage(job, "extract", Some(sample), "success", None).await;
                    }
                    Err(e) => {
                        record_telemetry_stage(
                            job,
                            "extract",
                            Some(sample),
                            "error",
                            Some(e.to_string()),
                        )
                        .await;
                    }
                }
            }
            result?
        }
    };

    if let Some(h) = job.hooks.as_ref() {
        h.set_selector_matches(extraction.selector_matches).await?;
    }

    let items = match job.hooks.as_ref() {
        Some(h) => h.after_extract(extraction.items).await?,
        None => extraction.items,
    };

    let filter_sample = telemetry_stage_start(job).await?;
    let items = match job.hooks.as_ref() {
        Some(h) if h.has_filter_item() => {
            let mut filtered = Vec::new();
            for item in items {
                if let Some(mut kept) = h.filter_item(item).await? {
                    // Even if user filters, we still want the engine to tag them for the DB
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
    if let Some(sample) = filter_sample {
        let error = match job.hooks.as_ref() {
            Some(h) => h.take_filter_error().await,
            None => None,
        };
        let status = if error.is_some() { "error" } else { "success" };
        record_telemetry_stage(job, "filter", Some(sample), status, error).await;
    }

    if items.is_empty() {
        if status_code == 200 {
            if extraction.selector_matches == 0 {
                crate::t_warnln!(
                    "Job [{}]: Selector '{}' matched 0 elements. The site may have changed or you have wrong selector",
                    color::c_job(&job.config.name),
                    job.config.selector
                );
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

    let items = match job.hooks.as_ref() {
        Some(h) => match h.before_store(items).await? {
            None => return Ok(()),
            Some(i) => i,
        },
        None => items,
    };

    let store_sample = telemetry_stage_start(job).await?;
    let store_result = smol::unblock({
        let db = Arc::clone(db);
        let config = job.config.clone();
        move || db.batch_check_and_insert(&config, items)
    })
    .await;
    if let Some(sample) = store_sample {
        match &store_result {
            Ok(_) => {
                record_telemetry_stage(job, "store", Some(sample), "success", None).await;
            }
            Err(e) => {
                record_telemetry_stage(job, "store", Some(sample), "error", Some(e.to_string()))
                    .await;
            }
        }
    }
    let new_items = store_result?;

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

    let notify_items = match job.hooks.as_ref() {
        Some(h) => h.before_notify(new_items.clone()).await?,
        None => Some(new_items.clone()),
    };

    if let Some(items) = notify_items {
        let notify_sample = telemetry_stage_start(job).await?;
        let notify_result = smol::unblock({
            let config = job.config.clone();
            let items = items.clone();
            move || notifier::trigger_notification(&config, &items)
        })
        .await;
        if let Some(sample) = notify_sample {
            match &notify_result {
                Ok(true) | Ok(false) => {
                    record_telemetry_stage(job, "notify", Some(sample), "success", None).await;
                }
                Err(e) => {
                    record_telemetry_stage(
                        job,
                        "notify",
                        Some(sample),
                        "error",
                        Some(e.to_string()),
                    )
                    .await;
                }
            }
        }
    }

    let payload = webhook::build_default_payload(&job.config.name, &new_items);
    let payload = match job.hooks.as_ref() {
        Some(h) => match h.before_webhook(payload).await? {
            None => return Ok(()),
            Some(p) => p,
        },
        None => payload,
    };

    let webhook_sample = telemetry_stage_start(job).await?;
    let webhook_result = webhook::trigger_webhook(&job.config, payload).await;
    if let Some(sample) = webhook_sample {
        match &webhook_result {
            Ok(true) | Ok(false) => {
                record_telemetry_stage(job, "webhook", Some(sample), "success", None).await;
            }
            Err(e) => {
                record_telemetry_stage(job, "webhook", Some(sample), "error", Some(e.to_string()))
                    .await;
            }
        }
    }
    webhook_result?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Field, JobConfig};
    use crate::lua::hooks::JobHooks;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("spyweb-{name}-{}-{nanos}", std::process::id()))
    }

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
            },
            hooks: Some(hooks),
            dir: Some(dir.clone()),
        };
        let runner = Arc::new(Runner::new());

        smol::block_on(async {
            let base_request = RequestConfig::from_job(&job.config);
            run_once(&job, &db, &runner, &base_request).await.unwrap();

            let hooks = job.hooks.as_ref().unwrap();
            let lua = hooks.lua.lock().await;
            let telemetry: mlua::Table = lua.globals().get("spyweb_telemetry").unwrap();
            let map: mlua::Table = telemetry.get("map").unwrap();

            assert!(telemetry.get::<f64>("total_duration_ms").unwrap() >= 0.0);
            assert!(matches!(
                lua.globals().get::<mlua::Value>("last_fetch").unwrap(),
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
                "success"
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
}
