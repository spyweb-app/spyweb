use crate::config::defaults;
use crate::lua::hooks::JobHooks;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Jobs {
    pub list: Vec<Job>,
}

#[derive(Debug)]
pub struct Job {
    pub config: JobConfig,
    pub hooks: Option<JobHooks>,
    pub has_hooks_file: bool,
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobConfig {
    pub name: String,
    pub url: String,
    pub selector: String,
    pub fields: Vec<Field>,
    pub keywords: Option<Vec<String>>,
    pub search_fields: Option<Vec<String>>,
    pub webhook: Option<Webhook>,

    #[serde(default)]
    pub debug: bool,

    #[serde(default = "defaults::enabled")]
    pub enabled: bool,

    #[serde(default = "defaults::interval")]
    pub interval: u32,

    pub proxy: Option<Proxy>,

    #[serde(default = "defaults::notification")]
    pub notification: Option<Notification>,
    pub headers: Option<HashMap<String, String>>,

    pub hash_fields: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Proxy {
    #[serde(default = "defaults::enabled")]
    pub enabled: bool,

    #[serde(default = "defaults::rotate")]
    pub rotate: Rotate,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobFile {
    pub jobs: Vec<JobConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Field {
    Shorthand(String),
    Full {
        name: String,
        selector: String,
        #[serde(default = "defaults::text")]
        att: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Webhook {
    #[serde(default = "defaults::enabled")]
    pub enabled: bool,
    pub url: String,
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Notification {
    #[serde(default = "defaults::enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub timeout: u32,
    pub title: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub enum Rotate {
    Sticky,
    Random,
    RoundRobin,
}

pub fn normalize_job_id(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());

    for c in input.chars() {
        if c.is_alphanumeric() {
            normalized.extend(c.to_lowercase());
        } else {
            normalized.push('_');
        }
    }

    normalized.trim_matches('_').to_string()
}
// pub struct Field {
//     pub name: String,
//     pub selector: String,
//     pub att: String,
// }

impl JobConfig {
    pub fn id(&self) -> String {
        normalize_job_id(&self.name)
    }
    pub fn field_names(&self) -> Vec<String> {
        self.fields
            .iter()
            .map(|field| match field {
                Field::Shorthand(raw) => raw
                    .split_once(':')
                    .map(|(name, _)| name.trim().to_owned())
                    .unwrap_or_else(|| raw.trim().to_owned()),
                Field::Full { name, .. } => name.clone(),
            })
            .collect()
    }
}

fn spyweb_home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            if !user_profile.trim().is_empty() {
                return Some(PathBuf::from(user_profile));
            }
        }

        if let (Ok(homedrive), Ok(homepath)) =
            (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH"))
        {
            let combined = format!("{}{}", homedrive, homepath);
            if !combined.trim().is_empty() {
                return Some(PathBuf::from(combined));
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return Some(PathBuf::from(home));
        }
    }

    None
}

pub fn spyweb_profile_root() -> Option<PathBuf> {
    spyweb_home_dir().map(|home| home.join(".spyweb"))
}

pub fn spyweb_job_profile_dir(job_dir: &Path) -> Option<PathBuf> {
    let job_folder = job_dir.file_name()?.to_string_lossy().to_string();
    spyweb_profile_root().map(|root| root.join(job_folder))
}

impl Job {
    pub fn profile_dir(&self) -> Option<PathBuf> {
        self.dir.as_deref().and_then(spyweb_job_profile_dir)
    }

    pub fn profile_label(&self) -> String {
        self.dir
            .as_ref()
            .and_then(|dir| dir.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.config.id())
    }
}
