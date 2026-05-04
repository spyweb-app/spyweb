# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
