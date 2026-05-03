# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.1.0] - Unreleased

### Added
- Added `--port` CLI flag and `SPYWEB_PORT` environment variable to override the default REST API port.
- Introduced `dump()` global helper function in Lua for pretty-printing tables to the console during debugging.
- Added comprehensive semantic terminal colors for a much better CLI experience (job names, warnings, execution times).

### Changed
- **Lua API:** Refactored the `after_fetch` hook to receive a unified `fetch_result` envelope (`{ ok, request, response, error }`), allowing hooks to safely access HTTP errors and failed request states.
- Terminal output now automatically detects TTY environments and strips ANSI colors when logging is piped to a file.
- `Runner::fetch` now preserves full error contexts instead of swallowing early failures.

### Fixed
- Fixed outdated documentation and examples referencing old fetch_result structure.
- Fix hot reload to only drop handles when new config parses successfully

## [1.0.0] - 2026-04-30

### Added
- Initial stable "Clean Release".
