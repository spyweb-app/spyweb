use std::sync::Arc;

use clap::Subcommand;

use crate::config::loader;
use crate::config::types::{Job, Jobs};
use crate::services::db::Db;

#[derive(Subcommand)]
pub enum ProfileCommands {
    /// Show profile status (exists / in use) for a job or all jobs
    #[command(alias = "list")]
    Check { job: Option<String> },
    /// Wipe the browser profile for a job or all jobs (clears cookies/cache, keeps the directory)
    Clear { target: String },
    /// Delete the profile directory for a job or all jobs entirely
    Delete { target: String },
}

fn load_jobs_for_profiles() -> anyhow::Result<Jobs> {
    let db = Arc::new(Db::open("data")?);
    loader::load_all_jobs("jobs.toml", "jobs", db)
}

fn print_profile_check(job: &Job) -> anyhow::Result<()> {
    let summary = crate::services::profiles::profile_summary(job)?;
    println!("{}", summary);
    Ok(())
}

fn print_profile_check_all(jobs: &Jobs) -> anyhow::Result<()> {
    let mut shown = 0usize;
    for job in &jobs.list {
        if job.dir.is_none() {
            continue;
        }
        print_profile_check(job)?;
        shown += 1;
    }

    if shown == 0 {
        println!("{}", crate::color::c_dim("No job profiles found."));
    }

    Ok(())
}

fn resolve_job<'a>(jobs: &'a Jobs, target: &str) -> anyhow::Result<&'a Job> {
    crate::services::profiles::find_job(jobs, target).ok_or_else(|| {
        anyhow::anyhow!(
            "Job '{}' not found. Use `spyweb check` to list jobs.",
            target
        )
    })
}

fn clear_job_profile(job: &Job) -> anyhow::Result<()> {
    let Some(path) = crate::services::profiles::profile_dir_for_job(job) else {
        println!(
            "{} has no profile directory",
            crate::color::c_job(&job.config.name)
        );
        return Ok(());
    };

    crate::services::profiles::clear_profile_dir(&path)?;
    println!(
        "Cleared profile for {} at {}",
        crate::color::c_job(&job.config.name),
        path.display()
    );
    Ok(())
}

fn delete_job_profile(job: &Job) -> anyhow::Result<()> {
    let Some(path) = crate::services::profiles::profile_dir_for_job(job) else {
        println!(
            "{} has no profile directory",
            crate::color::c_job(&job.config.name)
        );
        return Ok(());
    };

    crate::services::profiles::delete_profile_dir(&path)?;
    println!(
        "Deleted profile for {} at {}",
        crate::color::c_job(&job.config.name),
        path.display()
    );
    Ok(())
}

pub fn handle_profile_command(cmd: ProfileCommands) -> anyhow::Result<()> {
    let jobs = load_jobs_for_profiles()?;

    match cmd {
        ProfileCommands::Check { job } => {
            match job.as_deref() {
                None | Some("all") | Some("--all") => print_profile_check_all(&jobs)?,
                Some(target) => print_profile_check(resolve_job(&jobs, target)?)?,
            }
            Ok(())
        }
        ProfileCommands::Clear { target } => {
            if target == "all" || target == "--all" {
                for job in &jobs.list {
                    if job.dir.is_none() {
                        continue;
                    }
                    clear_job_profile(job)?;
                }
            } else {
                clear_job_profile(resolve_job(&jobs, &target)?)?;
            }
            Ok(())
        }
        ProfileCommands::Delete { target } => {
            if target == "all" || target == "--all" {
                for job in &jobs.list {
                    if job.dir.is_none() {
                        continue;
                    }
                    delete_job_profile(job)?;
                }
            } else {
                delete_job_profile(resolve_job(&jobs, &target)?)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_job;
    use crate::config::types::{Field, Job, JobConfig, Jobs};

    fn mock_job(dir: Option<&str>, name: &str) -> Job {
        Job {
            config: JobConfig {
                name: name.to_string(),
                url: "https://example.com".to_string(),
                selector: ".item".to_string(),
                fields: vec![Field::Shorthand("title".to_string())],
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
            hooks: None,
            has_hooks_file: false,
            dir: dir.map(std::path::PathBuf::from),
        }
    }

    #[test]
    fn resolves_job_by_name_or_id() {
        let jobs = Jobs {
            list: vec![mock_job(Some("jobs/spyweb"), "Spyweb")],
        };
        assert!(resolve_job(&jobs, "SpyWeb").is_ok());
        assert!(resolve_job(&jobs, "spyweb").is_ok());
    }
}
