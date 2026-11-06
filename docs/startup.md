# Startup Lua

`startup.lua` is an optional file in the project root. It runs global setup code **once per process**, after the database is open and before configuration or job hooks are loaded.

Use it for work that must happen once before the engine starts, instead of duplicating the same setup in every job's `hooks.lua`.

```text
project/
├── startup.lua       # optional, runs once per process
├── jobs.toml
├── jobs/
│   └── demo/
│       ├── config.toml
│       └── hooks.lua
└── server/
    └── init.lua
```

## Lifecycle

Startup order:

1. Ctrl-C handler is installed.
2. The database opens (`data`).
3. **`startup.lua` runs.**
4. Configuration and job hooks load.
5. Shutdown hooks, web server, watcher, and jobs start.

`startup.lua` runs only for:

- `spyweb start`
- `spyweb-tray`

It does **not** run for `spyweb debug`, `spyweb test`, `spyweb check`, `spyweb profile`, or `spyweb update`. `spyweb check config` parses `startup.lua` for syntax but does not execute it.

"Once" means once per OS process launch. Restarts and separate processes run it again. Hot reload, job cycles, and HTTP requests never run it. Make `startup.lua` idempotent.

## Errors

A missing `startup.lua` is a no-op.

If the file exists but fails to read, has a syntax error, or raises a runtime error, startup aborts. The CLI exits non-zero. The tray app logs the error and exits instead of running without an engine.

## Throwaway VM

`startup.lua` runs in its own throwaway Lua VM. The VM is dropped after the file finishes. In-memory globals do not survive into jobs, server handlers, or later restarts.

Persist state with:

- `global_store_*`
- the database (`db_query` / `db_exec` on SQLite builds)
- files under the project root

## Available bindings

| Binding | Notes |
|---|---|
| `engine` | Runtime information table |
| `env_get` | Reads `SPYWEB_*` environment variables |
| `log` | Writes to `startup.log` |
| `fs_append`, `fs_overwrite`, `fs_read`, `fs_read_binary` | Project-root relative; existing path and extension restrictions apply |
| `http_request`, `http_get`, `http_post`, `http_multipart` | Full network access |
| `tls_probe` | TLS certificate inspection |
| `json_encode`, `json_decode` | JSON helpers |
| `require` | Luau builds only; resolves from the project root |
| `db_query`, `db_exec` | Real on SQLite builds; KV builds return the usual variant error |
| `global_store_set`, `global_store_get`, `global_store_incr`, `global_store_delete` | Shared storage across jobs |
| `sleep` | Async delay |
| `notify` | Real desktop notification binding |
| `spyweb.assert_eq`, `spyweb.assert_ne` | Assertions |
| `dump`, `copy`, `deep_copy` | Lua helpers |

The base Lua/Luau standard library is not restricted. `startup.lua` is trusted project code with the same trust model as `hooks.lua` and `server/init.lua`.

## Unavailable bindings

These names are present as explicit error stubs:

| Binding | Error |
|---|---|
| `store_set`, `store_get`, `store_delete` | `store_* is job-scoped and unavailable in startup.lua; use global_store_* or db_*` |
| `defer` | `defer is only available inside cycle hooks; not available in startup.lua` |
| `queue_push` | `queue_push is only available inside on_start; not available in startup.lua` |
| `cdp` | `cdp is not available in startup.lua` |

Use `global_store_*` or `db_*` instead of job-scoped `store_*`. CDP/browser automation belongs in job hooks.

## Hot reload

The root `startup.lua` is not hot-reloaded. Restart SpyWeb to apply changes.

Other `.lua` and `.toml` files still trigger the normal reload. A nested file such as `jobs/demo/startup.lua` is not special and reloads like any other Lua file.

## Caveats

- Startup is synchronous. Slow network calls delay the server and jobs; there is no startup timeout.
- `startup.lua` runs before config validation. Invalid job config is detected after startup side effects.
- `notify` may fail on headless systems and abort startup unless wrapped in `pcall`.
- Multiple processes against the same project each run `startup.lua`. Coordinate with the database, `global_store_*`, or a lock file when needed.
- No persistent VM, shutdown hook, reload hook, CDP, `defer`, or job-scoped `store_*`.

## Examples

### Create a SQLite table once

```lua
if engine.storage == "sqlite" then
    db_exec([[
        CREATE TABLE IF NOT EXISTS cache (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        )
    ]])
end
```

### Seed global state

```lua
if global_store_get("schema_version") == nil then
    global_store_set("schema_version", "1")
end
```

### Validate required configuration

```lua
local token = env_get("API_TOKEN")
if token == nil or token == "" then
    error("SPYWEB_API_TOKEN is required")
end

global_store_set("api_token", token)
```

### Write a startup lock file

```lua
local previous = fs_read("run.lock")
if previous then
    log("replacing previous run.lock")
end

fs_overwrite("run.lock", tostring(os.time()))
```

### Optional notification

```lua
local ok, err = pcall(notify, "SpyWeb", "startup complete", 3000)
if not ok then
    log("notification skipped: " .. tostring(err))
end
```
