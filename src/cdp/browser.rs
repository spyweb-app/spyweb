use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use fs4::FileExt;
use futures_lite::StreamExt;

use crate::cdp::transport::CdpTransport;
use crate::cdp::types::LaunchOptions;

pub struct Browser {
    pub(crate) transport: Arc<CdpTransport>,

    process: Option<async_process::Child>,
    user_data_dir: Option<PathBuf>,
    pub(crate) _lock_file: Option<std::fs::File>,
    pub(crate) keep_alive: bool,
    pub(crate) browser_ws_url: Option<String>,
}

impl Browser {
    pub fn get_user_data_dir(&self) -> Option<PathBuf> {
        self.user_data_dir.clone()
    }

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
            browser_ws_url: Some(ws_url.to_string()),
        })
    }

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

        // Step 5: Connect to browser-level WebSocket
        let transport = CdpTransport::connect(&ws_url).await?;

        // Step 6: Return Browser (browser-level transport — use b:attach() for page access)
        Ok(Self {
            transport: Arc::new(transport),
            process: Some(child),
            user_data_dir: options.user_data_dir,
            _lock_file: lock_file,
            keep_alive: options.keep_alive,
            browser_ws_url: Some(ws_url),
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
    }
}

pub fn find_browser_executable() -> String {
    if let Ok(val) = std::env::var("BROWSER") {
        let v = val.trim().to_string();
        if !v.is_empty() && is_cdp_compatible(&v) {
            if std::path::Path::new(&v).is_file() {
                return v;
            }
            if let Some(path) = find_in_path(&v) {
                return path;
            }
        }
    }

    let default = get_os_default_browser();

    if let Some(ref d) = default
        && is_cdp_compatible(d)
    {
        // on non-windows, verify it exists in PATH
        #[cfg(not(target_os = "windows"))]
        if let Some(path) = find_in_path(d) {
            return path;
        }
    }

    // 2. Windows absolute paths (uses `default` from above)
    #[cfg(target_os = "windows")]
    {
        // Helper: score a path so the detected default browser is checked first
        let sort_key = |path: &str| -> u8 {
            let p = path.to_lowercase();
            match default.as_deref() {
                Some(d) if d.contains("edge") && p.contains("edge") => 0,
                Some(d) if d.contains("brave") && p.contains("brave") => 0,
                Some(_) if p.contains("chrome") => 0,
                _ => 1,
            }
        };

        // check user-level install first (most common)
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let mut candidates = vec![
                format!(r"{}\Google\Chrome\Application\chrome.exe", local),
                format!(r"{}\Microsoft\Edge\Application\msedge.exe", local),
                format!(
                    r"{}\BraveSoftware\Brave-Browser\Application\brave.exe",
                    local
                ),
            ];
            candidates.sort_by_key(|p| sort_key(p));
            for path in &candidates {
                if std::path::Path::new(path).exists() {
                    return path.clone();
                }
            }
        }

        // system-level install
        let mut system_paths = vec![
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
        ];
        system_paths.sort_by_key(|p| sort_key(p));
        for path in system_paths {
            if std::path::Path::new(path).exists() {
                return path.to_string();
            }
        }

        // genuine last resort
        return "chrome.exe".to_string();
    }

    // 3. Linux/macOS fallback list via PATH
    #[cfg(not(target_os = "windows"))]
    {
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
    }

    // 4. macOS absolute paths
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

    #[cfg(not(target_os = "windows"))]
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
        // open -Ra "https:" returns the path of the default app bundle for the https scheme
        let output = Command::new("open").args(["-Ra", "https:"]).output().ok()?;
        if output.status.success() {
            let app_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !app_path.is_empty() {
                let stripped = app_path.strip_suffix(".app").unwrap_or(&app_path);
                let bin_name = stripped.rsplit('/').next().unwrap_or(stripped);
                let lower = bin_name.to_lowercase();
                if lower.contains("google chrome") || lower.contains("chrome") {
                    return Some("google-chrome".into());
                }
                if lower.contains("chromium") {
                    return Some("chromium".into());
                }
                if lower.contains("brave") {
                    return Some("brave-browser".into());
                }
                if lower.contains("edge") {
                    return Some("microsoft-edge".into());
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;

        // Try multiple registry paths to find the default browser ProgId
        let progid = [
            // Most common path on Windows 10/11
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice",
            // Fallback: http scheme
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\http\\UserChoice",
            // Fallback: .htm file association
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.htm\\UserChoice",
        ]
        .iter()
        .find_map(|path| {
            let output = Command::new("reg")
                .args(["query", path, "/v", "ProgId"])
                .output()
                .ok()?;
            if output.status.success() {
                let line = String::from_utf8_lossy(&output.stdout);
                // Extract the value after REG_SZ/REG_EXPAND_SZ
                line.lines().find_map(|l| {
                    let l = l.trim();
                    if l.starts_with("ProgId") || l.starts_with("Progid") {
                        l.split_whitespace().last().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        });

        if let Some(ref p) = progid {
            let lower = p.to_lowercase();
            if lower.contains("chromehtml") || lower.contains("chrome") {
                return Some("chrome".into());
            }
            if lower.contains("chromium") {
                return Some("chromium".into());
            }
            if lower.contains("brave") {
                return Some("brave".into());
            }
            if lower.contains("msedgehtm") || lower.contains("edge") {
                return Some("msedge".into());
            }
        }

        // Last resort: read the command line for https handler directly
        let cmd_output = Command::new("reg")
            .args([
                "query",
                "HKEY_CLASSES_ROOT\\https\\shell\\open\\command",
                "/ve",
            ])
            .output()
            .ok()?;
        if cmd_output.status.success() {
            let cmd_line = String::from_utf8_lossy(&cmd_output.stdout);
            let lower = cmd_line.to_lowercase();
            if lower.contains("chrome") {
                return Some("chrome".into());
            }
            if lower.contains("chromium") {
                return Some("chromium".into());
            }
            if lower.contains("brave") {
                return Some("brave".into());
            }
            if lower.contains("msedge") || lower.contains("edge") {
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
