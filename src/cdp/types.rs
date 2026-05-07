use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<CdpErrorBody>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
}

#[derive(Debug, serde::Deserialize)]
pub struct CdpErrorBody {
    pub code: i64,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
}

#[derive(Debug)]
pub struct LaunchOptions {
    pub executable: String, // path to browser binary
    pub headless: bool,     // default: true
    pub keep_alive: bool,   // default: false (kill on drop)
    pub user_data_dir: Option<PathBuf>,
    pub args: Vec<String>, // extra browser args
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            executable: crate::cdp::browser::find_browser_executable(),
            headless: true,
            keep_alive: false,
            user_data_dir: None,
            args: vec![],
        }
    }
}
