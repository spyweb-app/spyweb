use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

use crate::config::types::{JobConfig, Jobs};
use crate::scraper::extractor::{ExtractedItem, Extractor};
use crate::scraper::request::{FetchAttempt, RequestConfig, RequestHandler, RequestResult};
use crate::services::db::Db;

#[derive(Debug, Clone)]
pub struct JobRunResult {
    // pub config: JobConfig,
    pub url: String,
    pub status: u16,
    pub item_count: usize,
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
        // futures
        // futures::future::join_all(futures)
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
        let items = self.extractor.extract(config, dir, &response.body)?;
        let items = self.keyword_filter(config, items);
        let item_count = items.len();

        Ok(JobRunResult {
            // config: job.clone(),
            url: response.url,
            status: response.status,
            item_count,
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
    ) -> Result<Vec<ExtractedItem>> {
        self.extractor.extract(config, dir, &response.body)
    }

    pub fn keyword_filter(&self, job: &JobConfig, items: Vec<ExtractedItem>) -> Vec<ExtractedItem> {
        let keywords = job.keywords.as_deref();
        let search_fields = job.search_fields.as_deref();

        if keywords.is_none_or(|kw| kw.is_empty()) {
            return items;
        }

        // match keywords {
        //     None => return items,
        //     Some(kw) if kw.is_empty() => return items,
        //     _ => {}
        // }

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

    // Smart match: normalize the input to compare against job IDs
    let search_id = job_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();

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

    println!(
        "Debug run for job: {}",
        crate::color::c_job(job_name)
    );

    let runner = Runner::new();
    let request = RequestConfig::from_job(&job.config);

    // 1. before_fetch
    let request = match job.hooks.as_ref() {
        Some(h) => match h.before_fetch(request).await? {
            None => {
                println!("Request aborted by {} hook.", crate::color::c_warn("before_fetch"));
                return Ok(());
            }
            Some(r) => r,
        },
        None => request,
    };

    // 2. fetch
    println!("Fetching URL: {}", crate::color::c_info(&request.url));
    let fetch_attempt = runner.fetch(&job.config, &request);
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
                println!("Response aborted by {} hook.", crate::color::c_warn("after_fetch"));
                return Ok(());
            }
            Some(r) => r,
        },
        None => fetch_attempt.result.map_err(anyhow::Error::msg)?,
    };

    // 4. extract
    let items = runner.extract(&job.config, job.dir.as_deref(), &response)?;
    println!("Extracted {} raw item(s)", crate::color::c_info(&items.len().to_string()));

    // 5. after_extract
    let items = match job.hooks.as_ref() {
        Some(h) => h.after_extract(items).await?,
        None => items,
    };

    // 6. filter_item / keyword_filter
    let items = match job.hooks.as_ref() {
        Some(h) if h.has_filter_item() => {
            let mut filtered = Vec::new();
            for item in items {
                if let Some(i) = h.filter_item(item).await? {
                    filtered.push(i);
                }
            }
            filtered
        }
        _ => runner.keyword_filter(&job.config, items),
    };

    // 7. before_store
    let items = match job.hooks.as_ref() {
        Some(h) => match h.before_store(items).await? {
            None => {
                println!("Items aborted by {} hook.", crate::color::c_warn("before_store"));
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

        let result = runner.process_response(&job, None, response(html)).unwrap();

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

        let result = runner.process_response(&job, None, response(html)).unwrap();

        assert_eq!(result.item_count, 1);
        assert_eq!(
            result.items[0].fields.get("title").map(String::as_str),
            Some("Rust Developer")
        );
    }
}
