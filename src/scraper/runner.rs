use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

use crate::config::types::{JobConfig, Jobs};
use crate::scraper::extractor::{ExtractedItem, Extractor};
use crate::scraper::request::{RequestConfig, RequestHandler, RequestResult};
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

    pub fn fetch(&self, job: &JobConfig, request: &RequestConfig) -> Result<RequestResult> {
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
            .filter(|item| {
                crate::scraper::extractor::has_match(&item.fields, keywords, search_fields)
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

    println!("Debug run for job: \x1b[38;2;162;155;254;1m{}\x1b[0m", job_name);

    let runner = Runner::new();
    let request = RequestConfig::from_job(&job.config);

    // 1. before_fetch
    let request = match job.hooks.as_ref() {
        Some(h) => match h.before_fetch(request).await? {
            None => {
                println!("Request aborted by before_fetch hook.");
                return Ok(());
            }
            Some(r) => r,
        },
        None => request,
    };

    // 2. fetch
    println!("Fetching URL: \x1b[38;2;116;185;255m{}\x1b[0m", request.url);
    let fetch_result = runner.fetch(&job.config, &request);
    if let Ok(response) = &fetch_result {
        if response.status >= 200 && response.status < 300 {
            println!("\x1b[38;2;85;239;196;1mFETCH OK\x1b[0m (Status: {})", response.status);
        } else {
            println!("\x1b[1;31mFETCH FAILED\x1b[0m (Status: {})", response.status);
        }
    }

    // 3. after_fetch
    let response = match job.hooks.as_ref() {
        Some(h) => match h.after_fetch(fetch_result).await? {
            None => {
                println!("Response aborted by after_fetch hook.");
                return Ok(());
            }
            Some(r) => r,
        },
        None => fetch_result?,
    };

    // 4. extract
    let items = runner.extract(&job.config, job.dir.as_deref(), &response)?;
    println!("Extracted {} raw item(s)", items.len());

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
                println!("Items aborted by before_store hook.");
                return Ok(());
            }
            Some(i) => i,
        },
        None => items,
    };

    println!("\n\x1b[1mFinal pipeline output ({} items):\x1b[0m", items.len());
    for (i, item) in items.iter().enumerate() {
        println!("\n  Item {}:", i + 1);
        for (k, v) in &item.fields {
            println!("    \x1b[1m{}:\x1b[0m {}", k, v);
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
        html_path.display(),
        fields_path.display()
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
