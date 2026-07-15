# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.5.3] - 2026-07-15

### Added
- **API Server:** Added `/api/public/{name}` routes that bypass `X-SpyWeb-Key` auth for public programmable API endpoints, defined via `public.get:name` / `public.post:name` in `server/init.lua`.
- **CLI:** Added `spyweb start --no-server` flag and `SPYWEB_DISABLE_SERVER` env var to start the engine without the HTTP server (scraping/monitoring only).

## [1.5.2] - 2026-07-09

### Added
- **Lua API:** `require` now falls back to `init.lua` (e.g. `require("foo.bar")` tries `foo/bar.lua` then `foo/bar/init.lua`).
- **Lua Testing:** `spyweb test server` now auto-discovers `server/tests.lua`, starts the programmable API server on a random port, runs tests against it, and shuts it down when done.
- **Engine Info:** Added global `engine` table exposing runtime info: `os`, `arch`, `headless`, `version`, `lua_version`, `storage`.
- **TLS Probe:** Added `tls_probe(host, port?)` for certificate inspectio, returns subject, issuer, serial, validity dates, days until expiry, and fingerprint.

### Changed
- **Lua Loading:** All Lua script loading now uses async execution, enabling scripts to call async functions at load time.
- **Notifications:** Desktop notifications are now automatically skipped on headless environments.

### Fixed
- **Hot Reload:** Fix file watcher infinite reload loop on Linux when the config loader reads `.lua`/`.toml` files due to `Access(Open)` events.
- **Static Assets:** Extensionless paths (e.g. `/example`) now fall back to `index.html` for SPA client-side routing instead of returning 404.

## [1.5.1] - 2026-07-04

### Changed
- **Webhook:** Default payload format now uses flat item objects with `keywords` array instead of nested `fields`/`matches`. removed 50-item cap has been on payload.
- **Lua API:** `json_decode` now returns `(value, nil)` on success and `(nil, error)` on failure instead of throwing. 

## [1.5.0-beta] - 2026-07-01

### Added
- **CLI:** Added `spyweb types` command that writes `spyweb-types.lua` and `.luarc.json` to the current directory for Lua LSP type definitions.
- **CLI:** Added `spyweb update` command for self-updating.
- **Storage Backend:** Added **SQLite** as an optional compile-time storage backend (`--features sqlite`) alongside the default **redb** backend. swap at download by picking the `-sql` variant.
- **Lua API (SQLite only):** Added `db_query(sql, params?)` for arbitrary SELECT queries from Lua scripts. Returns an array of row-tables with column-value pairs. Integer/Real/Text/Null SQL types map to Lua number/string/nil.
- **Lua API (SQLite only):** Added `db_exec(sql, params?)` for executing INSERT, UPDATE, DELETE, REPLACE, CREATE TABLE, and any other SQL from Lua scripts. Returns the number of rows affected.
- **Lua API:** Added `fs_read(filename)` for reading files from the job directory. Returns file content as a string, or `nil` if the file does not exist. 
- **Lua API:** HTTP binding responses now include `url`, `proxy`, `time_ms`, and `size` metadata fields
- **Lua API:** All HTTP bindings (`http_get`, `http_post`, `http_request`, `http_multipart`) now return `(result, error)`, a nil result + error table on failure instead of throwing
- **Lua API:** Added `http_request(options)` generic HTTP binding supporting any method (GET, HEAD, POST, PUT, DELETE, etc.). Body is binary-safe (accepts arbitrary bytes). 
- **Lua API:** Added `http_multipart(url, fields, headers)` for multipart/file upload HTTP requests. Supports text fields and file attachments with binary content, custom filename, and MIME type.
- **Lua API:** Added `fs_read_binary(filename)` for reading any file (including binary files like images) from the job directory. Returns binary-safe Lua string; returns `nil` if file doesn't exist.
- **Config:** Added `workers` field to job config to control per-job concurrency.
- **Config:** Added `urls` field to job config for specifying multiple entry URLs.
- **Multi-worker Loop:** `run_job_loop` spawns N workers as configured by `workers` field, with 200ms stagger between worker starts to prevent thundering herd.
- **URL Dispatch:** Rust-managed `VecDeque` URL queue with work-stealing pop; workers atomically claim URLs from the shared `urls` config list.
- **on_finished hook:** New batch-level lifecycle hook `on_finished()` fires once per job cycle after all workers complete and all URLs drain, before the interval sleep. No `ctx` parameter — operates on shared globals accumulated during the batch.
- **Test Infrastructure:** Consolidated 13 scraper tests from `pipeline.rs`, `runner.rs`, `request.rs` into a single `src/scraper/tests.rs`.
- **Lua File I/O:** Added `shared/` folder support. `fs_read`/`fs_read_binary` fall back to `./shared/` when file not found in job directory. `fs_append`/`fs_overwrite` write to `./shared/` when path starts with `shared/`.
- **API Server:** Added programmable Lua-based API routing via `/api/v/<name>/...` endpoints. Requests are dispatched to `server/init.lua` which receives the HTTP method, route name, path segments, and raw request for fully custom REST endpoints alongside the built-in `/api/*` routes.

### Changed
- **CLI:** `spyweb version` now shows the compiled database backend (e.g. `SpyWeb v1.4.0-beta (Engine: Luau, DB: SQLite)` or `DB: redb`).
- **CLI:** Extended `spyweb check` with subcommands `config` and `update`. Plain `spyweb check` now also runs version, config validation, and update check.
- **Config:** refactor config to have its own validation and test file.
- **CLI:** Refactored `cli.rs` into `cli/mod.rs` (command dispatcher) and `cli/profile.rs` (profile handler + tests) for better maintainability.
- **Request Config:** `before_fetch` Lua hooks can now set `request.timeout = 15`, `request.proxy = "http://..."`,`request.max_body_size = 5`, `request.method = 'HEAD/POST` or any method.
- **HTTP Client:** Built-in fetch now enforces a 10MB response body limit by defaults
- **HTTP Client:** Proxy URL is now included in fetch error messages when a proxy was in use.
- **HTTP Client:** `ctx.last_fetch.response` now includes `proxy`, `time_ms`, and `size` metadata fields on the default fetch envelope.
- **HTTP Client:** Connection is now dropped immediately after request completion to free resources sooner.
- **Threading:** Server moved to a dedicated thread, server no longer steal and hostage one `SPYWEB_THREADS` creating deadlock if `SPYWEB_THREADS = 1`.
- **I/O Service:** Major refactor of the IO service for improved maintainability and structure.
- **Performance:** `RequestConfig.headers` wrapped in `Arc<IndexMap>` to avoid expensive clones on every fetch.
- **Compression:** Removed brotli compression support from HTTP client (drops `brotli` feature from ureq and `br` from `Accept-Encoding`).
- **Lua Context Isolation:** Hook state (`last_fetch`, `selector_matches`, `telemetry`, `__deferred`, `filter_error`) moved from global Lua scope to per-cycle context tables.
- **Pipeline Telemetry:** Introduced `TelemetryHandle` wrapper to DRY up telemetry stage recording across the entire pipeline.
- **Refactor:** `defer()` cleanup uses the active execution context via named registry instead of Lua globals.
- **Refactor:** Pipeline telemetry boilerplate consolidated behind `TelemetryHandle::stage()` and `TelemetryHandle::record()` helpers.
- **Refactor:** Collapsed 12 hook presence booleans in `JobHooks` into a `u16` bitmask.
- **Lua `require`:** Now scans job directory first, then project root. Removed `shared/` candidate.
- **Refactor:** `validate_path` refactored to only validate path safety (traversal, symlinks). Extension checks moved to callers (`handle_task`, `fs_read`, `fs_read_binary`) so each function controls its own allowlist.
- **Security:** `cdp._write_base64` now validates paths via `validate_path()` before writing to disk.
- **Security:** `page:call_save` now validates paths via `validate_path()` before writing to disk.
- **Security:** `json_decode` input capped at 10MB to prevent OOM.

### Fixed
- **Build:** Separated `spyweb` (headless) and `spyweb-tray` (tray) builds so tray feature no longer leaks into the headless binary.
- **Security:** Static asset path validation now uses `path.components()` instead of `contains("..")` to catch encoded traversal attempts.

## [1.4.0-beta] - 2026-05-29

### Added
- **Lua Testing:** Introduced a lightweight testing framework for Lua hooks via the `spyweb test <job>` CLI command, including a new `testing` Lua global for assertions and integrated IO support.
- **Auth:** Added optional stateless API authentication via `SPYWEB_API_KEY` env var. the `/api/*` routes check the `X-SpyWeb-Key` header.
- **Lua API:** Added `sleep(ms)` async binding for suspending execution.
- **CDP Transport:** Added flat session routing (`call_session`, `register_listener`, `wait_event_session`) for session-level event targeting instead of a shared event channel.
- **Browser:** Added `is_remote` flag to distinguish remote vs launched browser instances.
- **UI:** Added a color picker to the record viewer with support for automatic and manual color scheme switching.
- **Documentation:** Added `docs/lua-testing.md` and updated existing documentation and examples to reflect the latest CDP and testing features.

### Changed
- **IO Security:** Enhanced path validation in the IO service using canonicalization and component-based checks to prevent symlink and traversal attacks.
- **Server:** Refactored request handling into separate `handle_api_request` (authenticated) and `handle_static_request` (unguarded, with path traversal protection and extension allowist) branches.
- **Record Viewer:** Enhanced the built-in record viewer to support the new optional API authentication.
- **Branding:** Updated the tray icon, UI logo, and favicon for a refreshed visual identity.
- **API:** `/api/jobs` now returns `{ id, name }` objects instead of bare job IDs.
- **CDP Transport:** Replaced shared event broadcast channel with session-keyed listener map, allowing multiple independent subscribers.
- **Telemetry:** Changed memory threshold rounding from 1024 to 1000 for cleaner display.
- **Lazy-load CDP:** CDP bindings are now registered only for hook sources that reference `cdp.*`, jobs that never use CDP no longer pay the startup cost for that VM surface.
- **Lazy-load Hooks:** Disabled job no longer load `hooks.lua`, they do not create idle Lua VMs during startup or config reload.
- **Jobs API:** `/api/jobs` now includes an `enabled` flag while preserving the existing `{ id, name }` structure order for compatibility.

### Fixed
- None.

## [1.3.0-beta] - 2026-05-19

### Added
- **Telemetry:** Implemented comprehensive pipeline telemetry exposed via the `spyweb_telemetry` Lua global, tracking stage duration, memory usage, browser counts, and execution status for advanced job observability.
- **Lua API:** Added `defer(fn)` for synchronous hook-scoped cleanup.
- **Pipeline Cleanup:** Added `defer.lua` handler for async lifecycle and post-cycle cleanup.
- **Concurrency Control:** Added `SPYWEB_THREADS` environment variable (defaults to 2, max 64) to configure executor thread pool size.
- **Process Management:** Added global browser registry to terminate headless processes on unexpected exit.
- **Shutdown Helper:** Added a centralized `shutdown_system` utility helper in `services/utils` for clean process teardown.

### Fixed
- **Proxy Rotation:** Fixed clock bias in proxy rotation by switching to `fastrand`.
- **Header Parsing:** Fixed dropped duplicate HTTP headers in `flatten_headers` by joining values per RFC 9110.
- **Debug:** Fixed `debug_job` skipping keyword re-tagging during item filtering.
- **Process Management:** Fixed browser process leaks during `debug` CLI execution and daemon hot-reloads by ensuring all running browsers are terminated.
- **File I/O:** Fixed Lua File I/O functions (like `log()`, `fs_append()`, `fs_overwrite()`) failing in the `debug` CLI tool by properly initializing the IO subsystem.

### Changed
- **Lua HTTP Bindings:** `http_get` and `http_post` now return `{status, headers, body}` instead of a raw body string.
- **CDP Automation:** Changed logical CDP `close` methods (`page:close()` and `context:close()`) to be synchronous (fire-and-forget in the background) to prevent yielding errors when called inside synchronous `defer` hooks.
- **Graceful Shutdown:** Standardized Ctrl-C, tray exit, and debug commands to use the centralized shutdown helper.
- Cleaned up redundant template functions and unused code blocks.

## [1.2.0-beta] - 2026-05-16

### Added
- **Lua File I/O:** Added background worker and MPSC channel for non-blocking disk operations.
- **Lua API:** Added `fs_append(filename, content)` and `fs_overwrite(filename, content)`.
- **Auto-Rotation:** Added timestamp-based file rotation with 10MB limit and 5-file history.
- **IO Security:** Added path validation (rejects absolute paths, directory traversal) and extension allowlist (.csv, .json, .jsonl, .txt, .log).
- **Logging:** Updated `log()` to use the new IO backend.
- **CDP Automation:** Added browser automation support (navigate, click, JS injection, screenshots, cookies, network interception).
- **CDP Connect:** Added optional headers argument to `cdp.connect()`.
- **Lua API:** Added `cdp` global with `launch()` and `connect()`.
- **Lua API:** Added `browser:attach()` to create new pages.
- **Lua API:** Added page methods: `open`, `content`, `evaluate`, `click`, `wait_for_selector`, `wait_for_navigation`, `wait_event`, `screenshot`, `block_resources`, `fulfill_request`.
- **Lua API:** Added page helpers: `wait_for_url`, `wait_for_response`, `scroll`, `set_extra_headers`, `set_user_agent`, `cookies`, `set_cookies`.
- **CLI:** Added `spyweb profile <check|list|clear|delete> [job|all]` to manage Chrome user data directories.
- **Browser Auto-Detection:** Added detection for Chrome, Chromium, Edge, and Brave, with `$BROWSER` override.
- **CDP Transport:** Added session-level targeting, timeout-aware waiting, and clean shutdown.
- **CDP API:** Added `browser:close()`.
- **Examples:** Added `hybrid-recovery/` example.

### Changed
- **Docs:** Updated README with CDP and JS rendering examples.
- **CLI:** Migrated argument parsing to `clap`.
- **HTTP client:** Updated default User-Agent to Chrome 148, added default browser headers, and switched to `IndexMap` for header ordering.

### Fixed
- Server now returns a descriptive error on port bind failure instead of panicking.

## [1.1.0] - 2026-05-05

### Added
- Added `--port` flag and `SPYWEB_PORT` environment variable to override REST API port.
- **Lua API:** Added `dump()`, `copy()`, and `deep_copy()` globals.
- **Lua API:** Added `selector_matches` global variable.
- Added terminal colors and unified logging.
- **Lua API:** Added `override_fetch` and `override_extract` hooks.
- **CLI:** Updated `debug` command to support the 7 core stages of the Lua pipeline (safely bypassing notification and webhook stages).
- **Examples:** Added `set-and-forget` and `js-rendering` examples.

### Changed
- **Lua API:** Changed `after_fetch` hook to receive `{ ok, request, response, error }`. The HTTP agent now passes 4xx/5xx responses to hooks.
- Output now automatically strips ANSI colors when piped to a file.
- `Runner::fetch` preserves full error contexts.
- **Refactor:** Moved unit tests to dedicated `tests.rs` files.
- **Refactor:** Centralized Lua VM lifecycle in `engine.rs` and conversion logic in `conversions.rs`.
- **Refactor:** Modularized Lua bindings into `network.rs`, `storage.rs`, and `system.rs`.
- **Refactor:** Decomposed `hooks.rs` into `hooks/mod.rs` and `hooks/stages.rs`.

### Fixed
- Isolated the notification service to prevent execution failures from affecting subsequent webhook delivery.
- Updated all documentation and examples to match the latest Lua API and project architecture.
- Fix hot reload to only drop handles when new config parses successfully.
- Enhanced Mac build support for universal binary distribution.
- **Infrastructure:** Implemented a "Rolling Latest" release strategy in CI/CD, providing stable version-independent download links for the latest stable binaries.

## [1.0.0] - 2026-04-30

### Added
- Initial stable "Clean Release".
    