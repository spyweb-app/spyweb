use super::JobHooks;
use crate::lua::conversions;
use crate::scraper::extractor::ExtractedItem;
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestResult};
use anyhow::Result;

impl JobHooks {
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

    pub(crate) async fn try_before_fetch(
        &self,
        req: &RequestConfig,
    ) -> Result<Option<RequestConfig>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_fetch")?;
        let table = conversions::request_to_lua(&lua, req)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(conversions::lua_to_request(t, req.clone())?)),
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

    pub(crate) async fn try_override_fetch(&self, req: &RequestConfig) -> Result<FetchAttempt> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("override_fetch")?;
        let table = conversions::request_to_lua(&lua, req)?;
        let ret = func.call_async::<mlua::Value>(table).await?;
        let _ = lua.gc_collect();
        match ret {
            mlua::Value::Table(t) => {
                if let Some(err_msg) = t.get::<Option<String>>("error")? {
                    Ok(FetchAttempt {
                        request: req.clone(),
                        proxy: None,
                        result: Err(err_msg),
                    })
                } else {
                    let response = conversions::lua_table_to_response(t)?;
                    Ok(FetchAttempt {
                        request: req.clone(),
                        proxy: response.proxy.clone(),
                        result: Ok(response),
                    })
                }
            }
            _ => Err(anyhow::anyhow!(
                "override_fetch must return a response table"
            )),
        }
    }

    pub async fn after_fetch(&self, attempt: FetchAttempt) -> Result<Option<RequestResult>> {
        self.set_last_fetch(&attempt).await?;
        if !self.has_after_fetch {
            return attempt.result.map(Some).map_err(anyhow::Error::msg);
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
        let table = conversions::fetch_result_to_lua(&lua, attempt)?;
        lua.globals().set("last_fetch", table)?;
        Ok(())
    }

    pub(crate) async fn try_after_fetch(
        &self,
        attempt: &FetchAttempt,
    ) -> Result<Option<RequestResult>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("after_fetch")?;
        let table = conversions::fetch_result_to_lua(&lua, attempt)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => match &attempt.result {
                Ok(original) => Ok(Some(conversions::lua_to_after_fetch_success_response(
                    t,
                    original.clone(),
                )?)),
                Err(_) => Ok(Some(conversions::lua_to_after_fetch_error_response(t)?)),
            },
            _ => match &attempt.result {
                Ok(res) => Ok(Some(res.clone())),
                Err(err) => Err(anyhow::anyhow!(err.clone())),
            },
        }
    }

    pub fn has_override_extract(&self) -> bool {
        self.has_override_extract
    }

    pub async fn override_extract(&self, response: &RequestResult) -> Result<Vec<ExtractedItem>> {
        match self.try_override_extract(response).await {
            Ok(result) => Ok(result),
            Err(e) => {
                self.log_hook_error("override_extract", e);
                Ok(vec![])
            }
        }
    }

    pub(crate) async fn try_override_extract(
        &self,
        response: &RequestResult,
    ) -> Result<Vec<ExtractedItem>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("override_extract")?;
        let table = conversions::response_to_lua(&lua, response)?;
        let ret = func.call_async::<mlua::Value>(table).await?;
        match ret {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(vec![]),
            mlua::Value::Table(t) => conversions::lua_to_items(t, vec![]),
            _ => Ok(vec![]),
        }
    }

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

    pub(crate) async fn try_after_extract(
        &self,
        items: &[ExtractedItem],
    ) -> Result<Vec<ExtractedItem>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("after_extract")?;
        let table = conversions::items_to_lua(&lua, items)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(vec![]),
            mlua::Value::Table(t) => conversions::lua_to_items(t, items.to_vec()),
            _ => Ok(items.to_vec()),
        }
    }

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

    pub(crate) async fn try_filter_item(
        &self,
        item: &ExtractedItem,
    ) -> Result<Option<ExtractedItem>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("filter_item")?;
        let table = conversions::item_to_lua(&lua, item)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(conversions::lua_to_item(t, item.clone())?)),
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

    pub(crate) async fn try_before_store(
        &self,
        items: &[ExtractedItem],
    ) -> Result<Option<Vec<ExtractedItem>>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_store")?;
        let table = conversions::items_to_lua(&lua, items)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(conversions::lua_to_items(t, items.to_vec())?)),
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

    pub(crate) async fn try_before_notify(
        &self,
        items: &[ExtractedItem],
    ) -> Result<Option<Vec<ExtractedItem>>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_notify")?;
        let table = conversions::items_to_lua(&lua, items)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(conversions::lua_to_items(t, items.to_vec())?)),
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

    pub(crate) async fn try_before_webhook(
        &self,
        payload: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>> {
        let lua = self.lua.lock().await;
        let func: mlua::Function = lua.globals().get("before_webhook")?;
        let table = conversions::json_to_lua(&lua, payload)?;
        match func.call_async::<mlua::Value>(table).await? {
            mlua::Value::Nil | mlua::Value::Boolean(false) => Ok(None),
            mlua::Value::Table(t) => Ok(Some(conversions::lua_to_json(&mlua::Value::Table(t))?)),
            _ => Ok(Some(payload.clone())),
        }
    }
}
