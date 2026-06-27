use anyhow::{Result, bail};

use crate::config::types::{Field, Job, JobConfig};

/// Compile Lua source to bytecode without executing. Catches syntax errors only.
pub(crate) fn validate_lua_syntax(
    lua: &mlua::Lua,
    source: &str,
    chunk_name: &str,
) -> Result<(), String> {
    lua.load(source)
        .set_name(chunk_name)
        .into_function()
        .map_err(|e| format!("{}", e))?;
    Ok(())
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

fn validate_url_list(values: &[String], field_name: &str, job_name: &str) -> Result<()> {
    if values.is_empty() {
        bail!(
            "Invalid {} for job '{}': value cannot be empty",
            field_name,
            job_name
        );
    }

    for value in values {
        validate_url(value, field_name, job_name)?;
    }

    Ok(())
}

pub(crate) fn validate_job_config(config: &JobConfig) -> Result<()> {
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

    if let Some(workers) = config.workers
        && workers == 0
    {
        bail!(
            "Invalid job config for '{}': workers must be > 0",
            config.name
        );
    }

    validate_url(&config.url, "url", &config.name)?;

    if let Some(urls) = &config.urls {
        validate_url_list(urls, "urls", &config.name)?;
    }

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
            Field::Shorthand(raw) => {
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
            Field::Full { name, selector, .. } => {
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

pub(crate) fn validate_job_set(jobs: &[Job]) -> Result<()> {
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
