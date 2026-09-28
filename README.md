<p align="center">
  <h1 align="center">SpyWeb</h1>
</p>

<p align="center">
  <img alt="Release" src="https://img.shields.io/github/v/release/spyweb-app/spyweb?style=flat-square&label=Release"/>
  <img alt="Language" src="https://img.shields.io/badge/Language-Rust-orange?style=flat-square"/>
  <img alt="Scripting" src="https://img.shields.io/badge/Scripting-Luau-blue?style=flat-square"/>
  <img alt="License" src="https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-green?style=flat-square"/>
</p>

<p align="center">
  <a href="https://docs.spyweb.app/getting-started/"><b>Getting Started</b></a> |
  <a href="https://docs.spyweb.app/job-configuration/toml-config/">Config</a> |
  <a href="https://docs.spyweb.app/hook-reference/">Hooks</a> |
  <a href="https://docs.spyweb.app/browser-automation/">Browser Automation</a> |
  <a href="https://docs.spyweb.app/testing/">Testing</a> |
  <a href="https://docs.spyweb.app/api-and-server/">API & Server</a>
</p>

<p align="center">
  <img alt="SpyWeb Terminal Demo" src="https://spyweb.app/terminal.svg" width="100%" style="max-width: 800px;">
</p>

---

## What is SpyWeb?
**SpyWeb** is a zero-dependency web scraping & monitoring automation engine built for speed, efficiency and simplicity. Track listings, job boards, classifieds, price drops, restocks, public records, and anything that lives on an HTML page with simple TOML configs. Inject custom Lua logic for advanced workflows, and receive real-time alerts via desktop or webhooks, all packaged as two self-contained binaries (~8MB) that use under 10MB of RAM at idle.

## Demo

#### Built with SpyWeb
> **[UPTYME](https://github.com/spyweb-app/uptyme)** - a full production app running entirely on the SpyWeb engine: self-hosted website/API uptime monitoring with alerts, public status pages, and multi-location consensus. Vue/TypeScript dashboard.
>
> **[Live demo](https://uptyme.spyweb.app/)** (API key: `im-a-secure-key`)

#### Simple Jobs
```toml
[[jobs]]
name = "HN Front Page"
url = "https://news.ycombinator.com"
selector = ".athing"
fields = ["title:.titleline > a", "link:.titleline > a@href"]
keywords = ["rust", "linux", "open source"]
```

See the [Job Configuration](https://docs.spyweb.app/job-configuration/toml-config/) docs for every option.

## Features

| Feature | Description |
| :--- | :--- |
| **Zero Dependencies** | ~8MB self-contained binary. Completely portable, no runtime required. |
| **Scheduler** | Interval-based job scheduling - enable a job and SpyWeb loops it at its configured interval. |
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
| **Programmable API Server** | Lua-defined REST endpoints at `/api/v/*` via `server/init.lua`. |

## Install & Run
Download the latest archive from the [Release Page](https://github.com/spyweb-app/spyweb/releases/latest) and extract it. Two variants are available - **KV** (redb, default) and **SQL** (SQLite, queryable). See [Getting Started](https://docs.spyweb.app/getting-started/) for details and [File Structure](https://docs.spyweb.app/file-structure/) for what every file does.

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
Press `Ctrl+C` or send `SIGTERM` to quit - SpyWeb shuts down gracefully: jobs stop first, browsers close, then the database is checkpointed.

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

Other useful flags:

- **`-q`** / **`-qq`** - reduce log output (warnings / errors only), or set `SPYWEB_LOG` to `info`, `warn`, or `error`
- **`--no-server`** (or `SPYWEB_DISABLE_SERVER`) - run scraping/monitoring only, without the dashboard/API

## CLI Tools
```bash
./spyweb check [config|update]       # Health check or targeted validation
./spyweb version                     # Print version and active engine
./spyweb types                       # Generate Lua LSP type definitions
./spyweb update [--check|--force|--keep [suffix]|--overwrite]  # Self-update
./spyweb profile <check|list> [job]  # Browser profile status
./spyweb profile clear <target>      # Wipe a profile (cookies/cache, keeps directory)
./spyweb profile delete <target>     # Delete a profile directory entirely
./spyweb test [<job> [<pattern>]]    # Run Lua tests (`spyweb test server` for the API suite)
./spyweb debug "<job>"               # Run a single job with debug output
```

See [CLI & Operations](https://docs.spyweb.app/cli-and-operations/) for the full reference.

<p align="center">
  <img alt="SpyWeb CLI Debug Telemetry" src="https://spyweb.app/images/spyweb-debug.png" width="100%" style="max-width: 800px;">
</p>

## Lua Hooks & Storage
Place a `hooks.lua` next to your config to customize the pipeline. SpyWeb provides persistent storage to track state (like page numbers or failure counts) across restarts. For multi-worker jobs, define `on_finished()` to run logic after all workers complete each iteration - see the [Hook Reference](https://docs.spyweb.app/hook-reference/) for all 10 stages, the [lifecycle](https://docs.spyweb.app/hook-reference/lifecycle/) order, and the [multi-worker docs](https://docs.spyweb.app/job-configuration/multi-worker/) for details.

### Scoped vs Global Storage
*   **`store_get/set/delete(key)`**: Scoped to the individual job. Safe for standard logic because hooks for a single job are sequential.
*   **`global_store_incr(key, default, delta)`**: **Atomic**. Use this when you need to mutate shared state across *multiple* jobs simultaneously to avoid race conditions.
*   **`global_store_get/set/delete(key)`**: Shared across all jobs.

See [Storage](https://docs.spyweb.app/job-configuration/storage/) for the full reference.

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

## Testing
SpyWeb supports co-located Lua tests for jobs. Define global functions that start with `test_` in `hooks.lua` or `tests.lua`, and run them with the `spyweb test` command.

Tests run in isolated Lua VMs with temporary databases, so global state and database changes do not leak between cases. The programmable API server has its own suite - `spyweb test server` auto-starts the server on a random port, runs the tests against it, and shuts it down when done.

See the [Lua Testing docs](https://docs.spyweb.app/testing/) for the full testing workflow, file layout, and examples.

<p align="center">
  <img alt="SpyWeb Lua Unit Testing" src="https://spyweb.app/images/spyweb-test.png" width="100%" style="max-width: 800px;">
</p>

## Browser Automation (CDP)
SpyWeb does not bundle a browser. Instead, the built-in CDP module launches or connects to a Chromium-based or CDP-compatible browser already installed on your system (Chrome, Edge, Brave, Lightpanda, etc.) and controls it via the Chrome DevTools Protocol, all from your Lua hooks.

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

See the [Browser Automation docs](https://docs.spyweb.app/browser-automation/) for the full API - browser management, page navigation, click/wait/inject, cookies, screenshots, and a complete hybrid-recovery pattern that falls back to a visual browser on bot detection.

## Programmable API Server
Beyond the built-in dashboard and JSON API, SpyWeb serves custom Lua-defined REST endpoints at `/api/v/*` via `server/init.lua` - define routes, handlers, and public (keyless) endpoints entirely in Lua. Build dashboards, integrations, or health endpoints on top of your scraping data.

The server runs alongside the engine by default; use `--no-server` (or `SPYWEB_DISABLE_SERVER`) for scraping-only mode.

See [REST API & Custom Server](https://docs.spyweb.app/api-and-server/) for route definitions, authentication, and deployment.

---

## ⚖️ Use Responsibly
- **Do not hammer sites:** Set a reasonable, modest `interval` in your configurations. Scraping a page every 5 seconds is almost never necessary and costs the site owner money. Furthermore, aggressive abuse is the fastest way to get your connection throttled, flagged, or permanently IP banned.
- **Respect resources:** If you are monitoring a small independent site, be extra gentle with your request frequency.
- **Honor the web:** SpyWeb is a tool built for personal monitoring and automation; it is not a weapon for Denial of Service or aggressive data harvesting. Be modest when scraping.

See [Sandboxing & Security](https://docs.spyweb.app/security/) for how SpyWeb isolates Lua scripts and filesystem access.

---
Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
