<p align="center">
  <h1 align="center">SpyWeb</h1>
  <p align="center">Tiny web scraping/monitoring engine with Lua scripting - ~7MB binary, no runtime required, under 5MB idle RAM</p>
</p>

<p align="center">
  <img alt="Language" src="https://img.shields.io/badge/Language-Rust-orange?style=flat-square"/>
  <img alt="Scripting" src="https://img.shields.io/badge/Scripting-Luau-blue?style=flat-square"/>
  <img alt="License" src="https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-green?style=flat-square"/>
</p>

<p align="center">
  📖 <a href="https://docs.spyweb.app/"><b>Master Guide</b></a> |
  📂 <a href="docs/database.md">Storage</a> |
  🧪 <a href="docs/lua-testing.md">Testing</a> |
  🌐 <a href="https://docs.spyweb.app/cdp">Browser Automation</a> |
  ⚙️ <a href="docs/config.md">Config</a> |
  📂 <a href="examples/">Examples</a> |
  🏗️ <a href="CONTRIBUTING.md">Build from Source</a>
</p>

<p align="center">
  <img alt="SpyWeb Terminal Demo" src="https://spyweb.app/terminal.svg" width="100%" style="max-width: 800px;">
</p>

---

## What is SpyWeb?
**SpyWeb** is a zero-dependency web scraping & monitoring engine built for speed, efficiency and simplicity. Track listings, job boards, classifieds, price drops, restocks, public records, and anything that lives on an HTML page with simple TOML configs. Inject custom Lua logic for advanced workflows, and receive real-time alerts via desktop or webhooks, all packaged as two self-contained binaries that use under 5MB of RAM at idle.

## Quick Demo
```toml
[[jobs]]
name = "HN Front Page"
url = "https://news.ycombinator.com"
selector = ".athing"
fields = ["title:.titleline > a", "link:.titleline > a@href"]
keywords = ["rust", "linux", "open source"]
```

## Features

| Feature | Description |
| :--- | :--- |
| **Zero Dependencies** | ~7MB self-contained binary. Completely portable, no runtime required. |
| **Lua Scripting** | 10 hook stages plus persistent Lua storage for counters, cursors, and shared state. |
| **Hot Reload** | Save a config or Lua script and SpyWeb respawns the job instantly. |
| **Internal DB** | Choice of **KV** (redb, default) or **SQL** (SQLite, queryable) backends for storage and deduplication. |
| **Dual Binary** | Choice of a headless CLI or a silent system tray app for background runs. |
| **Concurrency** | Async-first engine; slow proxies or large jobs never block others. |
| **Fault Tolerant** | Lua hook errors are caught and logged without stopping the job, preventing process crashes. |
| **Hybrid Engine** | Falls back to a spec-compliant DOM parser for broken or complex HTML. |
| **CDP Automation** | Launch or connect to any Chromium browser for JS rendering, clicking, waiting, screenshots. |
| **Alerting** | Integrated desktop notifications and customizable webhooks for monitoring. |
| **Lua Testing** | Co-located `test_*` functions run in fresh Lua VMs with isolated temporary databases. |
| **Pipeline Telemetry** | Stage-by-stage tracking of execution time, memory usage, and active browsers. |
| **Multi-Worker** | Per-job concurrency with URL queue mode and shared Lua state. |
| **API Server** | Programmable Lua-defined REST endpoints at `/api/v/*` via `server/init.lua`. |

## Install & Run
Download the latest archive from the [Release Page](https://github.com/spyweb-app/spyweb/releases/tag/latest) and extract it. Two variants are available - **KV** (redb, default) and **SQL** (SQLite, queryable).

Or download via terminal:
 >To get the SQL version, append `-sql` to the download URL (e.g. `https://dl.spyweb.app/linux-sql`).

```bash
# Linux
curl -L -o spyweb.tar.gz https://dl.spyweb.app/linux && tar -xf spyweb.tar.gz && rm spyweb.tar.gz

# macOS (Intel)
curl -L -o spyweb.tar.gz https://dl.spyweb.app/mac-intel && tar -xf spyweb.tar.gz && rm spyweb.tar.gz

# macOS (Apple Silicon)
curl -L -o spyweb.tar.gz https://dl.spyweb.app/mac-arm && tar -xf spyweb.tar.gz && rm spyweb.tar.gz

# Windows (CMD, Windows 10 or later)
curl -L -o spyweb.tar.gz https://dl.spyweb.app/windows && tar -xf spyweb.tar.gz && del spyweb.tar.gz

# Windows (PowerShell)
Invoke-WebRequest -Uri https://dl.spyweb.app/windows -OutFile spyweb.tar.gz; tar -xf spyweb.tar.gz; Remove-Item spyweb.tar.gz

```

### Release Structure
```text
spyweb/
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

Both binaries serve the admin dashboard at **http://127.0.0.1:7979** and will loop each enabled job at its configured interval. See the <a href="docs/api.md">REST API</a> docs for the built-in endpoints and the <a href="docs/server.md">Programmable API Server</a> for custom Lua-defined routes at `/api/v/*`.

> **Tip:** You can customize the port with `--port` or the `SPYWEB_PORT` environment variable:
```bash
# Linux / macOS
./spyweb start --port 9000

# Or:
SPYWEB_PORT=9000 ./spyweb start

# Windows (PowerShell)
.\spyweb.exe start --port 9000

# Or:
$env:SPYWEB_PORT=9000; .\spyweb.exe start
```

## CLI Tools
```bash
./spyweb check [config|update]       # Health check or targeted validation
./spyweb version                     # Print version and active engine
./spyweb types                       # Generate Lua LSP type definitions
./spyweb update [--check|--force|--keep [suffix]|--overwrite]  # Self-update
./spyweb profile <check|clear|delete> [all|<name>]  # Browser profiles
./spyweb test [<job> [<pattern>]]    # Run Lua tests
./spyweb debug "<job>"               # Run a single job with debug output
```

<p align="center">
  <img alt="SpyWeb CLI Debug Telemetry" src="https://spyweb.app/images/spyweb-debug.png" width="100%" style="max-width: 800px;">
</p>

## Lua API & Hooks
Place a `hooks.lua` next to your config to customize the pipeline. SpyWeb provides persistent storage to track state (like page numbers or failure counts) across restarts. For multi-worker jobs, define `on_finished()` to run logic after all workers complete each iteration - see the [lifecycle docs](docs/lifecycle.md) and [multi-worker docs](docs/multi-worker.md) for details.

## Testing
SpyWeb supports co-located Lua tests for jobs. Define global functions that start with `test_` in `hooks.lua` or `tests.lua`, and run them with the `spyweb test` command.

Tests run in isolated Lua VMs with temporary databases, so global state and database changes do not leak between cases.

See [docs/lua-testing.md](docs/lua-testing.md) for the full testing workflow, file layout, and examples.

<p align="center">
  <img alt="SpyWeb Lua Unit Testing" src="https://spyweb.app/images/spyweb-test.png" width="100%" style="max-width: 800px;">
</p>

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

Check the [Examples](examples/) for more Lua hook examples.

---
 
> **JavaScript Rendering with CDP:** SpyWeb does not bundle a browser. Instead, the built-in [CDP module](docs/cdp.md) launches or connects to a Chromium-based or CDP-compatible browser already installed on your system (Chrome, Edge, Brave, Lightpanda, etc.) and controls it via the Chrome DevTools Protocol, all from your Lua hooks.
 
```lua
function override_fetch(request)
    local browser = cdp.launch({})
    defer(function() browser:close() end)

    local page = browser:attach()
    page:open(request.url)
    page:wait_for_selector(".dynamic-content", 10000)
    return { status = 200, body = page:content(), url = request.url }
end
```
 
 See the [CDP Documentation](docs/cdp.md) for the full API - browser management, page navigation, click/wait/inject, cookies, screenshots, and a complete hybrid-recovery pattern that falls back to a visual browser on bot detection.

---

## ⚖️ Use Responsibly
- **Do not hammer sites:** Set a reasonable, modest `interval` in your configurations. Scraping a page every 5 seconds is almost never necessary and costs the site owner money. Furthermore, aggressive abuse is the fastest way to get your connection throttled, flagged, or permanently IP banned.
- **Respect resources:** If you are monitoring a small independent site, be extra gentle with your request frequency.
- **Honor the web:** SpyWeb is a tool built for personal monitoring and automation; it is not a weapon for Denial of Service or aggressive data harvesting. Be modest when scraping.

---
Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
