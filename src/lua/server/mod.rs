use crate::services::db::Db;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_BODY_SIZE: usize = 10 * 1024 * 1024; // 10MB

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
        req: &rouille::Request,
    ) -> rouille::Response {
        let body = extract_body(req);
        let headers = extract_headers(req);
        let query = extract_query(req);
        let path = req.url().to_string();
        let client_ip = req.remote_addr().to_string();
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
                    return rouille::Response::text(format!("Engine error: {}", e))
                        .with_status_code(500);
                }
            };

            setup_defer_registry(&lua, &error_log_path).await;
            inject_method_tables(&lua, &error_log_path).await;

            #[cfg(feature = "luau")]
            setup_timeout(&lua);

            if let Err(e) = lua.load(&init_source).set_name("server/init.lua").exec() {
                return handle_error(&name, e, error_log_path).await;
            }

            let self_table = build_context_table(
                &lua, &body, &headers, &query, &path, &path_args, &client_ip, &method,
            );
            let func = match resolve_handler(&lua, &method, &name) {
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

fn extract_body(req: &rouille::Request) -> Option<String> {
    let mut reader = req.data()?;
    let mut s = String::new();
    let _ = reader.read_to_string(&mut s);
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

fn extract_headers(req: &rouille::Request) -> std::collections::HashMap<String, String> {
    req.headers()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn extract_query(req: &rouille::Request) -> std::collections::HashMap<String, String> {
    let mut query = std::collections::HashMap::new();
    for pair in req.raw_query_string().split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let k = parts.next().unwrap_or("");
        let v = parts.next().unwrap_or("");
        query.insert(url_decode(k), url_decode(v));
    }
    query
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

fn build_context_table(
    lua: &mlua::Lua,
    body: &Option<String>,
    headers: &std::collections::HashMap<String, String>,
    query: &std::collections::HashMap<String, String>,
    path: &str,
    path_args: &[String],
    client_ip: &str,
    method: &str,
) -> mlua::Table {
    let table = lua.create_table().unwrap();
    let _ = table.set("body", body.clone());
    let _ = table.set("headers", headers.clone());
    let _ = table.set("query", query.clone());
    let _ = table.set("path", path);
    let _ = table.set("path_args", path_args.to_vec());
    let _ = table.set("client_ip", client_ip);
    let _ = table.set("method", method);
    table
}

fn resolve_handler(
    lua: &mlua::Lua,
    method: &str,
    name: &str,
) -> Result<mlua::Function, rouille::Response> {
    let method_lower = method.to_lowercase();
    let globals = lua.globals();

    if let Ok(m_table) = globals.get::<mlua::Table>(method_lower.as_str()) {
        if let Ok(f) = m_table.get::<mlua::Function>(name) {
            return Ok(f);
        }
    }

    if let Ok(all_table) = globals.get::<mlua::Table>("all") {
        if let Ok(f) = all_table.get::<mlua::Function>(name) {
            return Ok(f);
        }
    }

    Err(rouille::Response::text("Not Found").with_status_code(404))
}

async fn format_response(
    lua: &mlua::Lua,
    result: Result<mlua::Value, mlua::Error>,
    name: &str,
    error_log_path: &std::path::Path,
) -> rouille::Response {
    match result {
        Ok(mlua::Value::Table(t)) => format_table_response(lua, &t),
        Ok(mlua::Value::String(s)) => {
            rouille::Response::text(s.to_string_lossy()).with_status_code(200)
        }
        Ok(mlua::Value::Nil) => rouille::Response::text("").with_status_code(200),
        Ok(_) => {
            rouille::Response::text("Invalid response type from Lua handler").with_status_code(500)
        }
        Err(e) => handle_error(name, e, error_log_path.to_path_buf()).await,
    }
}

fn format_table_response(lua: &mlua::Lua, t: &mlua::Table) -> rouille::Response {
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

    let mut res = rouille::Response {
        status_code: status,
        headers: vec![],
        data: rouille::ResponseBody::from_data(body_bytes),
        upgrade: None,
    };

    if let Some(ct) = content_type {
        res.headers.push(("Content-Type".into(), ct.into()));
    }

    if let Ok(h_tbl) = t.get::<mlua::Table>("headers") {
        for pair in h_tbl.pairs::<String, String>() {
            if let Ok((k, v)) = pair {
                if k.is_empty()
                    || k.contains('\n')
                    || k.contains('\r')
                    || v.contains('\n')
                    || v.contains('\r')
                {
                    continue;
                }
                res.headers.push((k.into(), v.into()));
            }
        }
    }

    res
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
    let deferred = match ctx.get::<mlua::Table>("__deferred") {
        Ok(t) => t,
        Err(e) => {
            log_server_error(
                &format!("API server: failed to get __deferred table: {}", e),
                error_log_path,
            )
            .await;
            return;
        }
    };
    let len = deferred.len().unwrap_or(0);
    for i in (1..=len).rev() {
        if let Ok(f) = deferred.get::<mlua::Function>(i) {
            if let Err(e) = f.call_async::<mlua::Value>((ctx.clone(),)).await {
                log_server_error(
                    &format!("API server: deferred function failed: {}", e),
                    error_log_path,
                )
                .await;
            }
        }
    }
}

async fn handle_error(name: &str, e: mlua::Error, error_log_path: PathBuf) -> rouille::Response {
    crate::t_eprintln!("API Server error in endpoint '{}': {}", name, e);
    let msg = format!("Error in /api/v/{}: {}\n", name, e);
    let task = crate::services::io::IoTask {
        path: error_log_path,
        content: msg.into_bytes(),
        op: crate::services::io::IoOp::Append,
        add_timestamp: true,
        reply: None,
    };
    let _ = crate::services::io::send_task(task).await;
    rouille::Response::json(&serde_json::json!({ "error": "Internal Server Error" }))
        .with_status_code(500)
}

fn url_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '+' {
            bytes.push(b' ');
        } else if c == '%' {
            let mut hex = String::new();
            if let Some(h1) = chars.next() {
                hex.push(h1);
            }
            if let Some(h2) = chars.next() {
                hex.push(h2);
            }
            if hex.len() == 2 {
                if let Ok(b) = u8::from_str_radix(&hex, 16) {
                    bytes.push(b);
                    continue;
                }
            }
            bytes.extend_from_slice(b"%");
            bytes.extend_from_slice(hex.as_bytes());
        } else {
            bytes.extend_from_slice(c.to_string().as_bytes());
        }
    }
    String::from_utf8(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests;
