use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use spyweb::config::loader;
use spyweb::config::types::Job;
use spyweb::scraper::pipeline::run_once;
use spyweb::scraper::request::RequestConfig;
use spyweb::scraper::runner::Runner;
use spyweb::services::db::Db;
use spyweb::services::server::types;
use tempfile::TempDir;

const FIXTURES_DIR: &str = "tests/e2e/jobs";

pub enum DbVariant {
    Kv,
    Sql,
}

pub struct MockServer {
    pub port: u16,
    pub recorded_webhooks: Arc<Mutex<Vec<String>>>,
}

impl MockServer {
    pub fn start() -> Self {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let r = recorded.clone();

        let server = types::MockServer::start(move |req| dispatch_e2e_mock(req, &r));
        let port = server.port();

        Self {
            port,
            recorded_webhooks: recorded,
        }
    }
}

fn dispatch_e2e_mock(
    request: &types::Request,
    webhooks: &Arc<Mutex<Vec<String>>>,
) -> types::Response {
    let url = &request.url;
    let method = &request.method;

    if url.starts_with("/echo") {
        let body = if method == "POST" || method == "PUT" || method == "PATCH" {
            request
                .body_bytes()
                .map(|b| String::from_utf8_lossy(b).to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        // Reconstruct query string from parsed params
        let query = request
            .query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        let json = serde_json::json!({ "query": query, "method": method, "body": body });
        return types::Response::json(&json);
    }

    if url.starts_with("/webhook") {
        let body = request
            .body_bytes()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .unwrap_or_default();
        webhooks.lock().unwrap().push(body);
        return types::Response::json(&serde_json::json!({ "status": "recorded" }));
    }

    if url.starts_with("/multipart") {
        return types::Response::json(&serde_json::json!({ "status": "multipart_received" }));
    }

    if url.starts_with("/status/") {
        if let Ok(c) = url.trim_start_matches("/status/").parse::<u16>() {
            return types::Response::text("").with_status(c);
        }
    }

    if url.starts_with("/delay/") {
        if let Ok(d) = url.trim_start_matches("/delay/").parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(d));
            return types::Response::text("delayed");
        }
    }

    types::Response::text("not found").with_status(404)
}

pub struct TestEnv {
    pub _dir: TempDir,
    pub db: Arc<Db>,
    pub jobs_dir: std::path::PathBuf,
}

impl TestEnv {
    pub fn new(fixture_name: &str, db_variant: DbVariant) -> Self {
        // Create temp dir inside CWD so the IO sandbox (which checks against
        // std::env::current_dir()) allows file operations.
        let cwd = std::env::current_dir().expect("failed to get CWD");
        let dir = TempDir::new_in(&cwd).expect("failed to create temp dir");
        let jobs_dir = dir.path().join("jobs");
        fs::create_dir_all(&jobs_dir).expect("failed to create jobs dir");

        let src = Path::new(FIXTURES_DIR).join(fixture_name);
        let dst = jobs_dir.join(fixture_name);
        copy_dir_recursive(&src, &dst).expect("failed to copy fixture");

        let db_filename = match db_variant {
            DbVariant::Kv => "test.redb",
            DbVariant::Sql => "test.sqlite",
        };
        let db_path = dir.path().join(db_filename);
        let db_path_str = db_path.to_str().expect("invalid db path");
        let db = Arc::new(Db::open(db_path_str).expect("failed to open db"));

        TestEnv {
            _dir: dir,
            db,
            jobs_dir,
        }
    }

    /// Change CWD to the temp directory for the lifetime of the returned guard.
    /// This lets Lua's `dofile` resolve relative paths in the fixture directory.
    pub fn load_job(&self, fixture_name: &str) -> Job {
        let jobs = loader::load_dir_jobs(
            self.jobs_dir.to_str().expect("invalid path"),
            self.db.clone(),
        )
        .expect("failed to load jobs");

        jobs.into_iter()
            .find(|j| j.config.name == fixture_name)
            .unwrap_or_else(|| panic!("job '{}' not found", fixture_name))
    }

    pub fn run_cycle(&self, job: &Job, worker_id: usize) -> anyhow::Result<()> {
        let runner = Arc::new(Runner::new());
        let req = RequestConfig::from_job(&job.config);
        smol::block_on(run_once(job, &self.db, &runner, &req, worker_id))
    }

    pub fn run_on_finished(&self, job: &Job) {
        if let Some(h) = job.hooks.as_ref() {
            smol::block_on(h.run_on_finished());
        }
    }

    pub fn read_log(&self, job: &Job) -> Vec<String> {
        let Some(h) = job.hooks.as_ref() else {
            return vec![];
        };
        smol::block_on(async {
            h.with_lua(|lua: &mlua::Lua| {
                lua.globals()
                    .get::<mlua::Table>("_G")
                    .ok()
                    .and_then(|g| g.get::<Vec<String>>("log").ok())
                    .unwrap_or_default()
            })
            .await
        })
    }

    pub fn read_global_bool(&self, job: &Job, key: &str) -> Option<bool> {
        let Some(h) = job.hooks.as_ref() else {
            return None;
        };
        smol::block_on(async {
            h.with_lua(|lua: &mlua::Lua| {
                lua.globals()
                    .get::<mlua::Table>("_G")
                    .ok()
                    .and_then(|g| g.get::<bool>(key).ok())
            })
            .await
        })
    }

    pub fn set_global_bool(&self, job: &Job, key: &str, val: bool) {
        let Some(h) = job.hooks.as_ref() else {
            return;
        };
        smol::block_on(async {
            h.with_lua(|lua: &mlua::Lua| {
                let _ = lua.globals().set(key, val);
            })
            .await;
        });
    }

    pub fn set_global_string(&self, job: &Job, key: &str, val: &str) {
        let Some(h) = job.hooks.as_ref() else {
            return;
        };
        smol::block_on(async {
            h.with_lua(|lua: &mlua::Lua| {
                let _ = lua.globals().set(key, val);
            })
            .await;
        });
    }

    pub fn read_global_raw_string(&self, job: &Job, key: &str) -> Option<String> {
        let Some(h) = job.hooks.as_ref() else {
            return None;
        };
        smol::block_on(async {
            h.with_lua(|lua: &mlua::Lua| lua.globals().get::<String>(key).ok())
                .await
        })
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            fs::copy(&entry.path(), &dst_path)?;
        }
    }
    Ok(())
}
