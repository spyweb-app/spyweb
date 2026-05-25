use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use ureq::http::HeaderMap;
use ureq::{Agent, Proxy};

use crate::config::types::{JobConfig, Rotate};

const DEFAULT_TIMEOUT_SECS: u64 = 30;

const DEFAULT_HEADERS: &[(&str, &str)] = &[
    (
        "User-Agent",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
    ),
    (
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
    ),
    ("Accept-Language", "en-US,en;q=0.9"),
    ("Accept-Encoding", "gzip, deflate, br"),
    (
        "sec-ch-ua",
        r#""Chromium";v="148", "Google Chrome";v="148", "Not-A.Brand";v="24""#,
    ),
    ("sec-ch-ua-mobile", "?0"),
    ("sec-ch-ua-platform", "\"Windows\""),
    ("Upgrade-Insecure-Requests", "1"),
    ("Sec-Fetch-Dest", "document"),
    ("Sec-Fetch-Mode", "navigate"),
    ("Sec-Fetch-Site", "none"),
    ("Sec-Fetch-User", "?1"),
    ("Priority", "u=0, i"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestConfig {
    pub url: String,
    pub headers: IndexMap<String, String>,
}

impl RequestConfig {
    pub fn from_job(job: &JobConfig) -> Self {
        let mut headers = IndexMap::new();
        for (name, value) in DEFAULT_HEADERS {
            headers.insert(name.to_string(), value.to_string());
        }
        if let Some(job_headers) = &job.headers {
            for (name, value) in job_headers {
                headers.shift_remove(name.as_str());
                headers.insert(name.clone(), value.clone());
            }
        }
        Self {
            url: job.url.clone(),
            headers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestResult {
    pub url: String,
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchAttempt {
    pub request: RequestConfig,
    pub proxy: Option<String>,
    pub result: Result<RequestResult, String>,
}

fn format_error_chain(err: &anyhow::Error) -> String {
    let mut chain = err.chain();
    let mut message = chain
        .next()
        .map(std::string::ToString::to_string)
        .unwrap_or_default();

    for cause in chain {
        if !message.is_empty() {
            message.push_str(": ");
        }
        message.push_str(&cause.to_string());
    }

    message
}

#[derive(Debug)]
pub struct RequestHandler {
    next_proxy_index: AtomicUsize,
    timeout: Duration,
    default_agent: OnceLock<Agent>,
}

impl Default for RequestHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestHandler {
    pub fn new() -> Self {
        Self {
            next_proxy_index: AtomicUsize::new(0),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            default_agent: OnceLock::new(),
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            next_proxy_index: AtomicUsize::new(0),
            timeout,
            default_agent: OnceLock::new(),
        }
    }

    fn get_agent(&self, proxy_url: Option<&str>) -> Result<Agent> {
        match proxy_url {
            Some(url) => self.build_agent(Some(url)),
            None => Ok(self
                .default_agent
                .get_or_init(|| self.build_agent(None).expect("default agent init"))
                .clone()),
        }
    }

    pub fn fetch(&self, job: &JobConfig) -> Result<RequestResult> {
        let selected_proxy = self.select_proxy(job);
        let agent = self.get_agent(selected_proxy.as_deref())?;

        let mut request = agent.get(&job.url);

        for (name, value) in DEFAULT_HEADERS {
            request = request.header(*name, *value);
        }

        if let Some(headers) = &job.headers {
            for (name, value) in headers {
                request = request.header(name, value);
            }
        }

        let mut response = request
            .call()
            .with_context(|| format!("request failed for job '{}'", job.name))?;

        let status = response.status().as_u16();
        let headers = flatten_headers(response.headers());
        let body = response
            .body_mut()
            .read_to_string()
            .context("failed to read response body")?;

        Ok(RequestResult {
            url: job.url.clone(),
            status,
            headers,
            body,
            proxy: selected_proxy,
        })
    }

    /// Fetch using a RequestConfig (possibly mutated by Lua hooks) for URL and headers.
    /// Proxy selection still comes from job.proxy.
    pub fn fetch_with_request(&self, job: &JobConfig, req: &RequestConfig) -> FetchAttempt {
        let selected_proxy = self.select_proxy(job);
        let agent = match self.get_agent(selected_proxy.as_deref()) {
            Ok(agent) => agent,
            Err(err) => {
                return FetchAttempt {
                    request: req.clone(),
                    proxy: selected_proxy,
                    result: Err(format_error_chain(&err)),
                };
            }
        };

        let mut request = agent.get(&req.url);

        for (name, value) in &req.headers {
            request = request.header(name, value);
        }

        let result = request
            .call()
            .with_context(|| format!("request failed for job '{}'", job.name))
            .and_then(|mut response| {
                let status = response.status().as_u16();
                let headers = flatten_headers(response.headers());
                let body = response
                    .body_mut()
                    .read_to_string()
                    .context("failed to read response body")?;

                Ok(RequestResult {
                    url: req.url.clone(),
                    status,
                    headers,
                    body,
                    proxy: selected_proxy.clone(),
                })
            });

        FetchAttempt {
            request: req.clone(),
            proxy: selected_proxy,
            result: result.map_err(|err| format_error_chain(&err)),
        }
    }

    fn build_agent(&self, proxy_url: Option<&str>) -> Result<Agent> {
        let mut config = Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .http_status_as_error(false)
            .build();

        if let Some(proxy_url) = proxy_url {
            let proxy = Proxy::new(proxy_url)
                .with_context(|| format!("invalid proxy url '{}'", proxy_url))?;
            config = Agent::config_builder()
                .timeout_global(Some(self.timeout))
                .http_status_as_error(false)
                .proxy(Some(proxy))
                .build();
        }

        Ok(config.into())
    }

    fn select_proxy(&self, job: &JobConfig) -> Option<String> {
        let proxy = job.proxy.as_ref()?;
        if !proxy.enabled || proxy.urls.is_empty() {
            return None;
        }

        match proxy.rotate {
            Rotate::Sticky => proxy.urls.first().cloned(),
            Rotate::Random => {
                let index = random_index(proxy.urls.len());
                proxy.urls.get(index).cloned()
            }
            Rotate::RoundRobin => {
                let index = self.next_proxy_index.fetch_add(1, Ordering::Relaxed);
                proxy.urls.get(index % proxy.urls.len()).cloned()
            }
        }
    }
}

fn flatten_headers(headers: &HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value) in headers.iter() {
        let v = value.to_str().unwrap_or_default();
        map.entry(name.as_str().to_owned())
            .and_modify(|existing: &mut String| {
                existing.push_str(", ");
                existing.push_str(v);
            })
            .or_insert_with(|| v.to_owned());
    }
    map
}

fn random_index(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    fastrand::usize(..len)
}

pub fn fetch_job(job: &JobConfig) -> Result<RequestResult> {
    if job.url.trim().is_empty() {
        bail!("job '{}' is missing a url", job.name);
    }

    RequestHandler::new().fetch(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Field, Proxy as JobProxy};

    fn job_with_proxy(rotate: Rotate, urls: Vec<&str>) -> JobConfig {
        JobConfig {
            name: "test".into(),
            url: "https://example.com".into(),
            selector: ".item".into(),
            fields: vec![Field::Shorthand("title".into())],
            debug: false,
            keywords: None,
            search_fields: None,
            webhook: None,
            enabled: true,
            interval: 60,
            proxy: Some(JobProxy {
                enabled: true,
                rotate,
                urls: urls.into_iter().map(str::to_string).collect(),
            }),
            notification: None,
            headers: None,
            hash_fields: None,
        }
    }

    #[test]
    fn round_robin_proxy_rotation_advances() {
        let handler = RequestHandler::new();
        let job = job_with_proxy(
            Rotate::RoundRobin,
            vec!["http://proxy-1:8080", "http://proxy-2:8080"],
        );

        assert_eq!(
            handler.select_proxy(&job).as_deref(),
            Some("http://proxy-1:8080")
        );
        assert_eq!(
            handler.select_proxy(&job).as_deref(),
            Some("http://proxy-2:8080")
        );
        assert_eq!(
            handler.select_proxy(&job).as_deref(),
            Some("http://proxy-1:8080")
        );
    }

    #[test]
    fn sticky_proxy_always_uses_first_entry() {
        let handler = RequestHandler::new();
        let job = job_with_proxy(
            Rotate::Sticky,
            vec!["http://proxy-1:8080", "http://proxy-2:8080"],
        );

        assert_eq!(
            handler.select_proxy(&job).as_deref(),
            Some("http://proxy-1:8080")
        );
        assert_eq!(
            handler.select_proxy(&job).as_deref(),
            Some("http://proxy-1:8080")
        );
    }

    #[test]
    fn disabled_or_empty_proxy_configuration_is_ignored() {
        let handler = RequestHandler::new();
        let mut job = job_with_proxy(Rotate::RoundRobin, vec![]);
        assert_eq!(handler.select_proxy(&job), None);

        job.proxy = Some(JobProxy {
            enabled: false,
            rotate: Rotate::RoundRobin,
            urls: vec!["http://proxy-1:8080".into()],
        });
        assert_eq!(handler.select_proxy(&job), None);
    }

    #[test]
    fn flatten_headers_preserves_header_names_and_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            ureq::http::HeaderValue::from_static("text/html"),
        );
        headers.insert(
            "x-request-id",
            ureq::http::HeaderValue::from_static("abc123"),
        );

        let flattened = flatten_headers(&headers);

        assert_eq!(
            flattened.get("content-type").map(String::as_str),
            Some("text/html")
        );
        assert_eq!(
            flattened.get("x-request-id").map(String::as_str),
            Some("abc123")
        );
    }

    #[test]
    fn build_agent_disables_http_status_as_error() {
        let handler = RequestHandler::new();
        let Ok(agent) = handler.build_agent(None) else {
            panic!("building an agent without a proxy should succeed");
        };

        assert!(
            !agent.config().http_status_as_error(),
            "HTTP 4xx/5xx responses should be treated as normal responses"
        );
    }

    #[test]
    fn error_chain_formatter_preserves_context_and_cause() {
        let err = anyhow::anyhow!("dns lookup failed").context("request failed for job 'Jumia'");

        let formatted = format_error_chain(&err);

        assert_eq!(
            formatted,
            "request failed for job 'Jumia': dns lookup failed"
        );
    }
}
