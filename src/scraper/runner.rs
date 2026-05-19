use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

use crate::config::types::{JobConfig, Jobs};
use crate::scraper::extractor::{ExtractedItem, Extractor};
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestHandler, RequestResult};
use crate::services::db::Db;

#[derive(Debug, Clone)]
pub struct JobRunResult {
    pub url: String,
    pub status: u16,
    pub item_count: usize,
    pub selector_matches: usize,
    pub items: Vec<ExtractedItem>,
    pub proxy: Option<String>,
}

#[derive(Debug, Default)]
pub struct Runner {
    request_handler: RequestHandler,
    extractor: Extractor,
}

impl Runner {
    pub fn new() -> Self {
        Self {
            request_handler: RequestHandler::new(),
            extractor: Extractor::new(),
        }
    }

    pub fn run_jobs(&self, jobs: &Jobs) -> Vec<Result<JobRunResult>> {
        jobs.list
            .iter()
            .filter(|job| job.config.enabled)
            .map(|job| self.run_job(&job.config, job.dir.as_deref()))
            .collect()
    }

    pub fn run_job(&self, config: &JobConfig, dir: Option<&Path>) -> Result<JobRunResult> {
        let response = self.request_handler.fetch(config)?;
        self.process_response(config, dir, response)
    }

    pub fn process_response(
        &self,
        config: &JobConfig,
        dir: Option<&Path>,
        response: RequestResult,
    ) -> Result<JobRunResult> {
        let result = self.extractor.extract(config, dir, &response.body)?;
        let items = self.keyword_filter(config, result.items);
        let item_count = items.len();

        Ok(JobRunResult {
            // config: job.clone(),
            url: response.url,
            status: response.status,
            item_count,
            selector_matches: result.selector_matches,
            items,
            proxy: response.proxy,
        })
    }

    pub fn fetch(&self, job: &JobConfig, request: &RequestConfig) -> FetchAttempt {
        self.request_handler.fetch_with_request(job, request)
    }
    pub fn extract(
        &self,
        config: &JobConfig,
        dir: Option<&Path>,
        response: &RequestResult,
    ) -> Result<crate::scraper::extractor::ExtractionResult> {
        self.extractor.extract(config, dir, &response.body)
    }

    pub fn keyword_filter(&self, job: &JobConfig, items: Vec<ExtractedItem>) -> Vec<ExtractedItem> {
        let keywords = job.keywords.as_deref();
        let search_fields = job.search_fields.as_deref();

        if keywords.is_none_or(|kw| kw.is_empty()) {
            return items;
        }

        items
            .into_iter()
            .filter_map(|mut item| {
                let matches = crate::scraper::extractor::matching_keywords(
                    &item.fields,
                    keywords,
                    search_fields,
                );
                if matches.is_empty() {
                    None
                } else {
                    item.matches = matches;
                    Some(item)
                }
            })
            .collect()
    }
}

pub fn run_jobs(jobs: &Jobs) -> Vec<Result<JobRunResult>> {
    Runner::new().run_jobs(jobs)
}

pub async fn debug_job(job_name: &str) -> Result<()> {
    let db = Arc::new(Db::open("data")?);
    let mut jobs = crate::config::loader::load_all_jobs("jobs.toml", "jobs", db)?;

    let search_id = crate::config::types::normalize_job_id(job_name);

    let available_jobs = jobs
        .list
        .iter()
        .map(|j| format!("'{}'", j.config.name))
        .collect::<Vec<_>>()
        .join(", ");

    let job = jobs
        .list
        .iter_mut()
        .find(|j| {
            j.config.id() == search_id || j.config.name.to_lowercase() == job_name.to_lowercase()
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Job '{}' not found. Available jobs: {}",
                job_name,
                available_jobs
            )
        })?;

    job.config.debug = true;

    println!("Debug run for job: {}", crate::color::c_job(job_name));

    if let Some(h) = job.hooks.as_ref() {
        h.init_telemetry().await?;
    }
    let started = std::time::Instant::now();

    let result = async {
        let runner = Runner::new();
        let request = RequestConfig::from_job(&job.config);

        // 1. before_fetch
        let request = match job.hooks.as_ref() {
            Some(h) => match h.before_fetch(request).await? {
                None => {
                    println!(
                        "Request aborted by {} hook.",
                        crate::color::c_warn("before_fetch")
                    );
                    return Ok(());
                }
                Some(r) => r,
            },
            None => request,
        };

        // 2. fetch
        let fetch_attempt = match job.hooks.as_ref().filter(|h| h.has_override_fetch()) {
            Some(h) => {
                println!("{}", crate::color::c_info("Using override_fetch hook"));
                h.override_fetch(request.clone()).await?
            }
            None => {
                println!("Fetching URL: {}", crate::color::c_info(&request.url));
                let sample = match job.hooks.as_ref() {
                    Some(h) => Some(h.telemetry_stage_start().await?),
                    None => None,
                };
                let attempt = runner.fetch(&job.config, &request);
                if let (Some(h), Some(s)) = (job.hooks.as_ref(), sample) {
                    let (status, error) = match &attempt.result {
                        Ok(_) => ("success", None),
                        Err(err) => ("error", Some(err.clone())),
                    };
                    let _ = h.record_telemetry_stage("fetch", s, status, error).await;
                }
                attempt
            }
        };

        if let Ok(response) = &fetch_attempt.result {
            if response.status >= 200 && response.status < 300 {
                println!(
                    "{} (Status: {})",
                    crate::color::c_ok("FETCH OK"),
                    response.status
                );
            } else {
                println!(
                    "{} (Status: {})",
                    crate::color::c_err("FETCH FAILED"),
                    response.status
                );
            }
        }

        // 3. after_fetch
        let response = match job.hooks.as_ref() {
            Some(h) => match h.after_fetch(fetch_attempt).await? {
                None => {
                    println!(
                        "Response aborted by {} hook.",
                        crate::color::c_warn("after_fetch")
                    );
                    return Ok(());
                }
                Some(r) => r,
            },
            None => fetch_attempt.result.map_err(anyhow::Error::msg)?,
        };

        // 4. extract
        let extraction = match job.hooks.as_ref().filter(|h| h.has_override_extract()) {
            Some(h) => {
                println!("{}", crate::color::c_info("Using override_extract hook"));
                let items = h.override_extract(&response).await?;
                let count = items.len();
                crate::scraper::extractor::ExtractionResult {
                    items,
                    selector_matches: count,
                }
            }
            None => {
                let sample = match job.hooks.as_ref() {
                    Some(h) => Some(h.telemetry_stage_start().await?),
                    None => None,
                };
                let result = runner.extract(&job.config, job.dir.as_deref(), &response)?;
                if let (Some(h), Some(s)) = (job.hooks.as_ref(), sample) {
                    let _ = h.record_telemetry_stage("extract", s, "success", None).await;
                }
                result
            }
        };

        println!(
            "Extracted {} item(s) (out of {} selector matches)",
            crate::color::c_info(&extraction.items.len().to_string()),
            crate::color::c_info(&extraction.selector_matches.to_string())
        );

        if let Some(h) = job.hooks.as_ref() {
            h.set_selector_matches(extraction.selector_matches).await?;
        }

        let items = extraction.items;

        // 5. after_extract
        let items = match job.hooks.as_ref() {
            Some(h) => h.after_extract(items).await?,
            None => items,
        };

        // 6. filter_item / keyword_filter
        let filter_sample = match job.hooks.as_ref() {
            Some(h) => Some(h.telemetry_stage_start().await?),
            None => None,
        };
        let items = match job.hooks.as_ref() {
            Some(h) if h.has_filter_item() => {
                let mut filtered = Vec::new();
                for item in items {
                    if let Some(mut i) = h.filter_item(item).await? {
                        i.matches = crate::scraper::extractor::matching_keywords(
                            &i.fields,
                            job.config.keywords.as_deref(),
                            job.config.search_fields.as_deref(),
                        );
                        filtered.push(i);
                    }
                }
                filtered
            }
            _ => runner.keyword_filter(&job.config, items),
        };
        if let (Some(h), Some(s)) = (job.hooks.as_ref(), filter_sample) {
            let error = h.take_filter_error().await;
            let status = if error.is_some() { "error" } else { "success" };
            let _ = h.record_telemetry_stage("filter", s, status, error).await;
        }

        // 7. before_store
        let items = match job.hooks.as_ref() {
            Some(h) => match h.before_store(items).await? {
                None => {
                    println!(
                        "Items aborted by {} hook.",
                        crate::color::c_warn("before_store")
                    );
                    return Ok(());
                }
                Some(i) => i,
            },
            None => items,
        };

        println!(
            "\n{}",
            crate::color::c_bold(&format!("Final pipeline output ({} items):", items.len()))
        );
        for (i, item) in items.iter().enumerate() {
            println!("\n  Item {}:", crate::color::c_info(&(i + 1).to_string()));
            for (k, v) in &item.fields {
                println!("    {}: {}", crate::color::c_bold(k), v);
            }
            if !item.matches.is_empty() {
                println!("    matches: {:?}", item.matches);
            }
        }

        let html_path = job
            .dir
            .as_deref()
            .unwrap_or(std::path::Path::new("."))
            .join(format!("{}-response.html", job.config.id()));
        let fields_path = job
            .dir
            .as_deref()
            .unwrap_or(std::path::Path::new("."))
            .join(format!("{}-fields.json", job.config.id()));

        println!(
            "\nHTML source and extracted fields written in: {}, {}",
            crate::color::c_dim(&html_path.display().to_string()),
            crate::color::c_dim(&fields_path.display().to_string())
        );

        Ok(())
    }
    .await;

    if let Some(h) = job.hooks.as_ref() {
        let _ = h.finalize_telemetry(started.elapsed()).await;
        h.print_telemetry().await;

        match &result {
            Ok(()) => h.run_on_success().await,
            Err(e) => h.run_on_error(e).await,
        }
        h.run_on_finally().await;
        h.cleanup_cycle_state().await;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Field, Rotate};

    fn job(enabled: bool, keywords: Option<Vec<&str>>) -> JobConfig {
        JobConfig {
            name: "demo".into(),
            url: "https://example.com/jobs".into(),
            selector: ".job".into(),
            fields: vec![
                Field::Shorthand("title:h2".into()),
                Field::Full {
                    name: "link".into(),
                    selector: "a".into(),
                    att: "href".into(),
                },
            ],
            keywords: keywords
                .map(|keywords| keywords.into_iter().map(str::to_string).collect::<Vec<_>>()),
            search_fields: None,
            webhook: None,
            enabled,
            interval: 60,
            debug: false,
            proxy: Some(crate::config::types::Proxy {
                enabled: false,
                rotate: Rotate::RoundRobin,
                urls: vec![],
            }),
            notification: None,
            headers: None,
            hash_fields: None,
        }
    }

    fn response(body: &str) -> RequestResult {
        RequestResult {
            url: "https://example.com/jobs".into(),
            status: 200,
            headers: Default::default(),
            body: body.into(),
            proxy: None,
        }
    }

    #[test]
    fn process_response_builds_job_result() {
        let runner = Runner::new();
        let job = job(true, None);
        let html = r#"
            <div class="job"><h2>Rust Developer</h2><a href="/rust">Apply</a></div>
            <div class="job"><h2>Go Developer</h2><a href="/go">Apply</a></div>
        "#;

        let Ok(result) = runner.process_response(&job, None, response(html)) else {
            panic!("processing a valid HTML response should succeed");
        };

        assert_eq!(result.status, 200);
        assert_eq!(result.item_count, 2);
    }

    #[test]
    fn process_response_uses_keyword_filtering() {
        let runner = Runner::new();
        let job = job(true, Some(vec!["rust"]));
        let html = r#"
            <div class="job"><h2>Rust Developer</h2><a href="/rust">Apply</a></div>
            <div class="job"><h2>Go Developer</h2><a href="/go">Apply</a></div>
        "#;

        let Ok(result) = runner.process_response(&job, None, response(html)) else {
            panic!("processing a valid HTML response should succeed");
        };

        assert_eq!(result.item_count, 1);
        assert_eq!(
            result.items[0].fields.get("title").map(String::as_str),
            Some("Rust Developer")
        );
    }
}
