use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use mlua::Lua;
use smol::lock::Mutex;

use crate::lua::engine;
use crate::services::db::Db;

pub mod stages;

pub struct JobHooks {
    pub(crate) lua: Mutex<Lua>,
    pub(crate) job_name: String,
    pub(crate) hook_path: PathBuf,
    pub(crate) has_before_fetch: bool,
    pub(crate) has_override_fetch: bool,
    pub(crate) has_after_fetch: bool,
    pub(crate) has_override_extract: bool,
    pub(crate) has_after_extract: bool,
    pub(crate) has_filter_item: bool,
    pub(crate) has_before_store: bool,
    pub(crate) has_before_notify: bool,
    pub(crate) has_before_webhook: bool,
}

impl std::fmt::Debug for JobHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobHooks")
            .field("job_name", &self.job_name)
            .field("hook_path", &self.hook_path)
            .field("has_before_fetch", &self.has_before_fetch)
            .field("has_override_fetch", &self.has_override_fetch)
            .field("has_after_fetch", &self.has_after_fetch)
            .field("has_override_extract", &self.has_override_extract)
            .field("has_after_extract", &self.has_after_extract)
            .field("has_filter_item", &self.has_filter_item)
            .field("has_before_store", &self.has_before_store)
            .field("has_before_notify", &self.has_before_notify)
            .field("has_before_webhook", &self.has_before_webhook)
            .finish()
    }
}

#[derive(Debug)]
struct LuaHookError {
    hook_name: &'static str,
    hook_path: PathBuf,
    job_name: String,
    source: mlua::Error,
}

impl std::fmt::Display for LuaHookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Lua {} error for job '{}' in {}: {}",
            self.hook_name,
            self.job_name,
            self.hook_path.display(),
            self.source
        )
    }
}

impl std::error::Error for LuaHookError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl JobHooks {
    pub fn load(path: &Path, db: Arc<Db>, job_name: &str) -> Result<Self> {
        let job_dir = path.parent().map(|p| p.to_path_buf());
        let lua = engine::create_engine(job_dir, db, job_name)?;

        let source = std::fs::read_to_string(path)?;
        let chunk_name = path.to_string_lossy();
        lua.load(&source).set_name(chunk_name.as_ref()).exec()?;

        let has = |name: &str| -> bool {
            lua.globals()
                .get::<Option<mlua::Function>>(name)
                .unwrap_or(None)
                .is_some()
        };

        Ok(Self {
            job_name: job_name.to_string(),
            hook_path: path.to_path_buf(),
            has_before_fetch: has("before_fetch"),
            has_override_fetch: has("override_fetch"),
            has_after_fetch: has("after_fetch"),
            has_override_extract: has("override_extract"),
            has_after_extract: has("after_extract"),
            has_filter_item: has("filter_item"),
            has_before_store: has("before_store"),
            has_before_notify: has("before_notify"),
            has_before_webhook: has("before_webhook"),
            lua: Mutex::new(lua),
        })
    }

    pub fn has_filter_item(&self) -> bool {
        self.has_filter_item
    }

    pub fn has_before_webhook(&self) -> bool {
        self.has_before_webhook
    }

    pub(crate) fn format_hook_error(&self, hook_name: &'static str, err: anyhow::Error) -> String {
        match err.downcast::<mlua::Error>() {
            Ok(source) => LuaHookError {
                hook_name,
                hook_path: self.hook_path.clone(),
                job_name: self.job_name.clone(),
                source,
            }
            .to_string(),
            Err(err) => format!(
                "Lua {} error for job '{}' in {}: {}",
                hook_name,
                self.job_name,
                self.hook_path.display(),
                err
            ),
        }
    }

    pub(crate) fn log_hook_error(&self, hook_name: &'static str, err: anyhow::Error) {
        crate::t_eprintln!("{}, skipping hook", self.format_hook_error(hook_name, err));
    }

    pub async fn set_selector_matches(&self, count: usize) -> Result<()> {
        let lua = self.lua.lock().await;
        lua.globals().set("selector_matches", count)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
