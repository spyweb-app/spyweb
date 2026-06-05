use crate::{
    color, config::types::Job, lua::hooks::TelemetryHandle, scraper::request::RequestConfig,
    scraper::runner::Runner, services::db::Db, services::notifier, services::webhook,
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

        if let Err(e) = result {
            crate::t_eprintln!("Job '{}' error: {}", color::c_job(&job.config.name), e);
        }

        crate::t_println!(
            "Job [{}] finished in {}, sleeping for {}",
            color::c_job(&job.config.name),
            color::c_info(&crate::services::utils::format_duration(pipeline_elapsed)),
            color::c_info(&format!("{}s", job.config.interval))
        );
        Timer::after(interval).await;
    }
}

async fn run_once(
    job: &Job,
    db: &Arc<Db>,
    runner: &Arc<Runner>,
    base_request: &RequestConfig,
) -> Result<()> {
    let ctx = match job.hooks.as_ref() {
        Some(h) => Some(h.new_cycle_context().await?),
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
                .new_cycle_context()
                .await
                .unwrap();
            let tel = TelemetryHandle::new(job.hooks.as_ref(), Some(&ctx));
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
