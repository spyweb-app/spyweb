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
//!     -- on success:
//!     --   fetch_result.body: string (mutable)
//!     --   fetch_result.status: number (read only in returned response)
//!     --   fetch_result.url: string (read only in returned response)
//!     --   fetch_result.headers: table of string->string (read only in returned response)
//!     --   fetch_result.proxy: string|nil (read only in returned response)
//!     -- on error:
//!     --   fetch_result.error: string
//!     -- return nil to skip extraction for this run
//!     -- return a response-like table to continue extraction, even after a fetch error
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
use std::sync::Arc;

use anyhow::Result;
use mlua::Lua;
use smol::lock::Mutex;

use crate::lua::{bindings, engine};
use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{RequestConfig, RequestResult};
use crate::services::db::Db;

pub struct JobHooks {
    lua: Mutex<Lua>,
    has_before_fetch: bool,
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
            .field("has_before_fetch", &self.has_before_fetch)
            .field("has_after_fetch", &self.has_after_fetch)
            .field("has_after_extract", &self.has_after_extract)
            .field("has_filter_item", &self.has_filter_item)
            .field("has_before_store", &self.has_before_store)
            .field("has_before_notify", &self.has_before_notify)
            .field("has_before_webhook", &self.has_before_webhook)
            .finish()
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
        lua.load(&source).exec()?;
        bindings::register(&lua, db, job_name)?;

        let has = |name: &str| -> bool {
            lua.globals()
                .get::<Option<mlua::Function>>(name)
                .unwrap_or(None)
                .is_some()
        };

        Ok(Self {
            has_before_fetch: has("before_fetch"),
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

    pub async fn before_fetch(&self, req: RequestConfig) -> Result<Option<RequestConfig>> {
        if !self.has_before_fetch {
            return Ok(Some(req));
        }
        match self.try_before_fetch(&req).await {
            Ok(result) => Ok(result),
            Err(e) => {
                crate::t_eprintln!("Lua before_fetch error: {e}, skipping hook");
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

    pub async fn after_fetch(&self, res: Result<RequestResult>) -> Result<Option<RequestResult>> {
        self.set_last_fetch(&res).await?;
        if !self.has_after_fetch {
            return res.map(Some);
        }
        match self.try_after_fetch(&res).await {
            Ok(result) => Ok(result),
            Err(e) => {
                crate::t_eprintln!("Lua after_fetch error: {e}, skipping hook");
                match res {
                    Ok(res) => Ok(Some(res)),
                    Err(err) => Err(err),
                }
            }
        }
    }

    async fn set_last_fetch(&self, res: &Result<RequestResult>) -> Result<()> {
        let lua = self.lua.lock().await;
        let table = engine::fetch_result_to_lua(&lua, res)?;
        lua.globals().set("last_fetch", table)?;
        Ok(())
    }

    async fn try_after_fetch(&self, res: &Result<RequestResult>) -> Result<Option<RequestResult>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("after_fetch")?;
        let table = engine::fetch_result_to_lua(&lua, res)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => match res {
                Ok(original) => Ok(Some(engine::lua_to_response_body_only(t, original.clone())?)),
                Err(_) => Ok(Some(engine::lua_to_response(t, None)?)),
            },
            _ => match res {
                Ok(res) => Ok(Some(res.clone())),
                Err(err) => Err(anyhow::anyhow!(err.to_string())),
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
                crate::t_eprintln!("Lua after_extract error: {e}, skipping hook");
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
                crate::t_eprintln!("Lua filter_item error: {e}, skipping hook");
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
                crate::t_eprintln!("Lua before_store error: {e}, skipping hook");
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
                crate::t_eprintln!("Lua before_notify error: {e}, skipping hook");
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
                crate::t_eprintln!("Lua before_webhook error: {e}, skipping hook");
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
