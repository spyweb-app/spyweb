---@meta spyweb

--=============================================================================
-- Request context (passed as `self` in `:` handlers, accessible via closure
-- capture in server `defer()`)
--=============================================================================

---@class spyweb_context
---@field body string|nil    Raw request body (nil for GET/HEAD)
---@field method string      HTTP method ("GET", "POST", etc.)
---@field path string        Full request path (e.g. "/api/v/users/123")
---@field path_args string[]  Trailing URL segments (e.g. {"123"})
---@field query table<string,string>  URL query parameters
---@field headers table<string,string>  Request headers (lowercase keys)
---@field client_ip string   Client IP address
---@field shared table|nil   Job-scoped shared state (pipeline hooks only)

--=============================================================================
-- HTTP response (returned by http_get, http_post, etc.)
--=============================================================================

---@class spyweb_http_response
---@field status integer                HTTP status code
---@field body string                   Response body
---@field headers table<string,string>  Response headers

--=============================================================================
-- Handler response (returned by route handlers)
--=============================================================================

---@class spyweb_handler_response
---@field status?  integer                         HTTP status (100-599, default 200)
---@field body?    string|table|number|boolean|nil  Response body
---@field headers? table<string,string>             Custom response headers

--=============================================================================
-- Items (returned by extract stages)
--=============================================================================

---@class spyweb_item
---@field content string
---@field [string] any

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
---@param string string
---@return any
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
---@return spyweb_http_response
function http_get(url, headers) end

---Send an HTTP POST request. 10MB body limit. 30s timeout.
---Content-Type defaults to application/x-www-form-urlencoded.
---@param url string
---@param body string
---@param headers? table<string,string>
---@return spyweb_http_response
function http_post(url, body, headers) end

---Send a generic HTTP request.
---@param args { method?: string, url: string, body?: string, headers?: table<string,string> }
---@return spyweb_http_response
function http_request(args) end

---Send a multipart POST request for file uploads.
---@param url string
---@param fields table<string, string|{ content: string, filename?: string, type?: string }>
---@param headers? table<string,string>
---@return spyweb_http_response
function http_multipart(url, fields, headers) end

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

---@type { assert_eq: fun(left: any, right: any, msg?: string), assert_ne: fun(left: any, right: any, msg?: string) }
spyweb = {}

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
---@field attach fun(self: spyweb_browser_context, url?: string): spyweb_page
---@field close fun(self: spyweb_browser_context)

---@class spyweb_browser
---@field call fun(self: spyweb_browser, method: string, params: table): table
---@field wait_event fun(self: spyweb_browser, event: string, timeout_ms?: integer, predicate?: fun(params: table): boolean): table
---@field close fun(self: spyweb_browser)
---@field get_user_data_dir fun(self: spyweb_browser): string|nil
---@field attach fun(self: spyweb_browser, opts?: { browserContextId?: string, url?: string, reuse?: boolean }): spyweb_page
---@field attach_session fun(self: spyweb_browser, target_id: string): table
---@field call_session fun(self: spyweb_browser, session_id: string, method: string, params: table): table
---@field wait_session_event fun(self: spyweb_browser, session_id: string, event: string, timeout_ms?: integer, predicate?: fun(params: table): boolean): table
---@field create_context fun(self: spyweb_browser): spyweb_browser_context

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

---Override the fetch request before it is sent. Return a modified request or
---nil to use the default. NOTE: returning nil for override_fetch is an error
---(not a skip).
---@param request { url: string, headers: table<string,string>, method?: string, body?: string }
---@param ctx spyweb_context
---@return { url: string, headers: table<string,string>, method?: string, body?: string }?
function before_fetch(request, ctx) end

---Completely replace the fetch stage. Must return a valid response table with
---`status` and `body` fields. Returning nil produces an error.
---@param request { url: string, headers: table<string,string>, method?: string, body?: string }
---@param ctx spyweb_context
---@return spyweb_http_response?
function override_fetch(request, ctx) end

---Post-process the response from the fetch stage. Return nil to skip the
---rest of the pipeline.
---@param response spyweb_http_response
---@param ctx spyweb_context
---@return spyweb_http_response?
function after_fetch(response, ctx) end

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

---Filter a single item. Return false to drop the item, true to keep it,
---or nil to continue without filtering.
---@param item spyweb_item
---@param ctx spyweb_context
---@return boolean?
function filter_item(item, ctx) end

---Transform or filter items before storage. Must return items.
---@param items spyweb_item[]
---@param ctx spyweb_context
---@return spyweb_item[]
function before_store(items, ctx) end

---Called before sending notifications. Return nil or false to skip notify.
---@param items spyweb_item[]
---@param ctx spyweb_context
---@return boolean|nil
function before_notify(items, ctx) end

---Called before sending the webhook. Return nil or false to skip webhook.
---@param payload table
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
