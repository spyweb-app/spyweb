pub mod defaults;
pub mod loader;
pub mod types;

use std::sync::Arc;

use crate::services::db::Db;

// pub const BASE_URL: &str = "127.0.0.1:8001";

pub fn get_port() -> u16 {
    std::env::var("SPYWEB_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7979)
}

pub fn get_base_url() -> String {
    format!("127.0.0.1:{}", get_port())
}

pub fn config_check() -> anyhow::Result<()> {
    let db = Arc::new(Db::open("data")?);
    match loader::load_all_jobs("jobs.toml", "jobs", db) {
        Ok(jobs) => {
            println!("Found {} job(s):", jobs.list.len());
            for job in &jobs.list {
                let hook_status = if job.hooks.is_some() {
                    "hook.lua"
                } else {
                    "no hook"
                };
                let status = if job.config.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                println!("  [{}] {} ({})", status, job.config.name, hook_status);
            }
            println!("Config OK");
            Ok(())
        }
        Err(e) => {
            eprintln!("Config error: {}", e);
            std::process::exit(1);
        }
    }
}
