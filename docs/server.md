# Programmable API Server

SpyWeb includes a built-in programmable API server that lets you define custom HTTP endpoints via Lua. Create a `server/init.lua` file in your project root and define handlers using method-keyed tables - SpyWeb routes incoming requests to your Lua functions automatically.

## Quick Start

1. Create `server/init.lua` in your project root:

```lua
function get:hello()
    return { status = 200, body = "Hello from SpyWeb!" }
end

function post:echo()
    return { status = 200, body = self.body, headers = { ["Content-Type"] = "text/plain" } }
end
```

2. Start SpyWeb normally:

```bash
./spyweb start
```

3. Hit your endpoints:

```bash
curl http://127.0.0.1:7979/api/v/hello
curl -X POST -d "test body" http://127.0.0.1:7979/api/v/echo
```

## How It Works

Each HTTP request to `/api/v/{name}` triggers the following:

1. SpyWeb reads `server/init.lua` from disk (automatic hot-reload - no restart needed)
2. A fresh Lua VM is created for the request
3. The Lua script is loaded, defining your route handlers
4. SpyWeb looks up `get["name"]` or `post["name"]` - falls back to `all["name"]` if the method-specific table has no match
5. The handler is called with `self` carrying the request context
6. The return value is converted to an HTTP response
7. The VM is dropped - no shared state between requests

## Route Registration

Routes are registered by defining functions on the pre-injected method tables using Lua's `:` syntax:

```lua
function get:users()
    -- handles GET /api/v/users
end

function post:users()
    -- handles POST /api/v/users
end

function all:ping()
    -- handles any method on /api/v/ping
end
```

The method tables `get`, `post`, `put`, `patch`, `delete`, and `all` are already injected into the Lua globals before your script runs - you don't need to create them.

Functions defined outside method tables are private helpers - they are never exposed as endpoints:

```lua
-- This is a private helper, not an endpoint
function validate_input(data)
    return data and data.name
end

function get:users()
    local users = fetch_users()
    return { status = 200, body = users }
end
```

## URL Structure

```
/api/v/{name}/{path_args...}
```

- `{name}` - the handler name (looked up in method tables)
- `{path_args...}` - optional trailing segments passed to `self.path_args`

Examples:

| URL | Handler | `self.path_args` |
|-----|---------|------------------|
| `/api/v/users` | `get.users` | `{}` |
| `/api/v/users/123` | `get.users` | `{"123"}` |
| `/api/v/users/123/profile` | `get.users` | `{"123", "profile"}` |

## Request Context (`self`)

Inside a `:` handler, `self` is a table containing the request data:

| Field | Type | Description |
|-------|------|-------------|
| `self.body` | string or nil | Raw request body (POST, PUT, PATCH). nil for GET/HEAD |
| `self.method` | string | HTTP method (`"GET"`, `"POST"`, etc.) |
| `self.path` | string | Full request path (`/api/v/users/123`) |
| `self.path_args` | table | Array of trailing path segments (`{"123"}`) |
| `self.query` | table | URL query parameters as key-value pairs |
| `self.headers` | table | Request headers (lowercase keys) |
| `self.client_ip` | string | Client IP address |

Example:

```lua
function get:search()
    local query = self.query
    local term = query.q or ""
    local limit = tonumber(query.limit) or 10

    return {
        status = 200,
        body = { query = term, limit = limit, results = search(term, limit) }
    }
end
```

## Response Contract

Handlers can return three types:

### 1. Table (full control)

```lua
return {
    status = 200,           -- HTTP status (clamped to 100-599)
    body = "Hello!",        -- string, table (auto-JSON), number, boolean, or binary
    headers = {             -- optional extra headers
        ["X-Custom"] = "value"
    }
}
```

**Body types:**

| Lua Type | Response |
|----------|----------|
| string | Raw body as-is (use for text or binary) |
| table | Auto-encoded as JSON with `Content-Type: application/json` |
| number | Converted to string |
| boolean | Converted to `"true"` or `"false"` |
| nil | Empty body |

> **Binary Responses:** To serve images, PDFs, or other binary data, read the file using `fs_read_binary()` and return it in the `body` while explicitly setting the `Content-Type` header (e.g., `image/png`).

### 2. String (quick response)

```lua
return "Hello, World!"
-- Equivalent to: { status = 200, body = "Hello, World!" }
```

### 3. Nil (empty 200)

```lua
return nil
-- Equivalent to: { status = 200, body = "" }
```

## Error Handling

If a handler throws a Lua error:

1. The error is logged to the terminal
2. The error is appended to `server/error.log` with a timestamp
3. The client receives: `{"error": "Internal Server Error"}` with status 500

If `server/init.lua` has a syntax error, all requests fail with 500 until the file is fixed.

## Timeout

With Luau (the default), each request has a **30-second timeout**. If a handler runs longer than 30 seconds, it is forcefully terminated and the client receives a 500 error with `"Script execution timed out"`.

This protects against infinite loops freezing the server. The timeout is enforced via Luau's bytecode interrupt hook - no threads are leaked.

> **Note:** Timeout is only available with Luau. Lua 5.4 builds do not have timeout protection.

## Available Lua Globals

The server VM has access to the same globals as scraper hooks:

### File I/O

| Function | Description |
|----------|-------------|
| `fs_read(filename)` | Read a text file from `server/` directory, falls back to `shared/` |
| `fs_read_binary(filename)` | Read a binary file (images, fonts, etc.), falls back to `shared/` |
| `fs_append(filename, content)` | Append to a file in `server/` |
| `fs_overwrite(filename, content)` | Overwrite a file in `server/`, or `shared/` if prefixed with `shared/` |
| `log(message)` | Append timestamped line to `server/server.log` |

> Writes go to `server/` by default. Use `shared/` prefix (e.g., `fs_overwrite("shared/data.json", ...)`) to write to the project root's shared folder. Reads scan `server/` first, then fall back to `shared/`. Text operations are restricted to `.csv`, `.json`, `.jsonl`, `.txt`, and `.log` extensions. **`fs_read_binary`** additionally supports media and assets including `.png`, `.jpg`, `.jpeg`, `.gif`, `.svg`, `.webp`, `.bmp`, `.ico`, `.pdf`, `.zip`, `.woff`, `.woff2`, `.ttf`, `.otf`, etc.

### HTTP Client

| Function | Description |
|----------|-------------|
| `http_get(url, [headers])` | HTTP GET request. Returns `(res, err)` — two-return pattern. |
| `http_post(url, body, [headers])` | HTTP POST request. Returns `(res, err)` — two-return pattern. |
| `http_request({ method, url, body?, headers?, proxy?, timeout?, max_body_size? })` | Generic HTTP request with optional proxy/timeout/max_body_size. Returns `(res, err)`. |
| `http_multipart(url, fields, [headers])` | Multipart file uploads. Returns `(res, err)` — two-return pattern. |

### Storage

| Function | Description |
|----------|-------------|
| `global_store_set(key, value)` | Set a global key-value pair |
| `global_store_get(key)` | Get a global value |
| `global_store_incr(key, default, delta)` | Atomically increment a counter |
| `global_store_delete(key)` | Delete a global key |

> The server shares the same global storage as scraper jobs. Use this to serve data that jobs have collected.

### Database (SQLite variant only)

| Function | Description |
|----------|-------------|
| `db_query(sql, [params])` | Execute a SELECT query |
| `db_exec(sql, [params])` | Execute INSERT/UPDATE/DELETE |

### Utilities

| Function | Description |
|----------|-------------|
| `json_encode(value)` | Encode Lua value to JSON string |
| `json_decode(string)` | Decode JSON string to Lua value, returns (value, err) (10MB input limit) |
| `env_get(key)` | Read an environment variable |
| `defer(fn)` | Register an async cleanup function (runs after response) |
| `sleep(ms)` | Sleep for N milliseconds |
| `notify(title, body, [timeout])` | Send desktop notification |

### Defer (Async Cleanup)

Register **fully async** cleanup functions that run after the response is sent. Unlike `defer()` in pipeline hooks (which is synchronous and errors on async calls), the server's `defer()` supports async bindings

```lua
function get:resource()
    defer(function()
        sleep(1000)
        log("request finished for " .. self.path)
        http_post("https://hooks.example.com/callback", json_encode({ status = "done" }), {
            ["Content-Type"] = "application/json"
        })
    end)

    return { status = 200, body = { message = "ok" } }
end
```

Deferred functions run in reverse order (last registered runs first). Each function in the queue runs sequentially in the same Lua VM, so they share globals and can safely access `self` via closure capture. Errors in a deferred function are logged but don't affect other deferred functions.

## Authentication

The API server reuses SpyWeb's existing authentication. If `SPYWEB_API_KEY` is set, all `/api/*` routes (including `/api/v/*`) require the `X-SpyWeb-Key` header:

```bash
curl -H "X-SpyWeb-Key: your_key" http://127.0.0.1:7979/api/v/hello
```

Without the header, the server returns 401 Unauthorized.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SPYWEB_PORT` | `7979` | Server port |
| `SPYWEB_API_KEY` | *none* | API authentication key |

## Data Directory

The server's `fs_*` operations are scoped to the `server/` directory. Use `shared/` prefix to read/write from the project root's shared folder.

```
project/
├── server/
│   ├── init.lua          # Your route definitions
│   ├── server.log        # log() output
│   ├── error.log         # Error logs (auto-created)
│   └── data/             # Your persistent data
│       ├── tokens.json
│       └── cache.json
├── shared/               # Cross-VM shared data (read/write via shared/ prefix)
├── jobs.toml
├── data                  # Scraper database (SQLite file)
└── ui/                   # Dashboard files
```

## Examples

### Robust JSON API

`json_decode` returns `(value, nil)` on success and `(nil, error)` on failure, so you can handle malformed JSON gracefully without `pcall`.

```lua
function post:items()
    local data, err = json_decode(self.body or "")
    if not data then
        return { status = 400, body = { error = "Invalid JSON payload: " .. err } }
    end

    if not data.name then
        return { status = 400, body = { error = "Missing 'name' field" } }
    end

    -- Process data...
    return { status = 201, body = { message = "Created", item = data } }
end
```

### Serving Binary Data (Images)

You can serve images or other assets directly from the `server/` or `shared/` directories.

```lua
function get:logo()
    local data = fs_read_binary("logo.png")
    if not data then
        return { status = 404, body = "Not Found" }
    end

    return {
        status = 200,
        body = data,
        headers = { ["Content-Type"] = "image/png" }
    }
end
```

### Proxy to External API

```lua
function get:weather()
    local city = self.query.city or "London"
    local res = http_get("https://api.weather.com/v1/" .. city)
    return { status = res.status, body = res.body, headers = { ["Content-Type"] = "application/json" } }
end

function post:webhook()
    local payload = json_decode(self.body)
    http_post("https://hooks.example.com/relay", json_encode(payload), {
        ["Content-Type"] = "application/json"
    })
    return { status = 200, body = "Relayed" }
end
```

### Serve Scraped Data

```lua
function get:latest_prices()
    local prices = global_store_get("latest_prices")
    if not prices then
        return { status = 404, body = "No data yet" }
    end
    return { status = 200, body = json_decode(prices) }
end
```

### Health Check

```lua
function all:health()
    return { status = 200, body = "ok" }
end
```

## Troubleshooting

### All endpoints return 500

Check `server/error.log` for Lua syntax or runtime errors. Common causes:
- Syntax error in `init.lua`
- Calling a function that doesn't exist

### Endpoints return 404

- Check the handler is defined in the correct method table (`get`, `post`, etc.)
- Check the handler name matches the URL segment exactly
- The `all` table is only checked as a fallback after the method-specific table

### Body is empty or truncated

- Request body is capped at 10MB
- Only string, table, number, and boolean body types are supported in responses

### Timeout errors

- Luau only: 30-second timeout per request
- Check for infinite loops in your handler code
- Move heavy computation to background tasks using `defer`
