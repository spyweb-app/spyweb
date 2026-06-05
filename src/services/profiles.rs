use crate::config::types::{Job, Jobs, spyweb_job_profile_dir};
use anyhow::{Context, Result};
use fs4::FileExt;
use std::fs;
use std::path::{Path, PathBuf};

pub fn find_job<'a>(jobs: &'a Jobs, query: &str) -> Option<&'a Job> {
    let needle = query.trim();
    if needle.is_empty() {
        return None;
    }

    let normalized = crate::config::types::normalize_job_id(needle);

    jobs.list
        .iter()
        .find(|job| job.config.id() == normalized || job.config.name.eq_ignore_ascii_case(needle))
}

pub fn profile_dir_for_job(job: &Job) -> Option<PathBuf> {
    job.dir.as_deref().and_then(spyweb_job_profile_dir)
}

pub fn profile_in_use(path: &Path) -> Result<bool> {
    let lock_path = path.join(".spyweb.lock");
    if !lock_path.exists() {
        return Ok(false);
    }

    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("Failed to open lock file: {}", lock_path.display()))?;

    match file.try_lock_exclusive() {
        Ok(()) => Ok(false), // let _ = file.unlock();
        Err(_) => Ok(true),
    }
}

fn remove_dir_contents(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(path).with_context(|| format!("Failed to read {}", path.display()))? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            fs::remove_dir_all(&entry_path).with_context(|| {
                format!(
                    "Cannot clear profile: the browser profile at {} is in use (locked by a running browser). Close the browser and try again.",
                    entry_path.display()
                )
            })?;
        } else {
            fs::remove_file(&entry_path)
                .with_context(|| format!("Failed to remove {}", entry_path.display()))?;
        }
    }

    Ok(())
}

pub fn clear_profile_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    remove_dir_contents(path)
}

pub fn delete_profile_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    fs::remove_dir_all(path)
        .with_context(|| format!("Failed to delete profile directory {}", path.display()))
}

pub fn ensure_profile_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("Failed to create profile directory {}", path.display()))?;
    Ok(())
}

pub fn profile_dir_or_bail(job: &Job) -> Result<PathBuf> {
    profile_dir_for_job(job).ok_or_else(|| {
        anyhow::anyhow!(
            "Could not determine profile directory for job '{}'",
            job.config.name
        )
    })
}

pub fn profile_summary(job: &Job) -> Result<String> {
    let Some(path) = profile_dir_for_job(job) else {
        return Ok(format!(
            "[no profile] {}",
            crate::color::c_job(&job.config.name)
        ));
    };

    let exists = path.exists();
    let locked = profile_in_use(&path)?;
    Ok(format!(
        "{} {} {}",
        crate::color::c_job(&job.config.name),
        if exists {
            crate::color::c_ok("exists")
        } else {
            crate::color::c_dim("missing")
        },
        if locked {
            crate::color::c_warn("in use")
        } else {
            crate::color::c_dim("free")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Field, JobConfig};

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
            dir: dir.map(PathBuf::from),
        }
    }

    #[test]
    fn finds_job_by_name_or_id() {
        let jobs = Jobs {
            list: vec![mock_job(Some("jobs/spyweb"), "SpyWeb")],
        };
        assert!(find_job(&jobs, "SpyWeb").is_some());
        assert!(find_job(&jobs, "SpyWeb").is_some());
        assert!(find_job(&jobs, "  ").is_none());
    }

    #[test]
    fn resolves_profile_path_from_job_dir() {
        let job = mock_job(Some("jobs/spyweb"), "SpyWeb");
        let path = profile_dir_for_job(&job).expect("profile path");
        assert!(path.ends_with(".spyweb/spyweb"));
    }

    #[test]
    fn returns_none_without_job_dir_when_no_root_available() {
        let job = mock_job(None, "SpyWeb");
        let _ = profile_dir_for_job(&job);
    }
}
