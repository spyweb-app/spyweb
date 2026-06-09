use crate::config::types::JobConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub fields: HashMap<String, String>,
    pub timestamp: u64,
    pub hash: String,
    pub match_key: Vec<String>,
}

impl Record {
    pub fn datetime_zulu(&self) -> String {
        crate::services::utils::nanos_to_zulu(Some(self.timestamp))
    }
}

fn normalized_hash_fields(job: &JobConfig) -> Vec<String> {
    job.hash_fields
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

fn hash_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut pairs: Vec<_> = pairs.into_iter().collect();
    pairs.sort_unstable_by_key(|(k, _)| *k);

    let mut buffer = Vec::with_capacity(pairs.len() * 64);

    for (k, v) in pairs {
        buffer.extend_from_slice(k.as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(v.as_bytes());
        buffer.push(0);
    }

    let hash = xxhash_rust::xxh3::xxh3_64(&buffer);
    format!("{:016x}", hash)
}

pub fn hash_fields(job: &JobConfig, fields: &HashMap<String, String>) -> String {
    let configured = normalized_hash_fields(job);
    if configured.is_empty() {
        return hash_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    let selected: Vec<(&str, &str)> = configured
        .iter()
        .map(|name| {
            let value = fields.get(name).map(String::as_str).unwrap_or("");
            (name.as_str(), value)
        })
        .collect();

    let empty_fields: Vec<&str> = selected
        .iter()
        .filter_map(|(name, value)| value.trim().is_empty().then_some(*name))
        .collect();

    if !empty_fields.is_empty() {
        crate::t_eprintln!(
            "Job {} has empty hash_fields at runtime: {}",
            crate::color::c_job(&job.name),
            empty_fields.join(", ")
        );
    }

    if empty_fields.len() == selected.len() {
        crate::t_eprintln!(
            "Job {} has all configured hash_fields empty at runtime; falling back to hashing all extracted fields",
            crate::color::c_job(&job.name)
        );
        return hash_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    hash_pairs(selected)
}
