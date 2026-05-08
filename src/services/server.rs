use crate::services::db::{Db, Record};
use anyhow::Result;
use rouille::{Request, Response};
use std::sync::Arc;

pub struct WebServer {
    db: Arc<Db>,
    active_jobs: Arc<std::sync::RwLock<Vec<String>>>,
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
    pub fn new(db: Arc<Db>, active_jobs: Arc<std::sync::RwLock<Vec<String>>>) -> Self {
        Self { db, active_jobs }
    }

    pub fn listen(&self, addr: &str) -> Result<()> {
        let server_db = Arc::clone(&self.db);
        let server_active_jobs = Arc::clone(&self.active_jobs);

        let server = rouille::Server::new(addr, move |request| {
            let url = request.url();

            // HTML pages
            if url == "/" || url == "/records" {
                let ui_path = std::path::Path::new("ui/index.html");
                if ui_path.exists() {
                    return match std::fs::read_to_string(ui_path) {
                        Ok(content) => Response::html(content),
                        Err(e) => {
                            Response::html(format!("<pre>Error reading ui/index.html: {}</pre>", e))
                        }
                    };
                }

                return match handle_html_records(&server_db) {
                    Ok(html) => Response::html(html),
                    Err(e) => Response::html(format!("<pre>Error: {}</pre>", e)),
                };
            }

            // Static assets (favicon, images, custom scripts etc)
            let asset_response = rouille::match_assets(request, "ui");
            if asset_response.is_success() {
                return asset_response;
            }

            // API endpoints
            if !url.starts_with("/api") {
                return Response::empty_404();
            }

            match handle_request_logic(&server_db, &server_active_jobs, request) {
                Ok(response) => response,
                Err(e) => Response::json(&serde_json::json!({
                    "error": e.to_string()
                }))
                .with_status_code(500),
            }
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

fn handle_request_logic(
    db: &Db,
    active_jobs: &Arc<std::sync::RwLock<Vec<String>>>,
    request: &Request,
) -> Result<Response> {
    let url = request.url();

    if url == "/api/records" {
        return handle_records_request(request, db);
        // let job_id = request.get_param("job_id");
        // let all_records = db.get_all_records()?;

        // if let Some(id) = job_id {
        //     let records = all_records.get(&id).cloned().unwrap_or_default();
        //     return Ok(Response::json(&records));
        // } else {
        //     return Ok(Response::json(&all_records));
        // }
    }

    if url == "/api/jobs" {
        let job_ids = active_jobs
            .read()
            .map_err(|err| anyhow::anyhow!("active jobs lock poisoned: {}", err))?
            .clone();
        return Ok(Response::json(&job_ids));
    }

    Ok(Response::empty_404())
}

fn handle_records_request(request: &Request, db: &Db) -> Result<Response> {
    let query = RecordsQuery::from_request(request);

    if let Some(job_id) = query.job_id {
        let records = db.get_records_for_job_paginated(&job_id, query.limit, query.after)?;

        // #[derive(serde::Serialize)]
        // struct PaginatedResponse {
        //     records: Vec<Record>,
        //     limit: usize,
        //     has_more: bool,
        //     next_after: Option<u64>,
        // }

        // let has_more = records.len() == query.limit;
        // let next_after = records.last().map(|r| r.timestamp);

        // Ok(Response::json(&PaginatedResponse {
        //     records,
        //     limit: query.limit,
        //     has_more,
        //     next_after: if has_more { next_after } else { None },
        // }))
        // Transform on the fly
        let next_after = records.last().map(|r| r.timestamp);
        // let records_with_datetime: Vec<_> = records
        //     .into_iter()
        //     .map(|record| {
        //         // Serialize the original record to a value
        //         let mut value = serde_json::to_value(record)?;
        //         // Inject the date_time field
        //         value["date_time"] = serde_json::to_value(crate::services::utils::nanos_to_zulu(
        //             Some(value["timestamp"].as_u64().unwrap()),
        //         ))?;
        //         Ok(value)
        //     })
        //     .collect::<Result<Vec<_>>>()?;
        let records_with_datetime = add_datetime_to_records(records);

        // Similar response structure but with serde_json::Value
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
