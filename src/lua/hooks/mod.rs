use crate::lua::engine;
use crate::services::db::Db;
use anyhow::Result;
use mlua::{Lua, Table, Value};
use smol::lock::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
pub mod stages;
pub(crate) mod telemetry;

pub(crate) use telemetry::TelemetryHandle;

pub(crate) const HOOK_BEFORE_FETCH: u16 = 1 << 0;
pub(crate) const HOOK_OVERRIDE_FETCH: u16 = 1 << 1;
pub(crate) const HOOK_AFTER_FETCH: u16 = 1 << 2;
pub(crate) const HOOK_OVERRIDE_EXTRACT: u16 = 1 << 3;
pub(crate) const HOOK_AFTER_EXTRACT: u16 = 1 << 4;
pub(crate) const HOOK_FILTER_ITEM: u16 = 1 << 5;
pub(crate) const HOOK_BEFORE_STORE: u16 = 1 << 6;
pub(crate) const HOOK_BEFORE_NOTIFY: u16 = 1 << 7;
pub(crate) const HOOK_BEFORE_WEBHOOK: u16 = 1 << 8;
pub(crate) const HOOK_ON_SUCCESS: u16 = 1 << 9;
pub(crate) const HOOK_ON_ERROR: u16 = 1 << 10;
pub(crate) const HOOK_ON_FINALLY: u16 = 1 << 11;
pub(crate) const HOOK_ON_FINISHED: u16 = 1 << 12;

pub(crate) const RESERVED_CTX_KEYS: &[&str] = &[
    "last_fetch",
    "selector_matches",
    "telemetry",
    "__deferred",
    "filter_error",
    "worker_id",
];

pub(crate) fn source_uses_cdp(source: &str) -> bool {
    source.contains("cdp.")
}

pub struct JobHooks {
    pub(crate) lua: Mutex<Lua>,
    pub(crate) job_name: String,
    pub(crate) hook_path: PathBuf,
    pub(crate) hook_mask: u16,
}

impl std::fmt::Debug for JobHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobHooks")
            .field("job_name", &self.job_name)
            .field("hook_path", &self.hook_path)
            .field("hook_mask", &format_args!("{:#06x}", self.hook_mask))
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
        let source = std::fs::read_to_string(path)?;
        let defer_path = path.with_file_name("defer.lua");
        let defer_source = if defer_path.exists() {
            Some(
                std::fs::read_to_string(&defer_path)
                    .map_err(|e| anyhow::anyhow!("failed to read defer.lua: {e}"))?,
            )
        } else {
            None
        };
        let uses_cdp = source_uses_cdp(&source)
            || defer_source
                .as_deref()
                .map(source_uses_cdp)
                .unwrap_or(false);

        let lua = engine::create_engine(job_dir, db, job_name, uses_cdp)?;
        let chunk_name = path.to_string_lossy();
        lua.load(&source).set_name(chunk_name.as_ref()).exec()?;

        let has_defer_lua = defer_source.is_some();
        if let Some(defer_source) = defer_source {
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

        let mut hook_mask = 0u16;
        if has("before_fetch") {
            hook_mask |= HOOK_BEFORE_FETCH;
        }
        if has("override_fetch") {
            hook_mask |= HOOK_OVERRIDE_FETCH;
        }
        if has("after_fetch") {
            hook_mask |= HOOK_AFTER_FETCH;
        }
        if has("override_extract") {
            hook_mask |= HOOK_OVERRIDE_EXTRACT;
        }
        if has("after_extract") {
            hook_mask |= HOOK_AFTER_EXTRACT;
        }
        if has("filter_item") {
            hook_mask |= HOOK_FILTER_ITEM;
        }
        if has("before_store") {
            hook_mask |= HOOK_BEFORE_STORE;
        }
        if has("before_notify") {
            hook_mask |= HOOK_BEFORE_NOTIFY;
        }
        if has("before_webhook") {
            hook_mask |= HOOK_BEFORE_WEBHOOK;
        }
        if has_defer_lua {
            if has("on_success") {
                hook_mask |= HOOK_ON_SUCCESS;
            }
            if has("on_error") {
                hook_mask |= HOOK_ON_ERROR;
            }
            if has("on_finally") {
                hook_mask |= HOOK_ON_FINALLY;
            }
        }
        if has("on_finished") {
            hook_mask |= HOOK_ON_FINISHED;
        }

        Ok(Self {
            job_name: job_name.to_string(),
            hook_path: path.to_path_buf(),
            hook_mask,
            lua: Mutex::new(lua),
        })
    }

    pub fn has_filter_item(&self) -> bool {
        self.hook_mask & HOOK_FILTER_ITEM != 0
    }

    pub fn has_before_webhook(&self) -> bool {
        self.hook_mask & HOOK_BEFORE_WEBHOOK != 0
    }

    pub fn has_cycle_cleanup(&self) -> bool {
        self.hook_mask & (HOOK_ON_SUCCESS | HOOK_ON_ERROR | HOOK_ON_FINALLY) != 0
    }

    pub async fn new_cycle_context(&self, worker_id: usize) -> Result<Table> {
        let lua = self.lua.lock().await;
        let ctx = lua.create_table()?;
        let shared = lua.create_table()?;
        let deferred = lua.create_table()?;
        let store = lua.create_table()?;

        store.set("worker_id", worker_id)?;
        store.set("__deferred", deferred.clone())?;
        ctx.set("shared", shared)?;

        let guard = lua.create_table()?;
        guard.set("__store", store.clone())?;
        guard.set(
            "__index",
            lua.create_function(move |_lua, (tbl, key): (Table, mlua::Value)| {
                if let mlua::Value::String(s) = &key
                    && let Ok(k) = s.to_str()
                    && RESERVED_CTX_KEYS.contains(&&*k)
                {
                    return store.get::<mlua::Value>(k);
                }
                tbl.raw_get(key)
            })?,
        )?;
        guard.set(
            "__newindex",
            lua.create_function(
                move |_lua, (tbl, key, val): (Table, mlua::Value, mlua::Value)| {
                    if let mlua::Value::String(s) = &key
                        && let Ok(k) = s.to_str()
                        && RESERVED_CTX_KEYS.contains(&&*k)
                    {
                        return Err(mlua::Error::external(format!(
                            "cannot overwrite reserved ctx field '{}'",
                            k
                        )));
                    }
                    tbl.raw_set(key, val)
                },
            )?,
        )?;
        guard.set("__metatable", false)?;
        ctx.set_metatable(Some(guard))?;

        Ok(ctx)
    }

    pub(crate) fn ctx_store(ctx: &Table) -> Result<Table> {
        let meta = ctx
            .metatable()
            .ok_or_else(|| anyhow::anyhow!("ctx has no metatable"))?;
        let store: Table = meta.raw_get("__store")?;
        Ok(store)
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

    pub async fn set_selector_matches(&self, ctx: &mlua::Table, count: usize) -> Result<()> {
        Self::ctx_store(ctx)?.raw_set("selector_matches", count)?;
        Ok(())
    }

    pub async fn run_on_success(&self, ctx: &Table) {
        if self.hook_mask & HOOK_ON_SUCCESS == 0 {
            return;
        }

        let lua = self.lua.lock().await;
        if let Err(e) = lua.set_named_registry_value("active_ctx", ctx.clone()) {
            crate::t_eprintln!(
                "defer.lua on_success: failed to set active_ctx for job '{}': {e}",
                self.job_name
            );
            return;
        }
        match lua.globals().get::<mlua::Function>("on_success") {
            Ok(f) => {
                if let Err(e) = f.call_async::<()>((ctx.clone(),)).await {
                    crate::t_eprintln!(
                        "defer.lua on_success error for job '{}': {e}",
                        self.job_name
                    );
                }
                stages::run_deferred(&lua, ctx, "on_success");
            }
            Err(e) => {
                crate::t_eprintln!(
                    "defer.lua: on_success not callable for job '{}': {e}",
                    self.job_name
                );
            }
        }
    }

    pub async fn run_on_error(&self, ctx: &Table, err: &anyhow::Error) {
        if self.hook_mask & HOOK_ON_ERROR == 0 {
            return;
        }

        let lua = self.lua.lock().await;
        let err = err.to_string();
        if let Err(e) = lua.set_named_registry_value("active_ctx", ctx.clone()) {
            crate::t_eprintln!(
                "defer.lua on_error: failed to set active_ctx for job '{}': {e}",
                self.job_name
            );
            return;
        }
        match lua.globals().get::<mlua::Function>("on_error") {
            Ok(f) => {
                if let Err(e) = f.call_async::<()>((err, ctx.clone())).await {
                    crate::t_eprintln!("defer.lua on_error error for job '{}': {e}", self.job_name);
                }
                stages::run_deferred(&lua, ctx, "on_error");
            }
            Err(e) => {
                crate::t_eprintln!(
                    "defer.lua: on_error not callable for job '{}': {e}",
                    self.job_name
                );
            }
        }
    }

    pub async fn run_on_finally(&self, ctx: &Table) {
        if self.hook_mask & HOOK_ON_FINALLY == 0 {
            return;
        }

        let lua = self.lua.lock().await;
        if let Err(e) = lua.set_named_registry_value("active_ctx", ctx.clone()) {
            crate::t_eprintln!(
                "defer.lua on_finally: failed to set active_ctx for job '{}': {e}",
                self.job_name
            );
            return;
        }
        match lua.globals().get::<mlua::Function>("on_finally") {
            Ok(f) => {
                if let Err(e) = f.call_async::<()>((ctx.clone(),)).await {
                    crate::t_eprintln!(
                        "defer.lua on_finally error for job '{}': {e}",
                        self.job_name
                    );
                }
                stages::run_deferred(&lua, ctx, "on_finally");
            }
            Err(e) => {
                crate::t_eprintln!(
                    "defer.lua: on_finally not callable for job '{}': {e}",
                    self.job_name
                );
            }
        }
    }

    pub async fn run_on_finished(&self) {
        if self.hook_mask & HOOK_ON_FINISHED == 0 {
            return;
        }

        let lua = self.lua.lock().await;
        match lua.globals().get::<mlua::Function>("on_finished") {
            Ok(f) => {
                if let Err(e) = f.call_async::<()>(()).await {
                    crate::t_eprintln!("on_finished error for job '{}': {e}", self.job_name);
                }
            }
            Err(e) => {
                crate::t_eprintln!("on_finished not callable for job '{}': {e}", self.job_name);
            }
        }
    }

    pub async fn cleanup_cycle_state(&self, ctx: &Table) {
        let store = match Self::ctx_store(ctx) {
            Ok(s) => s,
            Err(_) => return,
        };
        for &key in RESERVED_CTX_KEYS {
            let _ = store.raw_set(key, Value::Nil);
        }
    }

    /// Run a closure with access to the locked Lua VM.
    /// Only available for tests and internal use.
    pub async fn with_lua<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&Lua) -> T,
    {
        let lua = self.lua.lock().await;
        f(&lua)
    }
}

#[cfg(test)]
mod tests;
