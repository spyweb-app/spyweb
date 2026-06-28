use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
    ("Accept-Encoding", "gzip, deflate"),
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

const DEFAULT_MAX_BODY_SIZE: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestConfig {
    pub url: String,
    pub method: String,
    pub headers: Arc<IndexMap<String, String>>,
    pub timeout: Option<u64>,
    pub proxy: Option<String>,
    pub max_body_size: Option<u64>,
}

impl RequestConfig {
    pub fn from_job(job: &JobConfig) -> Self {
        let mut headers = IndexMap::new();
        for (name, value) in DEFAULT_HEADERS {
            headers.insert(name.to_string(), value.to_string());
        }

        headers.insert("Connection".to_string(), "close".to_string());

        if let Some(job_headers) = &job.headers {
            for (name, value) in job_headers {
                headers.shift_remove(name.as_str());
                headers.insert(name.clone(), value.clone());
            }
        }
        Self {
            url: job.url.clone(),
            method: "GET".into(),
            headers: Arc::new(headers),
            timeout: None,
            proxy: None,
            max_body_size: None,
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
    pub time_ms: Option<u64>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchAttempt {
    pub request: RequestConfig,
    pub proxy: Option<String>,
    pub result: Result<RequestResult, String>,
}

pub(crate) fn format_error_chain(err: &anyhow::Error) -> String {
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
    default_agent: OnceLock<Result<Agent>>,
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

    fn get_agent(&self, proxy_url: Option<&str>, timeout: Duration) -> Result<Agent> {
        if proxy_url.is_none() && timeout == self.timeout {
            match self
                .default_agent
                .get_or_init(|| self.build_agent(None, self.timeout))
            {
                Ok(agent) => Ok(agent.clone()),
                Err(err) => Err(anyhow::anyhow!("{}", err)),
            }
        } else {
            self.build_agent(proxy_url, timeout)
        }
    }

    pub fn fetch(&self, job: &JobConfig) -> Result<RequestResult> {
        let selected_proxy = self.select_proxy(job);
        let agent = self.get_agent(selected_proxy.as_deref(), self.timeout)?;

        let start = Instant::now();
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
        let elapsed = start.elapsed().as_millis() as u64;
        let size = body.len() as u64;

        Ok(RequestResult {
            url: job.url.clone(),
            status,
            headers,
            body,
            proxy: selected_proxy,
            time_ms: Some(elapsed),
            size: Some(size),
        })
    }

    /// Fetch using a RequestConfig (possibly mutated by Lua hooks).
    /// Proxy, timeout, and body size can be overridden per-request.
    pub fn fetch_with_request(&self, job: &JobConfig, req: &RequestConfig) -> FetchAttempt {
        let selected_proxy = req.proxy.clone().or_else(|| self.select_proxy(job));
        let effective_timeout = req.timeout.map(Duration::from_secs).unwrap_or(self.timeout);
        let max_body = req.max_body_size.unwrap_or(DEFAULT_MAX_BODY_SIZE);

        let agent = match self.get_agent(selected_proxy.as_deref(), effective_timeout) {
            Ok(agent) => agent,
            Err(err) => {
                return FetchAttempt {
                    request: req.clone(),
                    proxy: selected_proxy,
                    result: Err(format_error_chain(&err)),
                };
            }
        };

        let start = Instant::now();

        let err_context = |job: &JobConfig, proxy: &Option<String>| {
            let via = proxy
                .as_deref()
                .map(|p| format!(" via proxy '{}'", p))
                .unwrap_or_default();
            format!("request failed for job '{}'{}", job.name, via)
        };

        let result = match req.method.to_uppercase().as_str() {
            "HEAD" => {
                let request = req
                    .headers
                    .iter()
                    .fold(agent.head(&req.url), |r, (n, v)| r.header(n, v));
                request
                    .call()
                    .with_context(|| err_context(job, &selected_proxy))
                    .map(|response| {
                        let elapsed = start.elapsed().as_millis() as u64;
                        RequestResult {
                            url: req.url.clone(),
                            status: response.status().as_u16(),
                            headers: flatten_headers(response.headers()),
                            body: String::new(),
                            proxy: selected_proxy.clone(),
                            time_ms: Some(elapsed),
                            size: Some(0),
                        }
                    })
            }
            "DELETE" => {
                let request = req
                    .headers
                    .iter()
                    .fold(agent.delete(&req.url), |r, (n, v)| r.header(n, v));
                request
                    .call()
                    .with_context(|| err_context(job, &selected_proxy))
                    .and_then(|mut response| {
                        let status = response.status().as_u16();
                        let headers = flatten_headers(response.headers());
                        let body = read_body_bounded(response.body_mut().as_reader(), max_body)?;
                        let elapsed = start.elapsed().as_millis() as u64;
                        let size = body.len() as u64;
                        Ok(RequestResult {
                            url: req.url.clone(),
                            status,
                            headers,
                            body,
                            proxy: selected_proxy.clone(),
                            time_ms: Some(elapsed),
                            size: Some(size),
                        })
                    })
            }
            _ => {
                let request = req
                    .headers
                    .iter()
                    .fold(agent.get(&req.url), |r, (n, v)| r.header(n, v));
                request
                    .call()
                    .with_context(|| err_context(job, &selected_proxy))
                    .and_then(|mut response| {
                        let status = response.status().as_u16();
                        let headers = flatten_headers(response.headers());
                        let body = read_body_bounded(response.body_mut().as_reader(), max_body)?;
                        let elapsed = start.elapsed().as_millis() as u64;
                        let size = body.len() as u64;
                        Ok(RequestResult {
                            url: req.url.clone(),
                            status,
                            headers,
                            body,
                            proxy: selected_proxy.clone(),
                            time_ms: Some(elapsed),
                            size: Some(size),
                        })
                    })
            }
        };

        FetchAttempt {
            request: req.clone(),
            proxy: selected_proxy,
            result: result.map_err(|err| format_error_chain(&err)),
        }
    }

    pub(crate) fn build_agent(&self, proxy_url: Option<&str>, timeout: Duration) -> Result<Agent> {
        let mut config = Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .max_idle_connections(0)
            .build();

        if let Some(proxy_url) = proxy_url {
            let proxy = Proxy::new(proxy_url)
                .with_context(|| format!("invalid proxy url '{}'", proxy_url))?;
            config = Agent::config_builder()
                .timeout_global(Some(timeout))
                .http_status_as_error(false)
                .max_idle_connections(0)
                .proxy(Some(proxy))
                .build();
        }

        Ok(config.into())
    }

    pub(crate) fn select_proxy(&self, job: &JobConfig) -> Option<String> {
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

pub(crate) fn read_body_bounded<R: Read>(reader: R, max: u64) -> Result<String> {
    let mut buf = Vec::new();
    reader
        .take(max + 1)
        .read_to_end(&mut buf)
        .context("failed to read response body")?;
    if buf.len() as u64 > max {
        anyhow::bail!("response body exceeds {} byte limit", max);
    }
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("response body is not valid UTF-8: {e}"))
}

pub(crate) fn flatten_headers(headers: &HeaderMap) -> HashMap<String, String> {
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
