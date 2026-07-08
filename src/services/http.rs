use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};
use ureq::{Agent, Proxy};

pub const MAX_RESPONSE_BODY: u64 = 10 * 1024 * 1024;
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

pub struct RawResponse {
    pub url: String,
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub proxy: Option<String>,
    pub time_ms: u64,
    pub size: u64,
}

pub enum MultipartField {
    Text(String),
    File {
        content: Vec<u8>,
        filename: Option<String>,
        mime_type: Option<String>,
    },
}

pub fn collect_headers(headers: &ureq::http::HeaderMap) -> HashMap<String, String> {
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

pub fn read_response_body(body: &mut ureq::Body, max_bytes: u64) -> Result<String, String> {
    let mut buf = Vec::new();
    body.as_reader()
        .take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read response body failed: {e}"))?;
    if buf.len() as u64 > max_bytes {
        return Err(format!("response body exceeds {} byte limit", max_bytes));
    }
    String::from_utf8(buf).map_err(|e| format!("response body is not valid UTF-8: {e}"))
}

pub fn build_agent(proxy_url: Option<&str>, timeout: Duration) -> Result<Agent, String> {
    let mut config = Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();

    if let Some(proxy_url) = proxy_url {
        let proxy = Proxy::new(proxy_url)
            .map_err(|e| format!("invalid proxy url '{}': {}", proxy_url, e))?;
        config = Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .proxy(Some(proxy))
            .build();
    }

    Ok(config.into())
}

pub fn do_http(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    headers: Option<&HashMap<String, String>>,
    proxy: Option<&str>,
    timeout_secs: Option<u64>,
    max_body_bytes: Option<u64>,
) -> Result<RawResponse, String> {
    let start = Instant::now();
    let effective_timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let effective_max = max_body_bytes.unwrap_or(MAX_RESPONSE_BODY);
    let agent = build_agent(proxy, effective_timeout)?;

    let method_upper = method.to_uppercase();
    let has_body = matches!(method_upper.as_str(), "POST" | "PUT" | "PATCH");

    if has_body {
        let body_data = body.ok_or_else(|| format!("http method {method} requires a body"))?;
        let mut req = match method_upper.as_str() {
            "POST" => agent.post(url),
            "PUT" => agent.put(url),
            "PATCH" => agent.patch(url),
            _ => return Err(format!("unsupported http method: {method}")),
        };

        let mut content_type_set = false;
        if let Some(h) = headers {
            for (k, v) in h {
                if k.eq_ignore_ascii_case("content-type") {
                    content_type_set = true;
                }
                req = req.header(k, v);
            }
        }
        if !content_type_set {
            req = req.header("Content-Type", "application/x-www-form-urlencoded");
        }

        let mut response = req
            .send(body_data)
            .map_err(|e| format!("http_{} failed: {e}", method.to_lowercase()))?;
        let status = response.status().as_u16();
        let headers = collect_headers(response.headers());
        let body = read_response_body(response.body_mut(), effective_max)?;
        let elapsed = start.elapsed().as_millis() as u64;
        let size = body.len() as u64;
        Ok(RawResponse {
            url: url.to_string(),
            status,
            headers,
            body,
            proxy: proxy.map(String::from),
            time_ms: elapsed,
            size,
        })
    } else {
        let mut req = match method_upper.as_str() {
            "GET" => agent.get(url),
            "HEAD" => agent.head(url),
            "DELETE" => agent.delete(url),
            _ => return Err(format!("unsupported http method: {method}")),
        };

        if let Some(h) = headers {
            for (k, v) in h {
                req = req.header(k, v);
            }
        }

        let mut response = req
            .call()
            .map_err(|e| format!("http_{} failed: {e}", method.to_lowercase()))?;
        let status = response.status().as_u16();
        let headers = collect_headers(response.headers());
        let body = read_response_body(response.body_mut(), effective_max)?;
        let elapsed = start.elapsed().as_millis() as u64;
        let size = body.len() as u64;
        Ok(RawResponse {
            url: url.to_string(),
            status,
            headers,
            body,
            proxy: proxy.map(String::from),
            time_ms: elapsed,
            size,
        })
    }
}
