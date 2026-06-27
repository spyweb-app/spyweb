use std::sync::Arc;
use std::{fs, path::Path};

use crate::config;
use crate::config::validate;
use crate::lua::hooks::JobHooks;
use crate::services::db::Db;
use anyhow::Result;
use config::types::{Job, JobConfig, JobFile, Jobs};

pub fn load_all_jobs(file_path: &str, dir_path: &str, db: Arc<Db>) -> Result<Jobs> {
    let mut list = load_config_from_file(file_path)?;
    list.extend(load_config_from_dir(dir_path, db)?);
    validate::validate_job_set(&list)?;
    Ok(Jobs { list })
}

pub fn load_dir_jobs(dir_path: &str, db: Arc<Db>) -> Result<Vec<Job>> {
    load_config_from_dir(dir_path, db)
}

fn load_config_from_file(path: &str) -> Result<Vec<Job>> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(path)?;
    let file: JobFile = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Config error in '{}': {}", path.display(), e))?;

    let mut jobs = Vec::with_capacity(file.jobs.len());
    for config in file.jobs {
        validate::validate_job_config(&config)?;
        jobs.push(Job {
            config,
            hooks: None,
            has_hooks_file: false,
            dir: None,
        });
    }

    Ok(jobs)
}

fn load_config_from_dir(path: &str, db: Arc<Db>) -> Result<Vec<Job>> {
    let mut jobs: Vec<Job> = vec![];
    let base = Path::new(path);
    if !base.exists() {
        return Ok(jobs);
    }
    for entry in fs::read_dir(base)? {
        let dir = entry?.path();
        if !dir.is_dir() {
            continue;
        }

        let config_path = dir.join("config.toml");
        if !config_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&config_path)?;
        let config: JobConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Config error in '{}': {}", config_path.display(), e))?;
        validate::validate_job_config(&config)?;
        let hook_path = dir.join("hooks.lua");
        let has_hooks_file = hook_path.exists();
        let hooks = if config.enabled && has_hooks_file {
            match JobHooks::load(&hook_path, Arc::clone(&db), &config.id()) {
                Ok(h) => Some(h),
                Err(e) => {
                    crate::t_eprintln!("Failed to load hooks for '{}': {}", config.name, e);
                    None
                }
            }
        } else {
            None
        };

        jobs.push(Job {
            config,
            hooks,
            has_hooks_file,
            dir: Some(dir),
        });
    }

    Ok(jobs)
}
