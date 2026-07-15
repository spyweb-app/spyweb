# SpyWeb Lua Testing

SpyWeb includes a lightweight Lua testing mode for job hooks. The goal is to keep tests close to production logic while still running each test in a fresh, isolated Lua VM.

## Overview

The test runner looks for global Lua functions whose names start with `test_`.

This lets you keep tests in the same file as the hook code or in a separate `tests.lua` file in the job directory.

Key properties:

- No registration tables
- No special test DSL
- Fresh Lua VM for every test function
- Temporary Redb database per test run
- Production Lua bindings are available in tests
- Production `ctx` table injected as `active_ctx` in the Lua registry, enabling `defer()` and ctx-aware assertions

## CLI Usage

Use the `test` subcommand:

```bash
spyweb test
spyweb test "Job Name"
spyweb test "Job Name" price
spyweb test server
spyweb test server price
```

Behavior:

- `spyweb test` runs every discovered `test_*` function across all jobs with Lua files, and also server tests if `server/tests.lua` exists.
- `spyweb test "Job Name"` runs tests for the matching job.
- `spyweb test "Job Name" price` runs only tests whose function name contains `price`.
- `spyweb test server` runs tests for the programmable API server at `server/tests.lua`, auto-starting the server on a random port.
- `spyweb test server price` runs only server tests whose name contains `price`.

Job matching uses the same exact resolver as the rest of the app, so it matches by job name or normalized job id.

## Test Discovery

SpyWeb discovers tests by loading Lua files for a job and then scanning global values for functions whose names start with `test_`.

Loaded files:

- `hooks.lua`
- `defer.lua`
- `tests.lua`

Notes:

- `hooks.lua` is the primary place to co-locate tests with production hooks.
- `tests.lua` is useful for larger suites or shared test helpers.
- `defer.lua` is also loaded so tests see the same lifecycle helpers as production.

Only global functions are discovered. Local functions are not treated as tests.

Example:

```lua
function extract_id(text)
    return text:match("ID%-(%d+)")
end

function test_extract_id()
    spyweb.assert_eq(extract_id("Product ID-9982"), "9982")
end
```

## Execution Model

Each test function runs in its own fresh Lua VM.

That means:

- Global mutations from one test do not leak into another.
- Global mocks set in one test do not affect later tests.
- Each test gets a new temporary Redb database file.

This isolation is important because SpyWeb keeps real bindings active during tests.

## Available Bindings

Tests get the same core bindings as production jobs, including:

- `http_get`
- `http_post`
- `http_request` — generic `{ method, url, body?, headers? }`
- `http_multipart` — multipart file uploads
- `tls_probe` — TLS certificate inspection
- `engine` — runtime environment info
- `fs_read_binary` — binary file reads
- storage helpers such as `store_set`, `store_get`, `global_store_set`, `global_store_get`
- `spyweb.assert_eq`
- `spyweb.assert_ne`

The HTTP bindings are async under the hood, and the test runner executes test functions asynchronously so they work correctly.

### Example: HTTP Test

```lua
function test_fetch_remote_page()
    local res = http_get("http://127.0.0.1:8080/")
    spyweb.assert_eq(res.status, 200)
    spyweb.assert_eq(res.body, "OK")
end

function test_head_request()
    local res = http_request({ method = "HEAD", url = "http://127.0.0.1:8080/" })
    spyweb.assert_eq(res.status, 200)
    spyweb.assert_eq(res.body, "")
end
```

### Example: Storage Seed

```lua
function test_seed_state()
    store_set("page", "3")
    spyweb.assert_eq(store_get("page"), "3")
end
```

## Mocking

Tests can overwrite globals inside the Lua VM to mock behavior.

Example:

```lua
function test_override_fetch_logic()
    override_fetch = function(req, ctx)
        return { status = 200, body = "mock", url = req.url }
    end

    local result = override_fetch({ url = "https://example.com" })
    spyweb.assert_eq(result.body, "mock")
end
```

Because each test uses a fresh VM, that override only affects the current test.

## Assertion Helpers

SpyWeb exposes a small assertion surface under `spyweb`.

### `spyweb.assert_eq(left, right, [message])`

Fails when the two values are not equal.

### `spyweb.assert_ne(left, right, [message])`

Fails when the two values are equal.

These helpers work with Lua scalars and tables.

## Recommended Structure

For small jobs:

- Keep tests in `hooks.lua` next to the hook they verify.

For larger jobs:

- Put production hooks in `hooks.lua`
- Put shared helpers and broader test cases in `tests.lua`
- Keep `test_*` functions global

## Server Testing

The programmable API server (`server/init.lua`) can be tested the same way. If `server/tests.lua` exists alongside `server/init.lua`, the test runner automatically starts the server on a random port, loads `init.lua` so helpers are available, runs tests, and shuts it down.

```lua
-- server/tests.lua

-- Logic test — helpers from init.lua are available
function test_format_price()
    spyweb.assert_eq(format_price(100), "$100.00")
end

-- Integration test — hits the server over HTTP
function test_hello_endpoint()
    local resp = http_get("http://127.0.0.1:" .. SERVER_PORT .. "/api/v/hello")
    spyweb.assert_eq(resp.status, 200)
end

-- Test a public endpoint (no auth required)
function test_public_endpoint()
    local resp = http_get("http://127.0.0.1:" .. SERVER_PORT .. "/api/public/status")
    spyweb.assert_eq(resp.status, 200)
end
```

The `SERVER_PORT` global tells tests where to reach the server. The server and all tests share the same temporary database.

Both private (`get:name`, `post:name`) and public (`public.get:name`, `public.post:name`) route handlers can be unit-tested directly by calling them from Lua, or integration-tested over HTTP via `/api/v/` and `/api/public/` respectively.

## Failure Output

When a test fails, SpyWeb prints the test name, the job name, and the Lua error context.

Typical output shape:

```text
running 1 test for job 'inventory-sync'
test test_id_extraction ... FAILED

failures:

---- test_id_extraction stdout ----
...

test result: FAILED. 0 passed; 1 failed; finished in 0.01s
```

## Practical Limits

The test runner is intentionally small and direct. It does not try to be a full Lua test framework.

Things to keep in mind:

- Test discovery is name-based, not annotation-based.
- Only global `test_*` functions are discovered.
- Tests run in isolated VMs, so state does not persist between them.
- `defer.lua` is loaded, and `defer()` callbacks follow the same lifecycle rules as production hooks.
- A production `ctx` table is registered as `active_ctx` before each test, so `defer()` and any other ctx-dependent bindings work out of the box.

## Example Layout

```text
jobs/
  inventory-sync/
    config.toml
    hooks.lua
    defer.lua
    tests.lua
```

## Example Suite

`hooks.lua`

```lua
function extract_id(text)
    return text:match("ID%-(%d+)")
end

function test_extract_id()
    spyweb.assert_eq(extract_id("Product ID-9982"), "9982")
end
```

`tests.lua`

```lua
function test_more_cases()
    spyweb.assert_eq(extract_id("SKU ID-1234"), "1234")
    spyweb.assert_ne(extract_id("No match"), "1234")
end
```

## Troubleshooting

### No tests are found

- Make sure the function name starts with `test_`.
- Make sure the function is global, not `local`.
- Make sure the file is in the job directory and is one of `hooks.lua`, `defer.lua`, or `tests.lua`.

### A test fails when using `http_get` or `http_post`

- Confirm the URL is reachable.
- If you are mocking, ensure the mock is assigned before the call.
- Remember that each test runs in a fresh Lua VM.

### A helper defined in `defer.lua` is missing

- Verify the file is named exactly `defer.lua`.
- Make sure it lives next to the job's `hooks.lua`.
