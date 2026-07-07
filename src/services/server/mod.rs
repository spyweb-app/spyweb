pub mod types;

use crate::services::db::{Db, Record};
use anyhow::Result;
use std::sync::Arc;

#[derive(serde::Serialize, Clone)]
pub struct JobSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(serde::Serialize)]
struct ApiRecord {
    #[serde(flatten)]
    record: Record,
    date_time: String,
}

pub struct WebServer {
    db: Arc<Db>,
    active_jobs: Arc<std::sync::RwLock<Vec<JobSummary>>>,
    auth_key: Option<String>,
}

#[derive(Debug)]
struct RecordsQuery {
    job_id: Option<String>,
    limit: usize,
    after: Option<u64>,
}

impl RecordsQuery {
    fn from_request(request: &types::Request) -> Self {
        Self {
            job_id: request.param("job_id").map(|s| s.to_string()),
            limit: request
                .param("limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(30)
                .min(1000),
            after: request.param("after").and_then(|s| s.parse().ok()),
        }
    }
}

impl WebServer {
    pub fn new(db: Arc<Db>, active_jobs: Arc<std::sync::RwLock<Vec<JobSummary>>>) -> Self {
        let auth_key = std::env::var("SPYWEB_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            db,
            active_jobs,
            auth_key,
        }
    }

    pub fn listen(&self, addr: &str) -> Result<()> {
        let server_db = Arc::clone(&self.db);
        let server_active_jobs = Arc::clone(&self.active_jobs);
        let auth_key = self.auth_key.clone();

        let server = rouille::Server::new(addr, move |rouille_req| {
            let req = types::from_rouille_request(rouille_req);

            if req.url.starts_with("/api") {
                if let Some(ref required_key) = auth_key
                    && !constant_time_eq(
                        req.header("X-SpyWeb-Key").unwrap_or_default().as_bytes(),
                        required_key.as_bytes(),
                    )
                {
                    return types::to_rouille_response(
                        types::Response::json(&serde_json::json!({"error": "Unauthorized"}))
                            .with_status(401),
                    );
                }

                return match handle_api_request(&server_db, &server_active_jobs, &req) {
                    Ok(resp) => types::to_rouille_response(resp),
                    Err(e) => types::to_rouille_response(
                        types::Response::json(&serde_json::json!({"error": e.to_string()}))
                            .with_status(500),
                    ),
                };
            }

            types::to_rouille_response(handle_static_request(&server_db, &req))
        })
        .map_err(|e| anyhow::anyhow!("Failed to start server on {}: {}", addr, e))?;

        crate::t_println!(
            "Server listening on {}",
            crate::color::c_info(&format!("http://{}", addr))
        );

        server.run();
        Ok(())
    }
}

/// Validates and serves files from the 'ui' directory.
fn handle_static_request(db: &Db, request: &types::Request) -> types::Response {
    let url = request.url.split('?').next().unwrap_or(&request.url);

    if url == "/" || url == "/records" {
        let index_path = std::path::Path::new("ui/index.html");
        if index_path.exists() {
            return match std::fs::read_to_string(index_path) {
                Ok(content) => types::Response::html(content),
                Err(e) => types::Response::html(format!("<pre>Error loading UI: {}</pre>", e)),
            };
        }
        return match handle_html_records(db) {
            Ok(html) => types::Response::html(html),
            Err(e) => types::Response::html(format!("<pre>Error: {}</pre>", e)),
        };
    }

    let path = url.trim_start_matches('/');
    let path = types::url_decode(path);
    if let Err(e) = validate_static_path(&path) {
        crate::t_eprintln!("Blocked static request for '{}': {}", url, e);
        return types::Response::empty_404();
    }

    let file_path = std::path::Path::new("ui").join(&path);
    match std::fs::read(&file_path) {
        Ok(bytes) => {
            let ext = file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let mime = mime_for_extension(&ext);
            types::Response {
                status: 200,
                headers: vec![("Content-Type".into(), mime.into())],
                body: types::ResponseBody::Bytes(bytes),
            }
        }
        Err(_) => {
            if path.contains('.') {
                types::Response::empty_404()
            } else {
                match std::fs::read_to_string(std::path::Path::new("ui/index.html")) {
                    Ok(content) => types::Response::html(content),
                    Err(_) => types::Response::empty_404(),
                }
            }
        }
    }
}

fn mime_for_extension(ext: &str) -> &'static str {
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

fn validate_static_path(path: &str) -> Result<()> {
    let path_obj = std::path::Path::new(path);
    for component in path_obj.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(anyhow::anyhow!("Directory traversal attempt blocked."));
        }
    }

    let allowed_extensions = [
        "html",
        "js",
        "css",
        "png",
        "jpg",
        "jpeg",
        "gif",
        "svg",
        "ico",
        "woff",
        "woff2",
        "ttf",
        "otf",
        "json",
        "webmanifest",
        "map",
    ];

    let ext = path_obj
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if !ext.is_empty() && !allowed_extensions.contains(&ext.as_str()) {
        return Err(anyhow::anyhow!(
            "File type not allowed for static assets: .{}",
            ext
        ));
    }

    Ok(())
}

fn handle_api_request(
    db: &Arc<Db>,
    active_jobs: &Arc<std::sync::RwLock<Vec<JobSummary>>>,
    request: &types::Request,
) -> Result<types::Response> {
    let url = &request.url;

    if let Some(path) = url.strip_prefix("/api/v/") {
        let mut segments: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        if segments.is_empty() {
            return Ok(types::Response::empty_404());
        }
        let name = segments.remove(0);
        let source = match std::fs::read_to_string("server/init.lua") {
            Ok(s) => s,
            Err(_) => {
                return Ok(
                    types::Response::text("Server configuration not available").with_status(500)
                );
            }
        };
        let server = crate::lua::server::ApiServer::new(
            db.clone(),
            source,
            std::path::PathBuf::from("server/error.log"),
        );
        return Ok(server.handle(request.method.as_str(), &name, segments, request));
    }

    if url == "/api/records" {
        return handle_records_request(request, db);
    }

    if url == "/api/jobs" {
        let summaries = active_jobs
            .read()
            .map_err(|err| anyhow::anyhow!("active jobs lock poisoned: {}", err))?
            .clone();
        return Ok(types::Response::json(&summaries));
    }

    Ok(types::Response::empty_404())
}

fn handle_records_request(request: &types::Request, db: &Db) -> Result<types::Response> {
    let query = RecordsQuery::from_request(request);

    if let Some(job_id) = query.job_id {
        let records = db.get_records_for_job_paginated(&job_id, query.limit, query.after)?;
        let next_after = records.last().map(|r| r.timestamp);
        let records_with_datetime = add_datetime_to_records(records);
        let has_more = records_with_datetime.len() == query.limit;

        Ok(types::Response::json(&serde_json::json!({
            "records": records_with_datetime,
            "limit": query.limit,
            "has_more": has_more,
            "next_after": if has_more { next_after } else { None }
        })))
    } else {
        let grouped = db.get_all_records(Some(query.limit))?;
        Ok(types::Response::json(&grouped))
    }
}

fn add_datetime_to_records(records: Vec<Record>) -> Vec<ApiRecord> {
    records
        .into_iter()
        .map(|record| {
            let date_time = record.datetime_zulu();
            ApiRecord { record, date_time }
        })
        .collect()
}

fn handle_html_records(db: &Db) -> Result<String> {
    let records = db.get_all_records(Some(50))?;

    let mut html = String::from(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>SpyWeb Records</title>
    <style>
        body { font-family: monospace; padding: 20px; background: #1a1a1a; color: #ccc; }
        h1 { color: #fff; }
        .job { margin-bottom: 30px; }
        .job-name { color: #4fc3f7; font-size: 1.2em; margin-bottom: 10px; }
        .record { background: #252525; padding: 10px; margin: 5px 0; border-left: 3px solid #4fc3f7; }
        .timestamp { color: #888; font-size: 0.9em; }
        .field { margin: 3px 0; }
        .field-name { color: #81c784; }
        .field-value { color: #fff; }
    </style>
</head>
<body>
    <h1>SpyWeb Records</h1>
"#,
    );

    if records.is_empty() {
        html.push_str("<p>No records found.</p>");
    } else {
        for (job_id, job_records) in records {
            html.push_str(&format!(
                "<div class='job'><div class='job-name'>{}</div>",
                job_id
            ));

            for record in job_records {
                let datetime = record.datetime_zulu();
                html.push_str(&format!(
                    "<div class='record'><div class='timestamp'>{}</div>",
                    datetime
                ));

                for (key, value) in &record.fields {
                    let escaped_value = value
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;");
                    html.push_str(&format!(
                        "<div class='field'><span class='field-name'>{}:</span> <span class='field-value'>{}</span></div>",
                        key, escaped_value
                    ));
                }

                if !record.match_key.is_empty() {
                    html.push_str(&format!(
                        "<div class='field'><span class='field-name'>keywords:</span> <span class='field-value'>{}</span></div>",
                        record.match_key.join(", ")
                    ));
                }

                html.push_str("</div>");
            }

            html.push_str("</div>");
        }
    }

    html.push_str("</body></html>");
    Ok(html)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max_len = std::cmp::max(a.len(), b.len());
    let mut result = 0u8;
    for i in 0..max_len {
        result |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    result == 0
}

#[cfg(test)]
mod tests;
