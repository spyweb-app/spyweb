use std::sync::Arc;
use std::{fs, path::Path};

use crate::config;
use crate::lua::hooks::JobHooks;
use crate::services::db::Db;
use anyhow::{Result, bail};
use config::types::{Job, JobConfig, JobFile, Jobs};

pub fn load_all_jobs(file_path: &str, dir_path: &str, db: Arc<Db>) -> Result<Jobs> {
    let mut list = load_config_from_file(file_path)?;
    list.extend(load_config_from_dir(dir_path, db)?);
    validate_job_set(&list)?;
    Ok(Jobs { list })
}

pub fn load_dir_jobs(dir_path: &str, db: Arc<Db>) -> Result<Vec<Job>> {
    load_config_from_dir(dir_path, db)
}

fn normalized_hash_fields(config: &JobConfig) -> Vec<String> {
    config
        .hash_fields
        .as_ref()
        .map(|fields| {
            fields
                .iter()
                .map(|field| field.trim())
                .filter(|field| !field.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn normalized_search_fields(config: &JobConfig) -> Vec<String> {
    config
        .search_fields
        .as_ref()
        .map(|fields| {
            fields
                .iter()
                .map(|field| field.trim())
                .filter(|field| !field.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn validate_url(value: &str, field_name: &str, job_name: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!(
            "Invalid {} for job '{}': value cannot be blank",
            field_name,
            job_name
        );
    }

    let uri: ureq::http::Uri = trimmed.parse().map_err(|_| {
        anyhow::anyhow!(
            "Invalid {} for job '{}': '{}' is not a valid URL",
            field_name,
            job_name,
            value
        )
    })?;

    if uri.scheme().is_none() || uri.authority().is_none() {
        bail!(
            "Invalid {} for job '{}': '{}' is not a valid absolute URL",
            field_name,
            job_name,
            value
        );
    }

    Ok(())
}

fn validate_job_config(config: &JobConfig) -> Result<()> {
    if config.name.trim().is_empty() {
        bail!("Invalid job config: name cannot be blank");
    }

    if config.url.trim().is_empty() {
        bail!(
            "Invalid job config for '{}': url cannot be blank",
            config.name
        );
    }

    if config.selector.trim().is_empty() {
        bail!(
            "Invalid job config for '{}': selector cannot be blank",
            config.name
        );
    }

    if config.fields.is_empty() {
        bail!(
            "Invalid job config for '{}': fields cannot be empty",
            config.name
        );
    }

    if config.interval == 0 {
        bail!(
            "Invalid job config for '{}': interval must be > 0",
            config.name
        );
    }

    validate_url(&config.url, "url", &config.name)?;

    if let Some(webhook) = &config.webhook
        && webhook.enabled
    {
        validate_url(&webhook.url, "webhook.url", &config.name)?;
    }

    if let Some(proxy) = &config.proxy
        && proxy.enabled
    {
        if proxy.urls.is_empty() {
            bail!(
                "Invalid proxy config for job '{}': proxy.urls cannot be empty when proxy is enabled",
                config.name
            );
        }

        for proxy_url in &proxy.urls {
            validate_url(proxy_url, "proxy.urls", &config.name)?;
        }
    }

    let mut valid_fields = Vec::with_capacity(config.fields.len());
    for field in &config.fields {
        match field {
            config::types::Field::Shorthand(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    bail!(
                        "Invalid field definition for job '{}': shorthand field cannot be blank",
                        config.name
                    );
                }

                let (name, selector) = match trimmed.split_once(':') {
                    Some((name, selector)) => (name.trim(), selector.trim()),
                    None => (trimmed, ""),
                };

                if name.is_empty() {
                    bail!(
                        "Invalid field definition for job '{}': field name cannot be blank",
                        config.name
                    );
                }

                if trimmed.contains(':') && selector.is_empty() {
                    bail!(
                        "Invalid field definition for job '{}': selector cannot be blank for field '{}'",
                        config.name,
                        name
                    );
                }

                valid_fields.push(name.to_owned());
            }
            config::types::Field::Full { name, selector, .. } => {
                if name.trim().is_empty() {
                    bail!(
                        "Invalid field definition for job '{}': field name cannot be blank",
                        config.name
                    );
                }

                if selector.trim().is_empty() {
                    bail!(
                        "Invalid field definition for job '{}': selector cannot be blank for field '{}'",
                        config.name,
                        name
                    );
                }

                valid_fields.push(name.trim().to_owned());
            }
        }
    }

    let mut seen_fields = std::collections::HashSet::new();
    let duplicate_fields: Vec<String> = valid_fields
        .iter()
        .filter(|name| !seen_fields.insert((*name).clone()))
        .cloned()
        .collect();

    if !duplicate_fields.is_empty() {
        bail!(
            "Invalid field definitions for job '{}': duplicate field name(s): {}",
            config.name,
            duplicate_fields.join(", ")
        );
    }

    let search_fields = normalized_search_fields(config);
    let unknown_search_fields: Vec<String> = search_fields
        .iter()
        .filter(|field| !valid_fields.iter().any(|valid| valid == *field))
        .cloned()
        .collect();

    if !unknown_search_fields.is_empty() {
        bail!(
            "Invalid search_fields for job '{}': unknown field(s): {}. Valid fields: {}",
            config.name,
            unknown_search_fields.join(", "),
            valid_fields.join(", ")
        );
    }

    let hash_fields = normalized_hash_fields(config);
    let unknown_fields: Vec<String> = hash_fields
        .iter()
        .filter(|field| !valid_fields.iter().any(|valid| valid == *field))
        .cloned()
        .collect();

    if !unknown_fields.is_empty() {
        bail!(
            "Invalid hash_fields for job '{}': unknown field(s): {}. Valid fields: {}",
            config.name,
            unknown_fields.join(", "),
            valid_fields.join(", ")
        );
    }

    Ok(())
}

fn validate_job_set(jobs: &[Job]) -> Result<()> {
    let mut seen_names = std::collections::HashSet::new();
    let duplicate_names: Vec<String> = jobs
        .iter()
        .map(|job| job.config.name.clone())
        .filter(|name| !seen_names.insert(name.clone()))
        .collect();

    if !duplicate_names.is_empty() {
        bail!(
            "Invalid job set: duplicate job name(s): {}",
            duplicate_names.join(", ")
        );
    }

    let mut seen_ids = std::collections::HashSet::new();
    let duplicate_ids: Vec<String> = jobs
        .iter()
        .map(|job| job.config.id())
        .filter(|id| !seen_ids.insert(id.clone()))
        .collect();

    if !duplicate_ids.is_empty() {
        bail!(
            "Invalid job set: duplicate job id(s): {}",
            duplicate_ids.join(", ")
        );
    }

    Ok(())
}

fn load_config_from_file(path: &str) -> Result<Vec<Job>> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(path)?;
    let file: JobFile = toml::from_str(&content)?;

    let mut jobs = Vec::with_capacity(file.jobs.len());
    for config in file.jobs {
        validate_job_config(&config)?;
        jobs.push(Job {
            config,
            hooks: None,
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

        let content = fs::read_to_string(config_path)?;
        let config: JobConfig = toml::from_str(&content)?;
        validate_job_config(&config)?;
        let hook_path = dir.join("hooks.lua");
        let hooks = if hook_path.exists() {
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
            dir: Some(dir),
        });
    }

    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Field, Job};

    fn mock_job(hash_fields: Option<Vec<&str>>) -> JobConfig {
        JobConfig {
            name: "test-job".to_string(),
            url: "https://example.com".to_string(),
            selector: ".item".to_string(),
            fields: vec![
                Field::Full {
                    name: "title".to_string(),
                    selector: ".title".to_string(),
                    att: "text".to_string(),
                },
                Field::Shorthand("price:.price".to_string()),
                Field::Shorthand("date".to_string()),
            ],
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 60,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: hash_fields
                .map(|fields| fields.into_iter().map(ToOwned::to_owned).collect()),
        }
    }

    fn wrap_job(config: JobConfig) -> Job {
        Job {
            config,
            hooks: None,
            dir: None,
        }
    }

    #[test]
    fn validates_known_hash_fields() {
        let job = mock_job(Some(vec!["title", "price"]));
        assert!(validate_job_config(&job).is_ok());
    }

    #[test]
    fn ignores_blank_hash_fields() {
        let job = mock_job(Some(vec!["title", " ", ""]));
        assert!(validate_job_config(&job).is_ok());
    }

    #[test]
    fn rejects_unknown_hash_fields() {
        let job = mock_job(Some(vec!["title", "stupid_typo"]));
        let err = validate_job_config(&job).unwrap_err().to_string();
        assert!(err.contains("Invalid hash_fields for job 'test-job'"));
        assert!(err.contains("stupid_typo"));
        assert!(err.contains("title, price, date"));
    }

    #[test]
    fn rejects_blank_required_fields() {
        let mut job = mock_job(None);
        job.name = " ".to_string();
        assert_eq!(
            validate_job_config(&job).unwrap_err().to_string(),
            "Invalid job config: name cannot be blank"
        );

        let mut job = mock_job(None);
        job.url = " ".to_string();
        assert_eq!(
            validate_job_config(&job).unwrap_err().to_string(),
            "Invalid job config for 'test-job': url cannot be blank"
        );

        let mut job = mock_job(None);
        job.selector = " ".to_string();
        assert_eq!(
            validate_job_config(&job).unwrap_err().to_string(),
            "Invalid job config for 'test-job': selector cannot be blank"
        );
    }

    #[test]
    fn rejects_empty_fields_and_zero_interval() {
        let mut job = mock_job(None);
        job.fields = vec![];
        assert_eq!(
            validate_job_config(&job).unwrap_err().to_string(),
            "Invalid job config for 'test-job': fields cannot be empty"
        );

        let mut job = mock_job(None);
        job.interval = 0;
        assert_eq!(
            validate_job_config(&job).unwrap_err().to_string(),
            "Invalid job config for 'test-job': interval must be > 0"
        );
    }

    #[test]
    fn rejects_invalid_urls() {
        let mut job = mock_job(None);
        job.url = "not-a-url".to_string();
        assert!(
            validate_job_config(&job)
                .unwrap_err()
                .to_string()
                .contains("Invalid url for job 'test-job'")
        );

        let mut job = mock_job(None);
        job.webhook = Some(crate::config::types::Webhook {
            enabled: true,
            url: "bad-webhook".to_string(),
            headers: None,
        });
        assert!(
            validate_job_config(&job)
                .unwrap_err()
                .to_string()
                .contains("Invalid webhook.url for job 'test-job'")
        );

        let mut job = mock_job(None);
        job.proxy = Some(crate::config::types::Proxy {
            enabled: true,
            rotate: crate::config::types::Rotate::Random,
            urls: vec!["bad-proxy".to_string()],
        });
        assert!(
            validate_job_config(&job)
                .unwrap_err()
                .to_string()
                .contains("Invalid proxy.urls for job 'test-job'")
        );
    }

    #[test]
    fn rejects_invalid_field_definitions_and_search_fields() {
        let mut job = mock_job(None);
        job.fields = vec![Field::Shorthand("   ".to_string())];
        assert!(
            validate_job_config(&job)
                .unwrap_err()
                .to_string()
                .contains("shorthand field cannot be blank")
        );

        let mut job = mock_job(None);
        job.fields = vec![Field::Full {
            name: "title".to_string(),
            selector: "   ".to_string(),
            att: "text".to_string(),
        }];
        assert!(
            validate_job_config(&job)
                .unwrap_err()
                .to_string()
                .contains("selector cannot be blank")
        );

        let mut job = mock_job(None);
        job.fields = vec![
            Field::Shorthand("title:.title".to_string()),
            Field::Full {
                name: "title".to_string(),
                selector: ".title-2".to_string(),
                att: "text".to_string(),
            },
        ];
        assert!(
            validate_job_config(&job)
                .unwrap_err()
                .to_string()
                .contains("duplicate field name(s): title")
        );

        let mut job = mock_job(None);
        job.search_fields = Some(vec!["title".to_string(), "missing".to_string()]);
        let err = validate_job_config(&job).unwrap_err().to_string();
        assert!(err.contains("Invalid search_fields for job 'test-job'"));
        assert!(err.contains("missing"));
    }

    #[test]
    fn rejects_duplicate_job_names_and_ids() {
        let err = validate_job_set(&[wrap_job(mock_job(None)), wrap_job(mock_job(None))])
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate job name(s): test-job"));

        let mut first = mock_job(None);
        first.name = "Hello World".to_string();
        let mut second = mock_job(None);
        second.name = "hello-world".to_string();

        let err = validate_job_set(&[wrap_job(first), wrap_job(second)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate job id(s): hello_world"));
    }
}
