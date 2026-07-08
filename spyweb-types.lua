---@meta spyweb

--=============================================================================
-- Request context (passed as `self` in `:` handlers, accessible via closure
-- capture in server `defer()`)
--=============================================================================

---@class spyweb_context
---@field shared table|nil              Per-cycle shared state (pipeline hooks)
---@field worker_id integer|nil         1-based worker ID (pipeline hooks)
---@field last_fetch table|nil          Fetch result envelope (pipeline hooks, after fetch)
---@field telemetry table|nil           Pipeline telemetry (pipeline hooks)
---@field body string|nil               Raw request body (API server)
---@field method string|nil             HTTP method (API server)
---@field path string|nil               Full request path (API server)
---@field path_args string[]|nil        Trailing URL segments (API server)
---@field query table<string,string>|nil  URL query parameters (API server)
---@field headers table<string,string>|nil  Request headers (API server)
---@field client_ip string|nil          Client IP address (API server)
---@field selector_matches integer|nil   Matched selector count (pipeline hooks, after extract)

--=============================================================================
-- HTTP response (returned by http_get, http_post, etc.)
--=============================================================================

---@class spyweb_http_response
---@field status  integer               HTTP status code
---@field body    string                Response body
---@field headers table<string,string>  Response headers
---@field url     string                Final URL after redirects
---@field time_ms integer               Request duration in milliseconds
---@field size    integer               Response body size in bytes
---@field proxy?  string                Proxy URL used (if any)

--=============================================================================
-- Handler response (returned by route handlers)
--=============================================================================

---@class spyweb_handler_response
---@field status?  integer                         HTTP status (100-599, default 200)
---@field body?    string|table|number|boolean|nil  Response body
---@field headers? table<string,string>             Custom response headers

--=============================================================================
-- Fetch result envelope (passed to after_fetch)
--=============================================================================

---@class spyweb_http_error
---@field error  string   Human-readable error message
---@field kind   string   Error kind: "dns" | "timeout" | "proxy" | "tls" | "connect" | "size" | "http" | "unknown"
---@field proxy? string   Proxy URL that failed (only on proxy errors)

---@class spyweb_fetch_result
---@field ok       boolean               Whether the request succeeded
---@field request  { url: string, headers: table<string,string>, proxy?: string }
---@field response spyweb_http_response|nil  HTTP response (present on both success and HTTP errors)
---@field error    { message: string, kind: string }|nil  Error info (nil on success)

--=============================================================================
-- Items (returned by extract stages)
--=============================================================================

---@class spyweb_item
---@field fields  table<string,string>  Item fields (must match job config)
---@field matches string[]              Matched keywords (read-only, populated by engine)

--=============================================================================
-- Helpers (injected from helpers.lua at startup)
--=============================================================================

---@param value any
---@param indent? integer
---@param seen? table
---@return string
function dump(value, indent, seen) end

---@generic T
---@param t T
---@return T
function copy(t) end

---@generic T
---@param t T
---@param seen? table
---@return T
function deep_copy(t, seen) end

--=============================================================================
-- Engine info (available in all contexts)
--=============================================================================

---@class spyweb_engine
---@field os           string  "linux" | "macos" | "windows" | "unknown"
---@field arch         string  "x86_64" | "aarch64" | "unknown"
---@field headless     boolean  True when no display server is available
---@field version      string  SpyWeb version (from Cargo.toml)
---@field lua_version  string  "luau" | "lua54"
---@field storage      string  "sqlite" | "redb"

---@type spyweb_engine
---@diagnostic disable-next-line: missing-fields
engine = {}

--=============================================================================
-- Async system functions
--=============================================================================

---Sleep for N milliseconds. Fully async (does not block other fibers).
---@param ms integer
function sleep(ms) end

---Send a desktop notification.
---@param title string
---@param body string
---@param timeout_ms? integer  Display duration in ms (default 5000)
function notify(title, body, timeout_ms) end

---Append a timestamped line to hooks.log (scraper) or server.log (API server).
---Only available when a job_dir is configured.
---@param msg string
function log(msg) end

---Append content to a file in the job directory. Falls back to shared/ for
---shared/ prefix. Only available when a job_dir is configured.
---@param path string    Relative path or shared/ prefixed path
---@param content string
function fs_append(path, content) end

---Overwrite a file in the job directory. Supports shared/ prefix.
---Only available when a job_dir is configured.
---@param path string
---@param content string
function fs_overwrite(path, content) end

---Read a text file from the job directory, falls back to shared/.
---Restricted to .csv, .json, .jsonl, .txt, .log extensions.
---Only available when a job_dir is configured.
---@param path string
---@return string|nil
function fs_read(path) end

---Read a binary file from the job directory, falls back to shared/.
---Supports images, fonts, PDFs, etc.
---Only available when a job_dir is configured.
---@param path string
---@return string|nil
function fs_read_binary(path) end

---Load and cache a Lua module from the job directory or project root.
---Only available with Luau when a job_dir is configured.
---@param name string  Module name (dots become path separators)
---@return any
function require(name) end

--=============================================================================
-- Sync system functions
--=============================================================================

---Encode a Lua value to a JSON string.
---@param value any
---@return string
function json_encode(value) end

---Decode a JSON string to a Lua value. 10MB input limit.
---Returns (value, nil) on success, (nil, error) on failure.
---@param string string
---@return any, string?
function json_decode(string) end

---Read an environment variable. Automatically prefixed with SPYWEB_ unless
---the key already starts with SPYWEB_.
---@param key string
---@return string|nil
function env_get(key) end

---Register a deferred (cleanup) function. In the scraper pipeline, deferred
---functions run after on_success/on_error/on_finally. In the API server,
---they run after the response is sent and support async bindings.
---@param fn function
function defer(fn) end

--=============================================================================
-- HTTP client (async)
--=============================================================================

---Send an HTTP GET request. 10MB body limit. 30s timeout.
---@param url string
---@param headers? table<string,string>
---@return spyweb_http_response?, spyweb_http_error?
function http_get(url, headers) end

---Send an HTTP POST request. 10MB body limit. 30s timeout.
---Content-Type defaults to application/x-www-form-urlencoded.
---@param url string
---@param body string
---@param headers? table<string,string>
---@return spyweb_http_response?, spyweb_http_error?
function http_post(url, body, headers) end

---Send a generic HTTP request.
---@param args { method?: string, url: string, body?: string, headers?: table<string,string>, proxy?: string, timeout?: number, max_body_size?: number }
---@return spyweb_http_response?, spyweb_http_error?
function http_request(args) end

---Send a multipart POST request for file uploads.
---@param url string
---@param fields table<string, string|{ content: string, filename?: string, type?: string }>
---@param headers? table<string,string>
---@return spyweb_http_response?, spyweb_http_error?
function http_multipart(url, fields, headers) end

---Probe a TLS endpoint for certificate info. Returns (table, nil) on success,
-- (nil, error) on failure.
---@param host string
---@param port? integer  Default 443
---@return { subject: string, issuer: string, serial: string, not_before: string, not_after: string, days_left: integer, fingerprint: string }|nil, string|nil
function tls_probe(host, port) end

--=============================================================================
-- Storage (key-value, persisted to the database)
--=============================================================================

---Set a job-scoped key-value pair. Key is prefixed with the job name.
---@param key string
---@param value string
function store_set(key, value) end

---Get a job-scoped value by key.
---@param key string
---@return string|nil
function store_get(key) end

---Delete a job-scoped key.
---@param key string
function store_delete(key) end

---Set a global (cross-job) key-value pair.
---@param key string
---@param value string
function global_store_set(key, value) end

---Get a global value by key.
---@param key string
---@return string|nil
function global_store_get(key) end

---Atomically increment a global counter. Returns the new value.
---@param key string
---@param default integer  Value if key does not exist
---@param delta integer    Amount to add
---@return integer
function global_store_incr(key, default, delta) end

---Delete a global key.
---@param key string
function global_store_delete(key) end

--=============================================================================
-- Database (SQLite variant only)
--=============================================================================

---Execute a SELECT query. Returns an array of row tables.
---@param sql string
---@param params? any[]  Positional query parameters
---@return table[]       Array of { column = value, ... }
function db_query(sql, params) end

---Execute an INSERT/UPDATE/DELETE. Returns number of rows changed.
---@param sql string
---@param params? any[]
---@return integer
function db_exec(sql, params) end

--=============================================================================
-- Testing
--=============================================================================

---@class spyweb_test
---@field assert_eq fun(left: any, right: any, msg?: string)
---@field assert_ne fun(left: any, right: any, msg?: string)

---@type spyweb_test
---@diagnostic disable-next-line: missing-fields
spyweb = {}

---Assert that two values are equal.
---@param left any
---@param right any
---@param msg? string
function spyweb.assert_eq(left, right, msg) end

---Assert that two values are not equal.
---@param left any
---@param right any
---@param msg? string
function spyweb.assert_ne(left, right, msg) end

--=============================================================================
-- CDP (Chrome DevTools Protocol) - browser automation
--=============================================================================

---@alias spyweb_event_predicate fun(params: table): boolean

---@class spyweb_page
---@field call fun(self: spyweb_page, method: string, params: table): table
---@field call_save fun(self: spyweb_page, method: string, params: table, path: string): table
---@field wait_event fun(self: spyweb_page, event: string, timeout_ms?: integer, predicate?: spyweb_event_predicate): table
---@field close fun(self: spyweb_page)
---@field open fun(self: spyweb_page, url: string, wait_until?: string|false, timeout_ms?: integer): boolean
---@field wait_for_selector fun(self: spyweb_page, selector: string, opts?: integer|{ timeout_ms?: integer, poll_ms?: integer, scroll?: boolean, visible?: boolean }): boolean
---@field wait_for_url fun(self: spyweb_page, pattern: string, timeout_ms?: integer): string|nil, string|nil
---@field wait_for_response fun(self: spyweb_page, predicate?: spyweb_event_predicate, timeout_ms?: integer): table|nil, string|nil
---@field wait_for_idle fun(self: spyweb_page, timeout_ms?: integer, quiet_ms?: integer): boolean|nil, string|nil
---@field scroll fun(self: spyweb_page, opts?: { max_scrolls?: integer, step?: integer, delay_ms?: integer, until_selector?: string, until_bottom?: boolean }): boolean|nil, string|nil
---@field content fun(self: spyweb_page): string
---@field evaluate fun(self: spyweb_page, js: string): any
---@field screenshot fun(self: spyweb_page, path: string, opts?: { format?: string, quality?: integer, full_page?: boolean }): string
---@field set_extra_headers fun(self: spyweb_page, headers: table<string,string>)
---@field set_user_agent fun(self: spyweb_page, user_agent: string, opts?: { accept_language?: string, platform?: string })
---@field cookies fun(self: spyweb_page, urls?: string[]): table[]
---@field set_cookies fun(self: spyweb_page, cookies: table[])
---@field block_resources fun(self: spyweb_page, types_or_patterns: string[]): string[]
---@field click fun(self: spyweb_page, selector: string, opts?: { real?: boolean }): boolean|nil, string|nil
---@field type fun(self: spyweb_page, selector: string, text: string, opts?: { real?: boolean }): boolean|nil, string|nil

---@class spyweb_browser_context
---@field id      string                                Unique context ID
---@field attach  fun(self: spyweb_browser_context, url?: string): spyweb_page
---@field close   fun(self: spyweb_browser_context)

---@class spyweb_browser
---@field call fun(self: spyweb_browser, method: string, params: table): table
---@field wait_event fun(self: spyweb_browser, event: string, timeout_ms?: integer, predicate?: fun(params: table): boolean): table
---@field close fun(self: spyweb_browser)
---@field get_user_data_dir fun(self: spyweb_browser): string|nil
---@field attach fun(self: spyweb_browser, opts?: { browserContextId?: string, url?: string, reuse?: boolean }): spyweb_page
---@field attach_session fun(self: spyweb_browser, target_id: string): table
---@field call_session fun(self: spyweb_browser, session_id: string, method: string, params: table): table
---@field wait_session_event fun(self: spyweb_browser, session_id: string, event: string, timeout_ms?: integer, predicate?: fun(params: table): boolean): table
---@field new_context fun(self: spyweb_browser): spyweb_browser_context

---@type table<string, fun(...):...>
cdp = {}

---Connect to an existing CDP browser over WebSocket.
---@param ws_url string  e.g. "ws://127.0.0.1:9222/..."
---@param headers? table<string,string>
---@return spyweb_browser
function cdp.connect(ws_url, headers) end

---Launch a new browser instance.
---@param opts { executable?: string, headless?: boolean, keep_alive?: boolean, user_data_dir?: string, args?: string[] }
---@return spyweb_browser
function cdp.launch(opts) end

---Find a browser executable on the system.
---@return string|nil
function cdp.get_browser() end

---Sleep for N milliseconds (CDP-specific variant).
---@param ms integer
function cdp.sleep(ms) end

--=============================================================================
-- Pipeline lifecycle hooks (define these as global functions in hooks.lua)
--=============================================================================

---@class spyweb_fetch_request
---@field url           string
---@field method        string
---@field headers       table<string,string>
---@field timeout?      number    Per-request timeout in seconds (default 30)
---@field proxy?        string    Per-request proxy URL
---@field max_body_size? integer   Max response body in MB (integer, default 10)

---Override the fetch request before it is sent. Return a modified request or
---nil to use the default. NOTE: returning nil for override_fetch is an error
---(not a skip).
---@param request spyweb_fetch_request
---@param ctx spyweb_context
---@return spyweb_fetch_request?
function before_fetch(request, ctx) end

---Completely replace the fetch stage. Must return a valid response table with
---`status` and `body` fields. Returning nil produces an error.
---@param request spyweb_fetch_request
---@param ctx spyweb_context
---@return spyweb_http_response?
function override_fetch(request, ctx) end

---Post-process the fetch result. Return nil to skip the rest of the pipeline.
---@param fetch_result spyweb_fetch_result
---@param ctx spyweb_context
---@return spyweb_http_response?
function after_fetch(fetch_result, ctx) end

---Completely replace the extraction stage. Return a table of items, or nil
---for empty items (after_extract still runs with an empty list).
---@param response spyweb_http_response
---@param ctx spyweb_context
---@return spyweb_item[]?
function override_extract(response, ctx) end

---Post-process extracted items. Return nil to skip filter, store, notify,
---and webhook stages.
---@param items spyweb_item[]
---@param ctx spyweb_context
---@return spyweb_item[]?
function after_extract(items, ctx) end

---Filter a single item. Return the item to keep it, or nil to drop it
---before it reaches the database. Mutually exclusive with keyword filter.
---@param item spyweb_item
---@param ctx spyweb_context
---@return spyweb_item|nil
function filter_item(item, ctx) end

---Last chance before DB insert. Return nil to skip storing and notifying.
---@param items spyweb_item[]
---@param ctx spyweb_context
---@return spyweb_item[]|nil
function before_store(items, ctx) end

---Called before sending notifications. Return nil or false to skip notify.
---@param items spyweb_item[]
---@param ctx spyweb_context
---@return spyweb_item[]|nil
function before_notify(items, ctx) end

---Called before sending the webhook. Return nil or false to skip webhook.
---@param payload { job_name: string, item_count: number, items: spyweb_item[] }
---@param ctx spyweb_context
---@return table|nil
function before_webhook(payload, ctx) end

---Called when a cycle completes successfully (all stored items posted).
---@param ctx spyweb_context
function on_success(ctx) end

---Called when a cycle encounters an error.
---@param err string
---@param ctx spyweb_context
function on_error(err, ctx) end

---Called after on_success or on_error, always fires.
---@param ctx spyweb_context
function on_finally(ctx) end

---Called when the job is finished (all cycles complete).
function on_finished() end

--=============================================================================
-- Server API route tables (injected by the API server)
--=============================================================================

---@type table<string, fun(self: spyweb_context): spyweb_handler_response|string|nil>
get = {}

---@type table<string, fun(self: spyweb_context): spyweb_handler_response|string|nil>
post = {}

---@type table<string, fun(self: spyweb_context): spyweb_handler_response|string|nil>
put = {}

---@type table<string, fun(self: spyweb_context): spyweb_handler_response|string|nil>
patch = {}

---@type table<string, fun(self: spyweb_context): spyweb_handler_response|string|nil>
delete = {}

---@type table<string, fun(self: spyweb_context): spyweb_handler_response|string|nil>
all = {}
