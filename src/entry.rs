use anyhow::Result;
use smol::{Executor, Timer};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::config::loader;
use crate::config::types::Jobs;
use crate::scraper::pipeline;
use crate::scraper::runner::Runner;
use crate::services::db::Db;
use crate::services::server::WebServer;
use crate::services::watcher;

pub fn start_app() -> Result<()> {
    start_app_with_port(None)
}

pub fn start_app_with_port(port_override: Option<u16>) -> Result<()> {
    crate::services::io::init();

    ctrlc::set_handler(move || {
        crate::t_println!("\nShutting down gracefully...");
        crate::services::io::shutdown();
        // Give it a moment to drain
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::exit(0);
    })
    .expect("Error setting Ctrl-C handler");

    let db = Arc::new(Db::open("data")?);
    let jobs = loader::load_all_jobs("jobs.toml", "jobs", Arc::clone(&db))?;
    let active_job_ids = Arc::new(std::sync::RwLock::new(
        jobs.list
            .iter()
            .map(|j| j.config.id())
            .collect::<Vec<String>>(),
    ));

    let ex = Arc::new(Executor::new());
    let (_s, r) = smol::channel::unbounded::<()>();

    for _ in 0..2 {
        let ex = Arc::clone(&ex);
        let s = r.clone();
        thread::spawn(move || smol::block_on(ex.run(s.recv())));
    }

    let server_db = Arc::clone(&db);
    let server_active_jobs = Arc::clone(&active_job_ids);
    ex.spawn(async move {
        let server = WebServer::new(server_db, server_active_jobs);
        let addr = crate::config::get_base_url_with_override(port_override);
        if let Err(e) = server.listen(&addr) {
            crate::t_eprintln!("Server error: {}", e);
        }
    })
    .detach();

    let (reload_tx, reload_rx) = smol::channel::unbounded::<()>();
    thread::spawn(move || {
        if let Err(e) = watcher::watch_configs(reload_tx) {
            crate::t_eprintln!("Watcher error: {}", e);
        }
    });

    let runner = Arc::new(Runner::new());
    let ex_manager: Arc<Executor<'static>> = Arc::clone(&ex);
    let manager_active_jobs = Arc::clone(&active_job_ids);
    ex.spawn(async move {
        job_manager(ex_manager, runner, db, jobs, reload_rx, manager_active_jobs).await;
    })
    .detach();

    smol::block_on(smol::future::pending::<()>());
    Ok(())
}

fn spawn_jobs(
    ex: &Arc<Executor<'static>>,
    runner: &Arc<Runner>,
    db: &Arc<Db>,
    jobs: Jobs,
) -> Vec<smol::Task<()>> {
    jobs.list
        .into_iter()
        .filter(|job| job.config.enabled)
        .map(|job| {
            let runner = Arc::clone(runner);
            let db = Arc::clone(db);
            ex.spawn(async move {
                pipeline::run_job_loop(job, db, runner).await;
            })
        })
        .collect()
}

async fn job_manager(
    ex: Arc<Executor<'static>>,
    runner: Arc<Runner>,
    db: Arc<Db>,
    initial_jobs: Jobs,
    reload_rx: smol::channel::Receiver<()>,
    active_job_ids: Arc<std::sync::RwLock<Vec<String>>>,
) {
    let mut handles = spawn_jobs(&ex, &runner, &db, initial_jobs);

    loop {
        if reload_rx.recv().await.is_err() {
            break;
        }

        Timer::after(Duration::from_millis(200)).await;
        while reload_rx.try_recv().is_ok() {}

        crate::t_println!("Reloading config...");

        match loader::load_all_jobs("jobs.toml", "jobs", Arc::clone(&db)) {
            Ok(new_jobs) => {
                drop(handles);

                if let Ok(mut lock) = active_job_ids.write() {
                    *lock = new_jobs.list.iter().map(|j| j.config.id()).collect();
                }
                handles = spawn_jobs(&ex, &runner, &db, new_jobs);
                crate::t_println!(
                    "{} {} jobs",
                    crate::color::c_ok("Reloaded"),
                    crate::color::c_info(&handles.len().to_string())
                );
            }
            Err(e) => {
                crate::t_eprintln!("Config reload failed: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Job, JobConfig, Jobs};
    use tempfile::TempDir;

    fn make_job(name: &str, enabled: bool) -> Job {
        Job {
            config: JobConfig {
                name: name.to_string(),
                url: "http://example.com".to_string(),
                selector: "div.item".to_string(),
                fields: vec![],
                keywords: None,
                search_fields: None,
                webhook: None,
                debug: false,
                enabled,
                interval: 60,
                proxy: None,
                notification: None,
                headers: None,
                hash_fields: None,
            },
            hooks: None,
            dir: None,
        }
    }

    fn make_temp_db() -> (TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        let path_str = path.to_str().unwrap();
        let db = Db::open(path_str).unwrap();
        (dir, db)
    }

    #[test]
    fn test_spawn_jobs_filters_disabled() {
        let (_dir, db) = make_temp_db();
        let ex = Arc::new(Executor::new());
        let runner = Arc::new(Runner::new());
        let db = Arc::new(db);

        let jobs = Jobs {
            list: vec![
                make_job("enabled_job_1", true),
                make_job("disabled_job", false),
                make_job("enabled_job_2", true),
            ],
        };

        let handles = spawn_jobs(&ex, &runner, &db, jobs);

        assert_eq!(handles.len(), 2);
        drop(handles);
    }

    #[test]
    fn test_spawn_jobs_all_disabled() {
        let (_dir, db) = make_temp_db();
        let ex = Arc::new(Executor::new());
        let runner = Arc::new(Runner::new());
        let db = Arc::new(db);

        let jobs = Jobs {
            list: vec![
                make_job("disabled_job_1", false),
                make_job("disabled_job_2", false),
            ],
        };

        let handles = spawn_jobs(&ex, &runner, &db, jobs);

        assert_eq!(handles.len(), 0);
        drop(handles);
    }

    #[test]
    fn test_spawn_jobs_all_enabled() {
        let (_dir, db) = make_temp_db();
        let ex = Arc::new(Executor::new());
        let runner = Arc::new(Runner::new());
        let db = Arc::new(db);

        let jobs = Jobs {
            list: vec![
                make_job("enabled_job_1", true),
                make_job("enabled_job_2", true),
                make_job("enabled_job_3", true),
            ],
        };

        let handles = spawn_jobs(&ex, &runner, &db, jobs);

        assert_eq!(handles.len(), 3);
        drop(handles);
    }

    #[test]
    fn test_drop_handles_cancels_tasks() {
        let (_dir, db) = make_temp_db();
        let ex = Arc::new(Executor::new());
        let runner = Arc::new(Runner::new());
        let db = Arc::new(db);

        let jobs = Jobs {
            list: vec![make_job("test_job", true)],
        };

        let handles = spawn_jobs(&ex, &runner, &db, jobs);

        let handle = handles.into_iter().next().unwrap();
        drop(handle);

        let jobs2 = Jobs { list: vec![] };
        let handles2 = spawn_jobs(&ex, &runner, &db, jobs2);
        assert_eq!(handles2.len(), 0);
    }
}
