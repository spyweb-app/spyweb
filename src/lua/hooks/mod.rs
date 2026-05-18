use crate::lua::engine;
use crate::services::db::Db;
use anyhow::Result;
use mlua::Lua;
use smol::lock::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    pub(crate) has_on_success: bool,
    pub(crate) has_on_error: bool,
    pub(crate) has_on_finally: bool,
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
            .field("has_on_success", &self.has_on_success)
            .field("has_on_error", &self.has_on_error)
            .field("has_on_finally", &self.has_on_finally)
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

        let defer_path = path.with_file_name("defer.lua");
        let has_defer_lua = defer_path.exists();
        if has_defer_lua {
            let defer_source = std::fs::read_to_string(&defer_path)
                .map_err(|e| anyhow::anyhow!("failed to read defer.lua: {e}"))?;
            lua.load(&defer_source)
                .set_name("defer.lua")
                .exec()
                .map_err(|e| anyhow::anyhow!("defer.lua load error for job '{job_name}': {e}"))?;
        }

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
            has_on_success: has_defer_lua && has("on_success"),
            has_on_error: has_defer_lua && has("on_error"),
            has_on_finally: has_defer_lua && has("on_finally"),
            lua: Mutex::new(lua),
        })
    }

    pub fn has_filter_item(&self) -> bool {
        self.has_filter_item
    }

    pub fn has_before_webhook(&self) -> bool {
        self.has_before_webhook
    }

    pub fn has_cycle_cleanup(&self) -> bool {
        self.has_on_success || self.has_on_error || self.has_on_finally
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

    pub async fn run_on_success(&self) {
        if !self.has_on_success {
            return;
        }

        let lua = self.lua.lock().await;
        match lua.globals().get::<mlua::Function>("on_success") {
            Ok(f) => {
                if let Err(e) = f.call_async::<()>(()).await {
                    crate::t_eprintln!(
                        "defer.lua on_success error for job '{}': {e}",
                        self.job_name
                    );
                }
            }
            Err(e) => {
                crate::t_eprintln!(
                    "defer.lua: on_success not callable for job '{}': {e}",
                    self.job_name
                );
            }
        }
    }

    pub async fn run_on_error(&self, err: &anyhow::Error) {
        if !self.has_on_error {
            return;
        }

        let lua = self.lua.lock().await;
        let err = err.to_string();
        match lua.globals().get::<mlua::Function>("on_error") {
            Ok(f) => {
                if let Err(e) = f.call_async::<()>(err).await {
                    crate::t_eprintln!("defer.lua on_error error for job '{}': {e}", self.job_name);
                }
            }
            Err(e) => {
                crate::t_eprintln!(
                    "defer.lua: on_error not callable for job '{}': {e}",
                    self.job_name
                );
            }
        }
    }

    pub async fn run_on_finally(&self) {
        if !self.has_on_finally {
            return;
        }

        let lua = self.lua.lock().await;
        match lua.globals().get::<mlua::Function>("on_finally") {
            Ok(f) => {
                if let Err(e) = f.call_async::<()>(()).await {
                    crate::t_eprintln!(
                        "defer.lua on_finally error for job '{}': {e}",
                        self.job_name
                    );
                }
            }
            Err(e) => {
                crate::t_eprintln!(
                    "defer.lua: on_finally not callable for job '{}': {e}",
                    self.job_name
                );
            }
        }
    }

    pub async fn cleanup_cycle_state(&self) {
        let lua = self.lua.lock().await;
        let _ = lua.globals().set("last_fetch", mlua::Value::Nil);
        let _ = lua.globals().set("selector_matches", mlua::Value::Nil);
        let _ = lua.globals().set("__deferred", mlua::Value::Nil);
        let _ = lua.gc_collect();
    }
}

#[cfg(test)]
mod tests;
