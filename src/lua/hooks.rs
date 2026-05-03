//! # Lua Hook Script Contract
//!
//! ```lua
//! -- All hooks are optional. Omit any you don't need.
//! -- Return the value (modified or not) to continue.
//! -- Return nil or false to drop/abort (where supported).
//! -- Lua globals persist across runs for the lifetime of the job.
//! -- Hot-reload resets all Lua state (new VM created from fresh script).
//!
//! -- Built-in async HTTP functions (available in all hooks):
//! --   http_get(url)        -> body string
//! --   http_post(url, body) -> body string
//! --   notify(title, body, timeout_ms?) -> nil
//! --   dump(value)          -> readable string for nested Lua tables/values
//!
//! -- Runtime-owned globals:
//! --   last_fetch -> last fetch result snapshot for this job VM
//! --                 overwritten on every fetch attempt
//!
//! function before_fetch(request)
//!     -- request.url: string (mutable)
//!     -- request.headers: table of string->string (mutable)
//!     -- return nil to skip this run entirely
//!     return request
//! end
//!
//! function after_fetch(fetch_result)
//!     -- fetch_result.ok: boolean
//!     -- fetch_result.request.url: string
//!     -- fetch_result.request.headers: table of string->string
//!     -- fetch_result.request.proxy: string|nil
//!     -- on success:
//!     --   fetch_result.response.status: number (read only)
//!     --   fetch_result.response.url: string (read only)
//!     --   fetch_result.response.headers: table of string->string (read only)
//!     --   fetch_result.response.body: string (mutable in the returned value)
//!     -- on error:
//!     --   fetch_result.response may still exist for HTTP errors like 403/500
//!     --   fetch_result.error.message: string
//!     --   fetch_result.error.kind: string
//!     -- return nil to skip extraction for this run
//!     -- return the fetch_result envelope to continue
//!     -- on success only response.body is accepted back
//!     -- on failure, return fetch_result.response = {...} to synthesize a response
//!     return fetch_result
//! end
//!
//! function after_extract(items)
//!     -- items: array of { fields = {...}, matches = {...} }
//!     -- batch hook — all extracted items at once, good for cross-item logic
//!     -- return filtered/modified array
//!     -- nil or empty = no items (not a full skip, just no items to process)
//!     return items
//! end
//!
//! function filter_item(item)
//!     -- item.fields: table of string->string (mutable)
//!     -- item.matches: array of matched keywords (read only)
//!     -- return nil to drop this item before it reaches the DB
//!     -- NOTE: if this function exists, built-in keyword filter is skipped entirely
//!     return item
//! end
//!
//! function before_store(items)
//!     -- items: array of item tables, same shape as filter_item
//!     -- last chance to drop items before dedup + insert
//!     -- return nil to skip storing AND notifying entirely for this run
//!     -- WARNING: items dropped here have no DB record and will
//!     --          appear as new items again on the next run
//!     return items
//! end
//!
//! function before_notify(items)
//!     -- items: only the NEW items that passed dedup (already stored at this point)
//!     -- return nil to silence notification entirely
//!     -- return modified array to change what gets notified
//!     return items
//! end
//!
//! function before_webhook(payload)
//!     -- payload: table with job_name, item_count, items
//!     -- return nil to skip webhook entirely
//!     -- return modified table to customize what gets POSTed
//!     return payload
//! end
//! ```

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use mlua::Lua;
use smol::lock::Mutex;

use crate::lua::{bindings, engine};
use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};
use crate::services::db::Db;

pub struct JobHooks {
    lua: Mutex<Lua>,
    job_name: String,
    hook_path: PathBuf,
    has_before_fetch: bool,
    has_override_fetch: bool,
    has_after_fetch: bool,
    has_after_extract: bool,
    has_filter_item: bool,
    has_before_store: bool,
    has_before_notify: bool,
    has_before_webhook: bool,
}

impl std::fmt::Debug for JobHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobHooks")
            .field("job_name", &self.job_name)
            .field("hook_path", &self.hook_path)
            .field("has_before_fetch", &self.has_before_fetch)
            .field("has_override_fetch", &self.has_override_fetch)
            .field("has_after_fetch", &self.has_after_fetch)
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
        #[cfg(feature = "luau")]
        let libs = mlua::StdLib::TABLE
            | mlua::StdLib::STRING
            | mlua::StdLib::UTF8
            | mlua::StdLib::MATH
            | mlua::StdLib::OS
            | mlua::StdLib::BIT
            | mlua::StdLib::COROUTINE;

        #[cfg(not(feature = "luau"))]
        let libs = mlua::StdLib::ALL;

        let lua = Lua::new_with(libs, mlua::LuaOptions::default())?;

        let job_dir = path.parent().map(|p| p.to_path_buf());
        bindings::register_http_and_fs(&lua, job_dir)?;

        let source = std::fs::read_to_string(path)?;
        let chunk_name = path.to_string_lossy();
        lua.load(&source).set_name(chunk_name.as_ref()).exec()?;
        bindings::register(&lua, db, job_name)?;

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

    fn format_hook_error(&self, hook_name: &'static str, err: anyhow::Error) -> String {
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

    fn log_hook_error(&self, hook_name: &'static str, err: anyhow::Error) {
        crate::t_eprintln!("{}, skipping hook", self.format_hook_error(hook_name, err));
    }

    pub async fn before_fetch(&self, req: RequestConfig) -> Result<Option<RequestConfig>> {
        if !self.has_before_fetch {
            return Ok(Some(req));
        }
        match self.try_before_fetch(&req).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("before_fetch", e);
                Ok(Some(req))
            }
        }
    }

    async fn try_before_fetch(&self, req: &RequestConfig) -> Result<Option<RequestConfig>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_fetch")?;
        let table = engine::request_to_lua(&lua, req)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(engine::lua_to_request(t, req.clone())?)),
            _ => Ok(Some(req.clone())),
        }
    }

    pub fn has_override_fetch(&self) -> bool {
        self.has_override_fetch
    }

    pub async fn override_fetch(&self, req: RequestConfig) -> Result<FetchAttempt> {
        match self.try_override_fetch(&req).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("override_fetch", e);
                Ok(FetchAttempt {
                    request: req,
                    proxy: None,
                    result: Err("override_fetch hook failed".into()),
                })
            }
        }
    }

    async fn try_override_fetch(&self, req: &RequestConfig) -> Result<FetchAttempt> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("override_fetch")?;
        let table = engine::request_to_lua(&lua, req)?;
        let ret = func.call_async::<mlua::Value>(table).await?;
        match ret {
            mlua::Value::Table(t) => {
                if let Some(err_msg) = t.get::<Option<String>>("error")? {
                    Ok(FetchAttempt {
                        request: req.clone(),
                        proxy: None,
                        result: Err(err_msg),
                    })
                } else {
                    let response = engine::lua_table_to_response(t)?;
                    Ok(FetchAttempt {
                        request: req.clone(),
                        proxy: response.proxy.clone(),
                        result: Ok(response),
                    })
                }
            }
            _ => Err(anyhow::anyhow!("override_fetch must return a response table")),
        }
    }

    pub async fn after_fetch(&self, attempt: FetchAttempt) -> Result<Option<RequestResult>> {
        self.set_last_fetch(&attempt).await?;
        if !self.has_after_fetch {
            return attempt
                .result
                .map(|res| Some(res))
                .map_err(anyhow::Error::msg);
        }
        match self.try_after_fetch(&attempt).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("after_fetch", e);
                match attempt.result {
                    Ok(res) => Ok(Some(res)),
                    Err(err) => Err(anyhow::Error::msg(err)),
                }
            }
        }
    }

    async fn set_last_fetch(&self, attempt: &FetchAttempt) -> Result<()> {
        let lua = self.lua.lock().await;
        let table = engine::fetch_result_to_lua(&lua, attempt)?;
        lua.globals().set("last_fetch", table)?;
        Ok(())
    }

    async fn try_after_fetch(&self, attempt: &FetchAttempt) -> Result<Option<RequestResult>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("after_fetch")?;
        let table = engine::fetch_result_to_lua(&lua, attempt)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => match &attempt.result {
                Ok(original) => Ok(Some(engine::lua_to_after_fetch_success_response(
                    t,
                    original.clone(),
                )?)),
                Err(_) => Ok(Some(engine::lua_to_after_fetch_error_response(t)?)),
            },
            _ => match &attempt.result {
                Ok(res) => Ok(Some(res.clone())),
                Err(err) => Err(anyhow::anyhow!(err.clone())),
            },
        }
    }

    // No shortcircuit — nil or empty table both become empty vec.
    // Batch hook, user sees all items at once.

    pub async fn after_extract(&self, items: Vec<ExtractedItem>) -> Result<Vec<ExtractedItem>> {
        if !self.has_after_extract {
            return Ok(items);
        }
        match self.try_after_extract(&items).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("after_extract", e);
                Ok(items)
            }
        }
    }

    async fn try_after_extract(&self, items: &[ExtractedItem]) -> Result<Vec<ExtractedItem>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("after_extract")?;
        let table = engine::items_to_lua(&lua, items)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(vec![]),
            mlua::Value::Table(t) => engine::lua_to_items(t, items.to_vec()),
            _ => Ok(items.to_vec()),
        }
    }

    // Single item in/out. nil/false = drop. Only runs if has_filter_item is true.

    pub async fn filter_item(&self, item: ExtractedItem) -> Result<Option<ExtractedItem>> {
        if !self.has_filter_item {
            return Ok(Some(item));
        }
        match self.try_filter_item(&item).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("filter_item", e);
                Ok(Some(item))
            }
        }
    }

    async fn try_filter_item(&self, item: &ExtractedItem) -> Result<Option<ExtractedItem>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("filter_item")?;
        let table = engine::item_to_lua(&lua, item)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(engine::lua_to_item(t, item.clone())?)),
            _ => Ok(Some(item.clone())),
        }
    }

    pub async fn before_store(
        &self,
        items: Vec<ExtractedItem>,
    ) -> Result<Option<Vec<ExtractedItem>>> {
        if !self.has_before_store {
            return Ok(Some(items));
        }
        match self.try_before_store(&items).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("before_store", e);
                Ok(Some(items))
            }
        }
    }

    async fn try_before_store(
        &self,
        items: &[ExtractedItem],
    ) -> Result<Option<Vec<ExtractedItem>>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_store")?;
        let table = engine::items_to_lua(&lua, items)?;
        match func.call_async::<mlua::Value>(table).await? {
            // nil = Ok(None), skip store and notify
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(engine::lua_to_items(t, items.to_vec())?)),
            _ => Ok(Some(items.to_vec())),
        }
    }

    pub async fn before_notify(
        &self,
        items: Vec<ExtractedItem>,
    ) -> Result<Option<Vec<ExtractedItem>>> {
        if !self.has_before_notify {
            return Ok(Some(items));
        }
        match self.try_before_notify(&items).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("before_notify", e);
                Ok(Some(items))
            }
        }
    }

    async fn try_before_notify(
        &self,
        items: &[ExtractedItem],
    ) -> Result<Option<Vec<ExtractedItem>>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_notify")?;
        let table = engine::items_to_lua(&lua, items)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(engine::lua_to_items(t, items.to_vec())?)),
            _ => Ok(Some(items.to_vec())),
        }
    }

    pub async fn before_webhook(
        &self,
        payload: serde_json::Value,
    ) -> Result<Option<serde_json::Value>> {
        if !self.has_before_webhook {
            return Ok(Some(payload));
        }
        match self.try_before_webhook(&payload).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("before_webhook", e);
                Ok(Some(payload))
            }
        }
    }

    async fn try_before_webhook(
        &self,
        payload: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_webhook")?;
        let table = engine::json_to_lua(&lua, payload)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(engine::lua_to_json(&mlua::Value::Table(t))?)),
            _ => Ok(Some(payload.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::db::Db;
    use std::fs;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("spyweb-{name}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn hook_errors_use_hook_file_path_instead_of_rust_source() {
        let dir = unique_test_dir("hook-traceback");
        fs::create_dir_all(&dir).unwrap();
        let hook_path = dir.join("hooks.lua");
        fs::write(
            &hook_path,
            r#"
function after_fetch(fetch_result)
    return fetch_result.response.status.code
end
"#,
        )
        .unwrap();

        let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
        let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();

        let attempt = FetchAttempt {
            request: RequestConfig {
                url: "https://example.com".into(),
                headers: std::collections::HashMap::new(),
            },
            proxy: None,
            result: Ok(RequestResult {
                url: "https://example.com".into(),
                status: 200,
                headers: std::collections::HashMap::new(),
                body: "<html></html>".into(),
                proxy: None,
            }),
        };

        let err = smol::block_on(async { hooks.try_after_fetch(&attempt).await }).unwrap_err();
        let rendered = hooks.format_hook_error("after_fetch", err);

        assert!(rendered.contains("Lua after_fetch error for job 'test_job'"));
        assert!(rendered.contains(hook_path.to_string_lossy().as_ref()));
        assert!(rendered.contains("attempt to index"));
        assert!(rendered.contains("hooks.lua:"));
        assert!(!rendered.contains("src/lua/hooks.rs"));

        let _ = fs::remove_file(dir.join("test.redb"));
        let _ = fs::remove_file(&hook_path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn test_override_fetch_hook() {
        let dir = unique_test_dir("override-fetch");
        fs::create_dir_all(&dir).unwrap();
        let hook_path = dir.join("hooks.lua");
        fs::write(
            &hook_path,
            r#"
function override_fetch(request)
    if request.url == "https://fail.com" then
        return { error = "simulated failure" }
    end
    return {
        status = 200,
        body = "overridden body for " .. request.url,
        url = request.url
    }
end
"#,
        )
        .unwrap();

        let db = Arc::new(Db::open(dir.join("test.redb").to_str().unwrap()).unwrap());
        let hooks = JobHooks::load(&hook_path, db, "test_job").unwrap();
        assert!(hooks.has_override_fetch());

        let req = RequestConfig {
            url: "https://example.com".into(),
            headers: Default::default(),
        };

        // Test Success
        let attempt = smol::block_on(hooks.override_fetch(req.clone())).unwrap();
        let res = attempt.result.unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(res.body, "overridden body for https://example.com");

        // Test Lua-returned error
        let req_fail = RequestConfig {
            url: "https://fail.com".into(),
            headers: Default::default(),
        };
        let attempt_fail = smol::block_on(hooks.override_fetch(req_fail)).unwrap();
        assert_eq!(attempt_fail.result.unwrap_err(), "simulated failure");

        let _ = fs::remove_file(dir.join("test.redb"));
        let _ = fs::remove_file(&hook_path);
        let _ = fs::remove_dir(&dir);
    }
}
