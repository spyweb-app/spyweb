use crate::services::db::{Db, Record};
use anyhow::Result;
use rouille::{Request, Response};
use std::sync::Arc;

#[derive(serde::Serialize, Clone)]
pub struct JobSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
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
    fn from_request(request: &Request) -> Self {
        Self {
            job_id: request.get_param("job_id").map(|s| s.to_string()),
            limit: request
                .get_param("limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(30)
                .min(1000),
            after: request.get_param("after").and_then(|s| s.parse().ok()),
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

        let server = rouille::Server::new(addr, move |request| {
            let url = request.url();

            // 1. API Route Branch (Guarded)
            if url.starts_with("/api") {
                if let Some(ref required_key) = auth_key {
                    let provided_key = request.header("X-SpyWeb-Key").unwrap_or_default();
                    if provided_key != required_key {
                        return Response::json(&serde_json::json!({ "error": "Unauthorized" }))
                            .with_status_code(401);
                    }
                }

                return match handle_api_request(&server_db, &server_active_jobs, request) {
                    Ok(response) => response,
                    Err(e) => Response::json(&serde_json::json!({ "error": e.to_string() }))
                        .with_status_code(500),
                };
            }

            // 2. Static Assets Branch (Unguarded, Strictly Caged)
            handle_static_request(&server_db, request)
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

/// Strictly validates and serves files from the 'ui' directory.
fn handle_static_request(db: &Db, request: &Request) -> Response {
    let url = request.url();

    // Special case for root/records routes -> serve index.html
    if url == "/" || url == "/records" {
        let index_path = std::path::Path::new("ui/index.html");
        if index_path.exists() {
            return match std::fs::read_to_string(index_path) {
                Ok(content) => Response::html(content),
                Err(e) => Response::html(format!("<pre>Error loading UI: {}</pre>", e)),
            };
        }
        // Fallback to minimal built-in HTML if ui/index.html is missing
        return match handle_html_records(db) {
            Ok(html) => Response::html(html),
            Err(e) => Response::html(format!("<pre>Error: {}</pre>", e)),
        };
    }

    // Sanitize and validate path for other assets
    let path = url.trim_start_matches('/');
    if let Err(e) = validate_static_path(path) {
        crate::t_eprintln!("Blocked static request for '{}': {}", url, e);
        return Response::empty_404();
    }

    // All checks passed, attempt to serve from ui/ folder
    let asset_response = rouille::match_assets(request, "ui");
    if asset_response.is_success() {
        return asset_response;
    }

    Response::empty_404()
}

fn validate_static_path(path: &str) -> Result<()> {
    // 1. Traversal Prevention (Strictly no ..)
    if path.contains("..") {
        return Err(anyhow::anyhow!("Directory traversal attempt blocked."));
    }

    // 2. Extension Allowlist
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

    let path_obj = std::path::Path::new(path);
    let ext = path_obj
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if !allowed_extensions.contains(&ext.as_str()) {
        return Err(anyhow::anyhow!(
            "File type not allowed for static assets: .{}",
            ext
        ));
    }

    Ok(())
}

fn handle_api_request(
    db: &Db,
    active_jobs: &Arc<std::sync::RwLock<Vec<JobSummary>>>,
    request: &Request,
) -> Result<Response> {
    let url = request.url();

    if url == "/api/records" {
        return handle_records_request(request, db);
    }

    if url == "/api/jobs" {
        let summaries = active_jobs
            .read()
            .map_err(|err| anyhow::anyhow!("active jobs lock poisoned: {}", err))?
            .clone();
        return Ok(Response::json(&summaries));
    }

    Ok(Response::empty_404())
}

fn handle_records_request(request: &Request, db: &Db) -> Result<Response> {
    let query = RecordsQuery::from_request(request);

    if let Some(job_id) = query.job_id {
        let records = db.get_records_for_job_paginated(&job_id, query.limit, query.after)?;
        let next_after = records.last().map(|r| r.timestamp);
        let records_with_datetime = add_datetime_to_records(records);
        let has_more = records_with_datetime.len() == query.limit;

        Ok(Response::json(&serde_json::json!({
            "records": records_with_datetime,
            "limit": query.limit,
            "has_more": has_more,
            "next_after": if has_more { next_after } else { None }
        })))
    } else {
        let grouped = db.get_all_records(Some(query.limit))?;
        Ok(Response::json(&grouped))
    }
}

#[derive(serde::Serialize)]
struct ApiRecord {
    #[serde(flatten)]
    record: Record,
    date_time: String,
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
