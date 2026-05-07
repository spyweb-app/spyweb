use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use fs4::FileExt;
use futures_lite::StreamExt;

use crate::cdp::transport::CdpTransport;
use crate::cdp::types::LaunchOptions;

pub struct Browser {
    pub(crate) transport: Arc<CdpTransport>,

    process: Option<async_process::Child>, // Some if launch by a child process, None if connected via WSS
    user_data_dir: Option<PathBuf>, // Profile dir — locked via fs4 to prevent collision across any CDP browser
    pub(crate) _lock_file: Option<std::fs::File>, // Held to keep the fs4 exclusive lock alive until Drop
    pub(crate) keep_alive: bool,
}

impl Browser {
    /// Get the path to the user data directory (profile), if one was provided.
    pub fn get_user_data_dir(&self) -> Option<PathBuf> {
        self.user_data_dir.clone()
    }

    /// Connect to an already-running CDP browser at the given WebSocket URL.
    pub async fn connect(ws_url: &str) -> Result<Self> {
        if !ws_url.starts_with("ws://") && !ws_url.starts_with("wss://") {
            bail!("Invalid WebSocket URL: must start with ws:// or wss://");
        }

        let transport = CdpTransport::connect(ws_url).await?;

        Ok(Self {
            transport: Arc::new(transport),
            process: None,
            user_data_dir: None,
            _lock_file: None,
            keep_alive: true,
        })
    }

    /// Launch a new browser process and connect to it via CDP.
    pub async fn launch(options: LaunchOptions) -> Result<Self> {
        use async_process::{Command, Stdio};
        use futures_lite::AsyncBufReadExt;
        use futures_lite::io::BufReader;

        let mut lock_file = None;

        // Step 1: Validate/create user_data_dir and acquire exclusive lock
        if let Some(ref dir) = options.user_data_dir {
            std::fs::create_dir_all(dir)
                .context(format!("Failed to create user_data_dir: {}", dir.display()))?;

            let lock_path = dir.join(".spyweb.lock");
            let file = std::fs::File::create(&lock_path).context(format!(
                "Failed to create lock file: {}",
                lock_path.display()
            ))?;

            file.try_lock_exclusive().map_err(|_| {
                anyhow::anyhow!("User data directory already in use: {}", dir.display())
            })?;

            lock_file = Some(file);
        }

        // Step 2: Build args
        let mut args: Vec<String> = Vec::new();

        if options.headless {
            args.push("--headless".into());
        }

        args.push("--remote-debugging-port=0".into());
        args.push("--no-first-run".into());
        args.push("--no-default-browser-check".into());
        args.push("--disable-extensions".into());

        if let Some(ref dir) = options.user_data_dir {
            args.push(format!("--user-data-dir={}", dir.display()));
        }

        args.extend(options.args.iter().cloned());

        // Step 3: Spawn process, capture stderr
        let mut child = Command::new(&options.executable)
            .args(&args)
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .context(format!(
                "Failed to spawn browser process: {}",
                options.executable
            ))?;

        // Step 4: Read stderr for WebSocket URL with timeout
        let stderr = child
            .stderr
            .take()
            .context("Failed to capture browser stderr")?;

        let ws_url = {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

            let mut found_url = None;

            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }

                let line_future = lines.next();
                let result: Option<Option<std::io::Result<String>>> =
                    smol::future::or(async { Some(line_future.await) }, async {
                        smol::Timer::after(remaining).await;
                        None
                    })
                    .await;

                match result {
                    Some(Some(Ok(line))) => {
                        if let Some(url) =
                            line.strip_prefix("DevTools listening on ").or_else(|| {
                                // Some browsers format it differently
                                if line.contains("ws://") || line.contains("wss://") {
                                    line.find("ws://")
                                        .or_else(|| line.find("wss://"))
                                        .map(|i| &line[i..])
                                } else {
                                    None
                                }
                            })
                        {
                            found_url = Some(url.trim().to_string());
                            break;
                        }
                    }
                    Some(Some(Err(_))) => continue,
                    _ => break, // timeout or EOF
                }
            }

            match found_url {
                Some(url) => url,
                None => {
                    let _ = child.kill();
                    bail!("Browser failed to start within 5s timeout");
                }
            }
        };

        // Step 5: Connect transport
        let transport = CdpTransport::connect(&ws_url).await?;

        // Step 6: Return Browser
        Ok(Self {
            transport: Arc::new(transport),
            process: Some(child),
            user_data_dir: options.user_data_dir,
            _lock_file: lock_file,
            keep_alive: options.keep_alive,
        })
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if !self.keep_alive
            && let Some(mut child) = self.process.take()
        {
            let _ = child.kill();
        }
        // fs4 lock is released when _lock_file is dropped
    }
}

/// Helper to find a CDP-compatible browser executable on the system.
/// Prioritizes the OS default browser, then searches common paths.
pub fn find_browser_executable() -> String {
    // 1. Try to find the OS default browser
    if let Some(default) = get_os_default_browser()
        && is_cdp_compatible(&default)
        && let Some(path) = find_in_path(&default)
    {
        return path;
    }

    // 2. Fallback to a prioritized list of known CDP-compatible browsers
    let fallbacks = [
        "google-chrome-stable",
        "google-chrome",
        "chromium-browser",
        "chromium",
        "brave-browser",
        "microsoft-edge",
    ];

    for bin in fallbacks {
        if let Some(path) = find_in_path(bin) {
            return path;
        }
    }

    // 3. Absolute fallbacks for specific OSs if PATH search fails
    #[cfg(target_os = "macos")]
    {
        let mac_paths = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ];
        for path in mac_paths {
            if std::path::Path::new(path).exists() {
                return path.to_string();
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, if "google-chrome" isn't in PATH, it's rarely found by name alone
        // but we'll return a sensible default for the cmd to try
        return "chrome.exe".to_string();
    }

    "google-chrome".to_string()
}

fn get_os_default_browser() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let output = Command::new("xdg-settings")
            .arg("get")
            .arg("default-web-browser")
            .output()
            .ok()?;
        if output.status.success() {
            let res = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // Maps "google-chrome.desktop" -> "google-chrome"
            return Some(res.replace(".desktop", ""));
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        // This is a bit complex to parse, but we can check the default handler
        let output = Command::new("defaults")
            .args([
                "read",
                "com.apple.LaunchServices/com.apple.launchservices.secure",
                "LSHandlers",
            ])
            .output()
            .ok()?;
        if output.status.success() {
            let res = String::from_utf8_lossy(&output.stdout);
            if res.contains("com.google.chrome") {
                return Some("google-chrome".into());
            }
            if res.contains("org.chromium.chromium") {
                return Some("chromium".into());
            }
            if res.contains("com.brave.browser") {
                return Some("brave-browser".into());
            }
            if res.contains("com.microsoft.edgemac") {
                return Some("microsoft-edge".into());
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let output = Command::new("reg")
            .args(["query", "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice", "/v", "ProgId"])
            .output()
            .ok()?;
        if output.status.success() {
            let res = String::from_utf8_lossy(&output.stdout);
            if res.contains("ChromeHTML") {
                return Some("chrome".into());
            }
            if res.contains("Chromium") {
                return Some("chromium".into());
            }
            if res.contains("Brave") {
                return Some("brave".into());
            }
            if res.contains("MSEdgeHTM") {
                return Some("msedge".into());
            }
        }
    }

    None
}

pub(crate) fn is_cdp_compatible(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("chrome")
        || n.contains("chromium")
        || n.contains("brave")
        || n.contains("edge")
        || n.contains("msedge")
}

fn find_in_path(bin: &str) -> Option<String> {
    #[cfg(not(target_os = "windows"))]
    let check_cmd = "which";
    #[cfg(target_os = "windows")]
    let check_cmd = "where";

    let output = std::process::Command::new(check_cmd)
        .arg(bin)
        .output()
        .ok()?;
    if output.status.success() {
        return Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    None
}
