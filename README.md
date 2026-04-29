<p align="center">
  <h1 align="center">SpyWeb</h1>
  <p align="center">Tiny web scraper with Lua scripting - ~5MB binary, no runtime required, under 5MB idle RAM</p>
</p>

<p align="center">
  <img alt="Language" src="https://img.shields.io/badge/Language-Rust-orange?style=flat-square"/>
  <img alt="Scripting" src="https://img.shields.io/badge/Scripting-Luau-blue?style=flat-square"/>
  <img alt="License" src="https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-green?style=flat-square"/>
</p>

<p align="center">
  📖 <a href="docs/index.html"><b>Master Guide</b></a> |
  ⚙️ <a href="docs/config.md">Config</a> |
  📜 <a href="https://spyweb.pages.dev/">Lua API</a> |
  🛠️ <a href="docs/api.md">REST API</a> |
  🚀 <a href="docs/vps-deployment.md">VPS Setup</a> |
  📂 <a href="examples/">Examples</a> |
  🏗️ <a href="CONTRIBUTING.md">Build from Source</a>
</p>

---

## What is spyweb?
**SpyWeb** is a zero-dependency web monitoring engine built for speed and precision. Track listings, job boards, classifieds, price drops, restocks, public records, and anything that lives on an HTML page with simple TOML configs. Inject custom Lua logic for advanced workflows, and receive real-time alerts via desktop or webhooks—all packaged as two self-contained binaries that sip under 5MB of RAM at idle.

## Quick Demo
```toml
[[jobs]]
name = "HN Front Page"
url = "https://news.ycombinator.com"
selector = ".athing"
fields = ["title:.titleline > a", "link:.titleline > a@href"]
keywords = ["rust", "linux", "open source"]
interval = 300
```

## Features

| Feature | Description |
| :--- | :--- |
| **Zero Dependencies** | ~5MB self-contained. Completely portable, no runtime required. |
| **Lua Scripting** | 7 hook stages plus persistent Lua storage for counters, cursors, and shared state. |
| **Hot Reload** | Save a config or Lua script and SpyWeb respawns the job instantly. |
| **Internal DB** | Built-in deduplication ensures you never see the same item twice. |
| **Dual Binary** | Choice of a headless CLI or a silent system tray app for background runs. |
| **Concurrency** | Async-first engine; slow proxies or large jobs never block others. |
| **Fault Tolerant** | Lua hook errors are caught and logged without stopping the job, ensuring 100% uptime. |
| **Hybrid Engine** | Automatically falls back to a spec-compliant DOM parser for broken or complex HTML. |
| **Pro Alerting** | Integrated desktop notifications and customizable webhooks for real-time monitoring. |

## Install & Run
Download the latest release ZIP from the [Releases page](https://github.com/spyweb-rs/spyweb/releases) and extract it.

### Release Structure
```text
spyweb-folder/
├── spyweb           # Terminal executable
├── spyweb-tray      # Background tray executable
├── data             # Internal database file (Created on first run)
├── ui/              # Dashboard UI files (Required for web dashboard)
├── jobs.toml        # Single-file config for simple jobs (Optional)
├── jobs/            # Folder for advanced per-job configs (Optional)
├── docs/            # Offline documentation (Safe to delete)
└── examples/        # Sample configurations and Lua hooks (Safe to delete)
```

SpyWeb ships as two separate binaries to provide the best experience for your environment:

### 1. Terminal Version (`spyweb`)
best for headless servers, VPS, and cloud environments. Runs in the terminal and outputs real-time logs for monitoring and debugging.
```bash
./spyweb start
```
Press `Ctrl+C` to quit.

### 2. Silent Tray Version (`spyweb-tray`)
Best for desktop use. Runs in the background without a terminal window and provides quick access via a system tray icon.
```bash
# Windows
spyweb-tray.exe
```
Right-click the tray icon to open the web UI or quit the app.

### Recommended Workflow
A typical workflow is to use the **Terminal Version** for your initial setup, debugging Lua hooks, and verifying selectors. Once you are happy with the results, switch to the **Tray Version** to let it run silently in the background without cluttering your taskbar or terminal.

Both binaries serve the admin dashboard at **http://127.0.0.1:7979** and will loop each enabled job at its configured interval.

> **Tip:** You can customize the port by setting the `SPYWEB_PORT` environment variable:
> ```bash
> # Linux / macOS
> SPYWEB_PORT=9000 ./spyweb start
> 
> # Windows (PowerShell)
> $env:SPYWEB_PORT=9000; .\spyweb.exe start
> ```

## CLI Tools
The terminal binary includes helpful developer tools:

```bash
# Validate your jobs.toml without running the scraper
./spyweb check

# Run a single job instantly (bypasses interval and runs the full async Lua pipeline)
# Saves '{job_location}/{job-id}-response.html' and '{job_location}/{job-id}-fields.json' for easy inspection!
./spyweb debug "My Job Name"

# Check version and active Lua engine
./spyweb version
```

## Lua API & Hooks
Place a `hooks.lua` next to your config to customize the pipeline. SpyWeb provides persistent storage to track state (like page numbers or failure counts) across restarts.

### Scoped vs Global Storage
*   **`store_get/set/delete(key)`**: Scoped to the individual job. Safe for standard logic because hooks for a single job are sequential.
*   **`global_store_incr(key, default, delta)`**: **Atomic**. Use this when you need to mutate shared state across *multiple* jobs simultaneously to avoid race conditions.
*   **`global_store_get/set/delete(key)`**: Shared across all jobs.

### Stage Example (Pagination)
```lua
function before_fetch(request)
    local page = tonumber(store_get("page") or "1")

    if page > 100 then
        log("[!] Page limit reached")
        return nil
    end

    request.url = request.url .. "?page=" .. page
    store_set("page", tostring(page + 1))

    return request
end
```
>
> 💡 **Pro-Tip:** Editing hooks and config on a VPS? Check out our [VPS Setup & Deployment Guide](docs/vps-deployment.md) for a high-performance terminal IDE experience (Syntax checking, Autocomplete, and more).
>
> **JavaScript & Rendering:** SpyWeb is designed for extreme efficiency and does not ship with a heavy headless browser. For JS-heavy client-rendered pages or protected sites, you can use the `before_fetch` hook to seamlessly delegate rendering to any external service or local proxy. This maintains SpyWeb's tiny footprint while providing the flexibility to handle complex rendering requirements.

---

## ⚖️ A Note on Scraping Morality
Please be a good internet citizen:
- **Do not hammer sites:** Set a reasonable, modest `interval` in your configurations. Scraping a page every 5 seconds is almost never necessary and costs the site owner money. Furthermore, aggressive abuse is the fastest way to get your connection throttled, flagged, or permanently IP banned.
- **Respect resources:** If you are monitoring a small independent site, be extra gentle with your request frequency.
- **Honor the web:** SpyWeb is a tool built for personal monitoring and automation; it is not a weapon for Denial of Service or aggressive data harvesting. Be modest when scraping.

---
Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
