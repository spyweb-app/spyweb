pub mod defaults;
pub mod loader;
pub mod types;

use std::sync::Arc;

use crate::services::db::Db;

// pub const BASE_URL: &str = "127.0.0.1:8001";

pub fn get_port() -> u16 {
    get_port_with_override(None)
}

pub fn get_port_with_override(override_port: Option<u16>) -> u16 {
    override_port
        .or_else(|| {
            std::env::var("SPYWEB_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
        })
        .unwrap_or(7979)
}

pub fn get_base_url_with_override(override_port: Option<u16>) -> String {
    format!("127.0.0.1:{}", get_port_with_override(override_port))
}

pub fn get_base_url() -> String {
    get_base_url_with_override(None)
}

pub fn config_check() -> anyhow::Result<()> {
    let db = Arc::new(Db::open("data")?);
    match loader::load_all_jobs("jobs.toml", "jobs", db) {
        Ok(jobs) => {
            println!(
                "Found {} job(s):",
                crate::color::c_info(&jobs.list.len().to_string())
            );
            for job in &jobs.list {
                let hook_status = if job.has_hooks_file {
                    if job.config.enabled {
                        crate::color::c_info("hooks.lua")
                    } else {
                        crate::color::c_dim("hooks.lua")
                    }
                } else {
                    crate::color::c_dim("no hook")
                };
                let status = if job.config.enabled {
                    crate::color::c_ok("enabled")
                } else {
                    crate::color::c_dim("disabled")
                };
                println!(
                    "  [{}] {} ({})",
                    status,
                    crate::color::c_job(&job.config.name),
                    hook_status
                );
            }
            println!("{}", crate::color::c_ok("Config OK"));
            Ok(())
        }
        Err(e) => {
            crate::t_eprintln!("Config error: {}", e);
            std::process::exit(1);
        }
    }
}
