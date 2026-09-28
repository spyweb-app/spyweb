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
use crate::services::server::{JobSummary, WebServer};
use crate::services::utils::{Phase, register_shutdown_hook, shutdown_system};
use crate::services::watcher;

pub fn start_app() -> Result<()> {
    start_app_with_port(None, false)
}

pub fn start_app_with_port(port_override: Option<u16>, disable_server: bool) -> Result<()> {
    if let Err(e) = ctrlc::set_handler(move || {
        crate::t_println!("\nShutting down gracefully...");
        shutdown_system();
        std::process::exit(0);
    }) {
        crate::t_warnln!("Warning: Could not set Ctrl-C handler: {}", e);
    }

    let db = Arc::new(Db::open("data")?);
    let jobs = loader::load_all_jobs("jobs.toml", "jobs", Arc::clone(&db))?;

    register_shutdown_hook(Phase::Post, {
        let db = Arc::clone(&db);
        move || {
            if let Err(e) = db.close() {
                crate::t_eprintln!("database close failed: {}", e);
            }
        }
    });

    let (stop_tx, stop_rx) = smol::channel::unbounded::<()>();
    register_shutdown_hook(Phase::Pre, move || {
        let _ = stop_tx.send_blocking(()); // Err = job_manager already gone
    });

    let active_job_ids = Arc::new(std::sync::RwLock::new(
        jobs.list
            .iter()
            .map(|j| JobSummary {
                id: j.config.id(),
                name: j.config.name.clone(),
                enabled: j.config.enabled,
            })
            .collect::<Vec<JobSummary>>(),
    ));

    let ex = Arc::new(Executor::new());
    let (_s, r) = smol::channel::unbounded::<()>();

    let thread_count = std::env::var("SPYWEB_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 64);

    for _ in 0..thread_count {
        let ex = Arc::clone(&ex);
        let s = r.clone();
        thread::spawn(move || smol::block_on(ex.run(s.recv())));
    }

    let server_disabled = disable_server || std::env::var("SPYWEB_DISABLE_SERVER").is_ok();
    if !server_disabled {
        let server_db = Arc::clone(&db);
        let server_active_jobs = Arc::clone(&active_job_ids);
        thread::spawn(move || {
            let server = WebServer::new(server_db, server_active_jobs);
            let addr = crate::config::get_base_url_with_override(port_override);
            if let Err(e) = server.listen(&addr) {
                crate::t_eprintln!("Server error: {}", e);
            }
        });
    }

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
        job_manager(
            ex_manager,
            runner,
            db,
            jobs,
            reload_rx,
            stop_rx,
            manager_active_jobs,
        )
        .await;
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
            let ex = Arc::clone(ex);
            let job_ex = Arc::clone(&ex);
            ex.spawn(async move {
                pipeline::run_job_loop(job, db, runner, &job_ex).await;
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
    stop_rx: smol::channel::Receiver<()>,
    active_job_ids: Arc<std::sync::RwLock<Vec<JobSummary>>>,
) {
    enum Signal {
        Reload,
        Stop,
    }

    let mut handles = spawn_jobs(&ex, &runner, &db, initial_jobs);

    loop {
        // Reload keeps the loop alive; Stop (or either channel closing)
        // falls out and drops `handles`, cancelling every job task.
        let signal = smol::future::or(
            async { reload_rx.recv().await.ok().map(|_| Signal::Reload) },
            async { stop_rx.recv().await.ok().map(|_| Signal::Stop) },
        )
        .await;
        match signal {
            Some(Signal::Reload) => {}
            Some(Signal::Stop) | None => break,
        }

        Timer::after(Duration::from_millis(200)).await;
        while reload_rx.try_recv().is_ok() {}

        crate::t_println!("Reloading config...");

        match loader::load_all_jobs("jobs.toml", "jobs", Arc::clone(&db)) {
            Ok(new_jobs) => {
                drop(handles);
                crate::cdp::browser::shutdown_all();

                if let Ok(mut lock) = active_job_ids.write() {
                    *lock = new_jobs
                        .list
                        .iter()
                        .map(|j| JobSummary {
                            id: j.config.id(),
                            name: j.config.name.clone(),
                            enabled: j.config.enabled,
                        })
                        .collect();
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
                workers: None,
                urls: None,
            },
            hooks: None,
            has_hooks_file: false,
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
    fn test_job_manager_reload_then_stop() {
        let (_dir, db) = make_temp_db();
        let ex = Arc::new(Executor::new());
        let runner = Arc::new(Runner::new());
        let (reload_tx, reload_rx) = smol::channel::unbounded::<()>();
        let (stop_tx, stop_rx) = smol::channel::unbounded::<()>();
        let active_job_ids = Arc::new(std::sync::RwLock::new(Vec::new()));
        let jobs = Jobs { list: vec![] };

        let mgr = job_manager(
            ex,
            runner,
            Arc::new(db),
            jobs,
            reload_rx,
            stop_rx,
            active_job_ids,
        );

        smol::block_on(async {
            // Reload must not terminate the manager...
            reload_tx.send(()).await.unwrap();
            // ...but Stop must (either channel closing counts too).
            stop_tx.send(()).await.unwrap();

            smol::future::or(mgr, async {
                Timer::after(Duration::from_secs(5)).await;
                panic!("job_manager did not stop after stop signal");
            })
            .await;
        });
    }

    #[test]
    fn test_job_manager_stops_when_channels_close() {
        let (_dir, db) = make_temp_db();
        let ex = Arc::new(Executor::new());
        let runner = Arc::new(Runner::new());
        let (reload_tx, reload_rx) = smol::channel::unbounded::<()>();
        let (_stop_tx, stop_rx) = smol::channel::unbounded::<()>();
        let active_job_ids = Arc::new(std::sync::RwLock::new(Vec::new()));
        let jobs = Jobs { list: vec![] };

        let mgr = job_manager(
            ex,
            runner,
            Arc::new(db),
            jobs,
            reload_rx,
            stop_rx,
            active_job_ids,
        );
        drop(reload_tx); // dropped sender → recv errors → break (old behavior)

        smol::block_on(async {
            smol::future::or(mgr, async {
                Timer::after(Duration::from_secs(5)).await;
                panic!("job_manager did not stop after channel close");
            })
            .await;
        });
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
