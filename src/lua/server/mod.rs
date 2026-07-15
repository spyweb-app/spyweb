use crate::services::db::Db;
use crate::services::server::types;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_BODY_SIZE: usize = 10 * 1024 * 1024; // 10MB

/// Hop-by-hop headers that Lua handlers are not allowed to set in responses.
const FORBIDDEN_RESPONSE_HEADERS: [&str; 6] = [
    "transfer-encoding",
    "content-length",
    "connection",
    "keep-alive",
    "upgrade",
    "trailer",
];

struct LuaContext {
    body: Option<String>,
    headers: std::collections::HashMap<String, String>,
    query: std::collections::HashMap<String, String>,
    path: String,
    path_args: Vec<String>,
    client_ip: String,
    method: String,
}

impl LuaContext {
    fn from_api(req: &types::Request, method: &str, path_args: Vec<String>) -> Self {
        Self {
            body: extract_body(req),
            headers: extract_headers(req),
            query: extract_query(req),
            path: req.url.clone(),
            path_args,
            client_ip: req.client_ip.clone().unwrap_or_default(),
            method: method.to_string(),
        }
    }
}

pub struct ApiServer {
    db: Arc<Db>,
    init_source: String,
    error_log_path: PathBuf,
}

impl ApiServer {
    pub fn new(db: Arc<Db>, init_source: String, error_log_path: PathBuf) -> Self {
        Self {
            db,
            init_source,
            error_log_path,
        }
    }

    #[cfg(test)]
    pub fn for_test(db: Arc<Db>, init_source: &str) -> Self {
        Self {
            db,
            init_source: init_source.to_string(),
            error_log_path: PathBuf::from("/dev/null"),
        }
    }

    pub fn handle(
        &self,
        method: &str,
        name: &str,
        path_args: Vec<String>,
        req: &types::Request,
        is_public: bool,
    ) -> types::Response {
        let ctx_fields = LuaContext::from_api(req, method, path_args);
        let method = method.to_string();
        let name = name.to_string();
        let init_source = self.init_source.clone();
        let db = self.db.clone();
        let error_log_path = self.error_log_path.clone();

        smol::block_on(async move {
            let lua = match crate::lua::engine::create_engine(
                Some(std::path::PathBuf::from("server")),
                db,
                "__server",
                false,
            ) {
                Ok(l) => l,
                Err(e) => {
                    return types::Response::text(format!("Engine error: {}", e)).with_status(500);
                }
            };

            setup_defer_registry(&lua, &error_log_path).await;
            inject_method_tables(&lua, &error_log_path).await;

            #[cfg(feature = "luau")]
            setup_timeout(&lua);

            if let Err(e) = lua
                .load(&init_source)
                .set_name("server/init.lua")
                .exec_async()
                .await
            {
                return handle_error(&name, e, error_log_path).await;
            }

            let self_table = build_context_table(&lua, &ctx_fields);
            let func = match resolve_handler(&lua, &method, &name, is_public) {
                Ok(f) => f,
                Err(r) => return r,
            };

            let result = func.call_async::<mlua::Value>((self_table,)).await;
            let response = format_response(&lua, result, &name, &error_log_path).await;
            run_deferred(&lua, &error_log_path).await;
            response
        })
    }
}

fn extract_body(req: &types::Request) -> Option<String> {
    let bytes = req.body_bytes().unwrap_or_default();
    let mut s = String::from_utf8_lossy(bytes).to_string();
    if s.len() > MAX_BODY_SIZE {
        crate::t_eprintln!(
            "Request body truncated from {} to {} bytes",
            s.len(),
            MAX_BODY_SIZE
        );
        s.truncate(MAX_BODY_SIZE);
    }
    Some(s)
}

fn extract_headers(req: &types::Request) -> std::collections::HashMap<String, String> {
    req.headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn extract_query(req: &types::Request) -> std::collections::HashMap<String, String> {
    req.query_params.clone()
}

async fn log_server_error(msg: &str, error_log_path: &std::path::Path) {
    crate::t_eprintln!("{}", msg);
    let task = crate::services::io::IoTask {
        path: error_log_path.to_path_buf(),
        content: format!("{}\n", msg).into_bytes(),
        op: crate::services::io::IoOp::Append,
        add_timestamp: true,
        reply: None,
    };
    let _ = crate::services::io::send_task(task).await;
}

async fn setup_defer_registry(lua: &mlua::Lua, error_log_path: &std::path::Path) {
    let ctx = match lua.create_table() {
        Ok(t) => t,
        Err(e) => {
            log_server_error(
                &format!("API server: failed to create defer context: {}", e),
                error_log_path,
            )
            .await;
            return;
        }
    };
    let deferred = match lua.create_table() {
        Ok(t) => t,
        Err(e) => {
            log_server_error(
                &format!("API server: failed to create deferred table: {}", e),
                error_log_path,
            )
            .await;
            return;
        }
    };
    if let Err(e) = ctx.set("__deferred", deferred) {
        log_server_error(
            &format!("API server: failed to set __deferred: {}", e),
            error_log_path,
        )
        .await;
    }
    if let Err(e) = lua.set_named_registry_value("active_ctx", ctx) {
        log_server_error(
            &format!("API server: failed to set active_ctx: {}", e),
            error_log_path,
        )
        .await;
    }
}

async fn inject_method_tables(lua: &mlua::Lua, error_log_path: &std::path::Path) {
    for m in &["get", "post", "put", "patch", "delete", "all"] {
        if let Err(e) = lua.globals().set(*m, lua.create_table().unwrap()) {
            log_server_error(
                &format!("API server: failed to inject method table '{}': {}", m, e),
                error_log_path,
            )
            .await;
        }
    }

    let public = lua.create_table().unwrap();
    for m in &["get", "post", "put", "patch", "delete", "all"] {
        if let Err(e) = public.set(*m, lua.create_table().unwrap()) {
            log_server_error(
                &format!("API server: failed to inject public.{} table: {}", m, e),
                error_log_path,
            )
            .await;
        }
    }
    if let Err(e) = lua.globals().set("public", public) {
        log_server_error(
            &format!("API server: failed to inject public table: {}", e),
            error_log_path,
        )
        .await;
    }
}

#[cfg(feature = "luau")]
fn setup_timeout(lua: &mlua::Lua) {
    let timeout_flag = Arc::new(AtomicBool::new(false));
    let timeout_flag_clone = timeout_flag.clone();

    smol::spawn(async move {
        smol::Timer::after(std::time::Duration::from_secs(30)).await;
        timeout_flag_clone.store(true, Ordering::Relaxed);
    })
    .detach();

    lua.set_interrupt(move |_| {
        if timeout_flag.load(Ordering::Relaxed) {
            Err(mlua::Error::RuntimeError(
                "Script execution timed out".into(),
            ))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
}

fn build_context_table(lua: &mlua::Lua, ctx: &LuaContext) -> mlua::Table {
    let table = lua.create_table().unwrap();
    let _ = table.set("body", ctx.body.clone());
    let _ = table.set("headers", ctx.headers.clone());
    let _ = table.set("query", ctx.query.clone());
    let _ = table.set("path", ctx.path.as_str());
    let _ = table.set("path_args", ctx.path_args.clone());
    let _ = table.set("client_ip", ctx.client_ip.as_str());
    let _ = table.set("method", ctx.method.as_str());
    table
}

fn resolve_handler(
    lua: &mlua::Lua,
    method: &str,
    name: &str,
    is_public: bool,
) -> Result<mlua::Function, types::Response> {
    let method_lower = method.to_lowercase();
    let globals = lua.globals();

    if is_public {
        if let Ok(pub_table) = globals.get::<mlua::Table>("public") {
            if let Ok(m_table) = pub_table.get::<mlua::Table>(method_lower.as_str())
                && let Ok(f) = m_table.get::<mlua::Function>(name)
            {
                return Ok(f);
            }
            if let Ok(all_table) = pub_table.get::<mlua::Table>("all")
                && let Ok(f) = all_table.get::<mlua::Function>(name)
            {
                return Ok(f);
            }
        }
    } else {
        if let Ok(m_table) = globals.get::<mlua::Table>(method_lower.as_str())
            && let Ok(f) = m_table.get::<mlua::Function>(name)
        {
            return Ok(f);
        }

        if let Ok(all_table) = globals.get::<mlua::Table>("all")
            && let Ok(f) = all_table.get::<mlua::Function>(name)
        {
            return Ok(f);
        }
    }

    Err(types::Response::text("Not Found").with_status(404))
}

async fn format_response(
    lua: &mlua::Lua,
    result: Result<mlua::Value, mlua::Error>,
    name: &str,
    error_log_path: &std::path::Path,
) -> types::Response {
    match result {
        Ok(mlua::Value::Table(t)) => format_table_response(lua, &t),
        Ok(mlua::Value::String(s)) => types::Response::text(s.to_string_lossy()).with_status(200),
        Ok(mlua::Value::Nil) => types::Response::text("").with_status(200),
        Ok(_) => types::Response::text("Invalid response type from Lua handler").with_status(500),
        Err(e) => handle_error(name, e, error_log_path.to_path_buf()).await,
    }
}

fn format_table_response(lua: &mlua::Lua, t: &mlua::Table) -> types::Response {
    let status = t.get::<i32>("status").unwrap_or(200);
    let status = status.clamp(100, 599) as u16;
    let body_val = t
        .get::<mlua::Value>("body")
        .unwrap_or(mlua::Value::String(lua.create_string("").unwrap()));

    let mut content_type = None;
    let mut body_bytes = Vec::new();

    match body_val {
        mlua::Value::String(s) => {
            body_bytes = s.as_bytes().to_vec();
        }
        mlua::Value::Table(tbl) => {
            let val = mlua::Value::Table(tbl);
            if let Ok(json) = serde_json::to_string(&val) {
                body_bytes = json.into_bytes();
                content_type = Some("application/json".to_string());
            }
        }
        mlua::Value::Number(n) => {
            body_bytes = n.to_string().into_bytes();
        }
        mlua::Value::Integer(n) => {
            body_bytes = n.to_string().into_bytes();
        }
        mlua::Value::Boolean(b) => {
            body_bytes = b.to_string().into_bytes();
        }
        _ => {}
    }

    let mut headers: Vec<(String, String)> = Vec::new();

    if let Some(ct) = content_type {
        headers.push(("Content-Type".into(), ct));
    }

    if let Ok(h_tbl) = t.get::<mlua::Table>("headers") {
        for (k, v) in h_tbl.pairs::<String, String>().flatten() {
            if k.is_empty()
                || k.contains('\n')
                || k.contains('\r')
                || v.contains('\n')
                || v.contains('\r')
            {
                continue;
            }
            let lower = k.to_lowercase();
            if FORBIDDEN_RESPONSE_HEADERS.contains(&lower.as_str()) {
                continue;
            }
            headers.push((k, v));
        }
    }

    types::Response {
        status,
        headers,
        body: types::ResponseBody::Bytes(body_bytes),
    }
}

async fn run_deferred(lua: &mlua::Lua, error_log_path: &std::path::Path) {
    let ctx = match lua.named_registry_value::<mlua::Table>("active_ctx") {
        Ok(t) => t,
        Err(e) => {
            log_server_error(
                &format!("API server: failed to get active_ctx for defer: {}", e),
                error_log_path,
            )
            .await;
            return;
        }
    };
    loop {
        let deferred = match ctx.get::<mlua::Table>("__deferred") {
            Ok(t) => t,
            Err(_) => return,
        };
        let len = deferred.raw_len();
        if len == 0 {
            return;
        }
        // swap in fresh table so new defers from this iteration are not lost
        if let Ok(fresh) = lua.create_table() {
            let _ = ctx.set("__deferred", fresh);
        }
        for i in (1..=len).rev() {
            if let Ok(f) = deferred.get::<mlua::Function>(i)
                && let Err(e) = f.call_async::<mlua::Value>((ctx.clone(),)).await
            {
                let msg = format!("API server: deferred function failed: {}", e);
                crate::t_eprintln!("{}", msg);
                log_server_error(&msg, error_log_path).await;
            }
        }
    }
}

async fn handle_error(name: &str, e: mlua::Error, error_log_path: PathBuf) -> types::Response {
    crate::t_eprintln!("API Server error in endpoint '{}': {}", name, e);
    let msg = format!("Error in endpoint '{}': {}\n", name, e);
    let task = crate::services::io::IoTask {
        path: error_log_path,
        content: msg.into_bytes(),
        op: crate::services::io::IoOp::Append,
        add_timestamp: true,
        reply: None,
    };
    let _ = crate::services::io::send_task(task).await;
    types::Response::json(&serde_json::json!({ "error": "Internal Server Error" })).with_status(500)
}

#[cfg(test)]
mod tests;
