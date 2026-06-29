use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

const MAX_REQUEST_BODY: u64 = 10 * 1024 * 1024; // 10MB

pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub query_params: HashMap<String, String>,
    pub client_ip: Option<String>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.query_params.get(name).map(|s| s.as_str())
    }

    pub fn body_bytes(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    pub fn fake(method: &str, url: &str) -> Self {
        Self::fake_with(method, url, vec![], vec![])
    }

    pub fn fake_with(
        method: &str,
        url: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Self {
        let query_params = parse_query(url);
        let clean_url = url.split('?').next().unwrap_or(url).to_string();
        Request {
            method: method.to_string(),
            url: clean_url,
            headers,
            body: Some(body),
            query_params,
            client_ip: Some("127.0.0.1:0".into()),
        }
    }
}

pub enum ResponseBody {
    Empty,
    Text(String),
    Html(String),
    Json(Vec<u8>),
    Bytes(Vec<u8>),
}

impl ResponseBody {
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            ResponseBody::Empty => vec![],
            ResponseBody::Text(s) => s.into_bytes(),
            ResponseBody::Html(s) => s.into_bytes(),
            ResponseBody::Json(b) => b,
            ResponseBody::Bytes(b) => b,
        }
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: ResponseBody,
}

impl Response {
    pub fn text(body: impl Into<String>) -> Self {
        Response {
            status: 200,
            headers: vec![],
            body: ResponseBody::Text(body.into()),
        }
    }

    pub fn html(body: impl Into<String>) -> Self {
        Response {
            status: 200,
            headers: vec![("Content-Type".into(), "text/html".into())],
            body: ResponseBody::Html(body.into()),
        }
    }

    pub fn json(value: &impl serde::Serialize) -> Self {
        let bytes = serde_json::to_vec(value).unwrap_or_default();
        Response {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: ResponseBody::Json(bytes),
        }
    }

    pub fn empty_404() -> Self {
        Response {
            status: 404,
            headers: vec![],
            body: ResponseBody::Empty,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn body_string(&self) -> String {
        match &self.body {
            ResponseBody::Empty => String::new(),
            ResponseBody::Text(s) => s.clone(),
            ResponseBody::Html(s) => s.clone(),
            ResponseBody::Json(b) | ResponseBody::Bytes(b) => {
                String::from_utf8_lossy(b).to_string()
            }
        }
    }
}

/// Mock HTTP server for integration tests.
/// Internally uses rouille, but callers are framework-agnostic.
pub struct MockServer {
    port: u16,
}

impl MockServer {
    pub fn start(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> Self {
        let handler: Arc<dyn Fn(&Request) -> Response + Send + Sync> = Arc::new(handler);

        let server = rouille::Server::new("127.0.0.1:0", move |rouille_req| {
            let req = from_rouille_request(rouille_req);
            let resp = handler(&req);
            to_rouille_response(resp)
        })
        .unwrap();

        let port = server.server_addr().port();
        std::thread::spawn(move || server.run());

        Self { port }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers: rouille ↔ agnostic
// ---------------------------------------------------------------------------

pub(crate) fn from_rouille_request(req: &rouille::Request) -> Request {
    let method = req.method().to_string();
    let url = req.url().to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let query_params = parse_query(&format!("?{}", req.raw_query_string()));
    let client_ip = Some(req.remote_addr().to_string());

    let body = req.data().map(|reader| {
        let mut buf = Vec::new();
        let _ = reader.take(MAX_REQUEST_BODY).read_to_end(&mut buf);
        buf
    });

    Request {
        method,
        url,
        headers,
        body,
        query_params,
        client_ip,
    }
}

pub(crate) fn to_rouille_response(resp: Response) -> rouille::Response {
    let (body_bytes, explicit_ct) = match resp.body {
        ResponseBody::Empty => (vec![], None),
        ResponseBody::Text(s) => (s.into_bytes(), Some("text/plain")),
        ResponseBody::Html(s) => (s.into_bytes(), Some("text/html")),
        ResponseBody::Json(b) => (b, Some("application/json")),
        ResponseBody::Bytes(b) => (b, None),
    };

    let mut r = rouille::Response {
        status_code: resp.status,
        headers: vec![],
        data: rouille::ResponseBody::from_data(body_bytes),
        upgrade: None,
    };

    if let Some(ct) = explicit_ct {
        if !resp
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("Content-Type"))
        {
            r.headers.push(("Content-Type".into(), ct.into()));
        }
    }

    for (k, v) in resp.headers {
        r.headers.push((k.into(), v.into()));
    }

    r
}

// ---------------------------------------------------------------------------
// Shared utilities
// ---------------------------------------------------------------------------

fn parse_query(url: &str) -> HashMap<String, String> {
    let mut query = HashMap::new();
    if let Some(pos) = url.find('?') {
        let qs = &url[pos + 1..];
        for pair in qs.split('&') {
            if pair.is_empty() {
                continue;
            }
            let mut parts = pair.splitn(2, '=');
            let k = parts.next().unwrap_or("");
            let v = parts.next().unwrap_or("");
            query.insert(url_decode(k), url_decode(v));
        }
    }
    query
}

pub(crate) fn url_decode(s: &str) -> String {
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
