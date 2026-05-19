# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
