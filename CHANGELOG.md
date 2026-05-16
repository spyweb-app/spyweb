# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0-beta] - 2026-05-16

### Added
- **Safe Lua File I/O:** Centralized, non-blocking disk operations for Lua hooks. All writes pass through a single-threaded background worker via an MPSC channel to prevent race conditions and disk contention.
- **Lua API:** New `fs_append(filename, content)` and `fs_overwrite(filename, content)` functions for custom data exports.
- **Auto-Rotation:** Implemented timestamp-based file rotation (e.g., `data.20260514-143005.csv`). Files feature a hard 10MB limit and maintain a history of 5 files to prevent disk exhaustion.
- **IO Security:** Strict path validation including absolute path rejection, directory traversal prevention (`../`), and an extension allowlist (.csv, .json, .jsonl, .txt, .log). Operations are confined to the job's directory.
- **Refactored Logging:** The `log()` function now uses the new safe IO backend, gaining automatic rotation and high-performance async behavior.
- **CDP Browser Automation:** Added browser automation support and full page control: navigate, click, wait for selectors, inject JavaScript, capture screenshots, manage cookies, intercept network requests.
- **CDP connect headers:** `cdp.connect(ws_url, [headers])` now accepts an optional headers table for custom HTTP headers (e.g., `Authorization: Bearer <token>`).
- **Lua API:** New global `cdp` table with `cdp.launch({...})` and `cdp.connect("ws://...")` for browser lifecycle management.
- **Lua API:** `browser:attach()` creates a new page/tab with its own WebSocket connection.
- **Lua API:** 10 native page methods: `open`, `content`, `evaluate`, `click`, `wait_for_selector`, `wait_for_navigation`, `wait_event`, `screenshot`, `block_resources`, `fulfill_request`.
- **Lua API:** 7 high-level page helpers injected via `cdp.lua`: `wait_for_url`, `wait_for_response`, `scroll`, `set_extra_headers`, `set_user_agent`, `cookies`, `set_cookies`.
- **CLI:** `spyweb profile <check|list|clear|delete> [job|all]` — manage per-job browser profile directories (Chrome user data dirs). Check status with lock detection, wipe caches, or delete profiles entirely.
- **Browser Auto-Detection:** Finds Chrome, Chromium, Edge, and Brave on Linux, macOS, and Windows without any configuration. Supports `$BROWSER` environment variable to override detection.
- **CDP Transport:** Session-level targeting (`call_session`), timeout-aware event waiting (`wait_event_timeout`), and clean shutdown that drains pending requests with errors.
- **CDP API:** `browser:close()` — explicitly kills the browser process, closes the WebSocket transport, and releases the profile lock file.
- **Examples:** Added `hybrid-recovery/` — production-grade pattern that uses headless CDP by default, detects bot blocks, launches a visible browser for human intervention, then captures the recovered session.

### Changed
- **Docs:** Updated README with CDP feature description, CLI profile commands, and JS rendering examples.
- **CLI:** Migrated argument parsing from manual `args.get()` dispatch to `clap` derive. Removed hand-rolled `parse_start_port`, `profile_usage`, and string-slice routing, replaced with typed subcommand enums. Auto-generated help/usage.

### Fixed
- Server no longer panics on port bind failure but returns descriptive error (e.g. "Failed to start server on 0.0.0.0:7979: Address in use") instead of crashing.

## [1.1.0] - 2026-05-05

### Added
- Added `--port` CLI flag and `SPYWEB_PORT` environment variable to override the default REST API port.
- Introduced `dump()`, `copy()`, and `deep_copy()` global helper functions in Lua for managing tables during debugging and state persistence.
- Added a `selector_matches` global variable in Lua, allowing hooks to see how many elements matched the main selector (even if they were later filtered out).
- Added comprehensive semantic terminal colors and a unified logging system for a better CLI experience.
- **Lua API:** Added `override_fetch` hook to bypass the built-in HTTP client for custom fetching (e.g. headless browsers).
- **Lua API:** Added `override_extract` hook to bypass the built-in HTML/CSS parser for custom extraction (e.g. JSON/XML scraping).
- **CLI:** Enhanced the `debug` command to support the full 9-stage Lua pipeline, providing detailed console output for `override_fetch` and `override_extract` stages.
- **Examples:** Added new high-level examples including `set-and-forget` (autonomous Sentinel pattern) and `js-rendering`.

### Changed
- **Lua API:** Refactored the `after_fetch` hook to receive a unified `fetch_result` envelope (`{ ok, request, response, error }`). The built-in HTTP agent now treats 4xx/5xx status codes as valid responses to enable hook interception.
- Terminal output now automatically detects TTY environments and strips ANSI colors when logging is piped to a file.
- `Runner::fetch` now preserves full error contexts instead of swallowing early failures.
- **Refactor:** Decoupled unit tests from core modules into dedicated `tests.rs` files across the Lua engine and scraper modules.
- **Refactor:** Overhauled the Lua engine architecture by centralizing VM lifecycle in `engine.rs` and extracting data conversion logic into `conversions.rs`.
- **Refactor:** Modularized Lua bindings into a granular directory structure: `bindings/network.rs` (HTTP), `bindings/storage.rs` (DB), and `bindings/system.rs` (OS utilities).
- **Refactor:** Decomposed the monolithic `hooks.rs` into a structured module (`hooks/mod.rs`, `hooks/stages.rs`) to separate hook orchestration from individual stage execution logic.

### Fixed
- Isolated the notification service to prevent execution failures from affecting subsequent webhook delivery.
- Updated all documentation and examples to match the latest Lua API and project architecture.
- Fix hot reload to only drop handles when new config parses successfully.
- Enhanced Mac build support for universal binary distribution.
- **Infrastructure:** Implemented a "Rolling Latest" release strategy in CI/CD, providing stable version-independent download links for the latest stable binaries.

## [1.0.0] - 2026-04-30

### Added
- Initial stable "Clean Release".
