# Chrome DevTools Protocol (CDP) Documentation

## Overview

**Spyweb does not bundle a browser.** Unlike tools that download a 300MB+ Chromium binary you don't need, Spyweb uses whatever browser you already have installed to render JavaScript-heavy pages. You don't download anything extra. You don't configure anything special. It just works.

If you need JS rendering in a hook (to scrape a React site, bypass a Cloudflare challenge, click buttons, wait for DOM elements), you use the `cdp` module. That's it.

For detailed documentation with examples, see [SpyWeb CDP Docs](https://docs.spyweb.app/cdp.html).

### How It Works

Spyweb can get a browser two ways:

1. **Launch one** — `cdp.launch()` finds Chrome, Edge, or Brave on your system, starts it in headless mode, and connects to it automatically. Zero config. You can also pass `{ executable = "/path/to/browser" }` to use a specific binary.
2. **Connect to an existing one** — `cdp.connect("ws://...")` attaches to any browser or CDP-compatible server already running.

Once connected, you call `browser:attach()` to get a page/tab, then drive it with simple commands like `page:open(url)`, `page:wait_for_selector()`, `page:click()`, and `page:content()` to get the rendered HTML.

### Under the Hood

When you call `cdp.launch()`, Spyweb spawns the browser as a child process with `--remote-debugging-port=0` (which picks a random free port). It reads the DevTools WebSocket URL from the browser's stderr output, connects to it, and gives you a `Browser` object. Every command and response is sent as JSON-RPC messages over WebSocket — the same protocol Chrome's own DevTools uses.

Each page/tab gets its own dedicated WebSocket connection. This is why `browser:attach()` doesn't just return a reference — it actually opens a new WebSocket to the page's specific endpoint.

By default, closing the browser kills the process and cleans up the profile. You can override this with `keep_alive = true` to keep the browser warm across multiple hook calls.

---

## Hook Integration

CDP is available globally via the `cdp` table. While it can be used in any hook, it is most commonly used in `override_fetch` to replace the default HTTP client.

### Basic Usage in `override_fetch`

To use CDP for a job, define an `override_fetch` function in your `hooks.lua`:

```lua
function override_fetch(request)
    -- 1. Launch a temporary browser
    local browser = cdp.launch({})
    
    -- 2. Attach to a page (tab)
    local page = browser:attach()
    
    -- 3. Navigate safely and check for errors
    local ok, err = page:open(request.url)
    if not ok then
        browser:close()
        return { error = "Navigation failed: " .. tostring(err) }
    end

    -- 4. Wait for content before extraction
    local found, wait_err = page:wait_for_selector(".dynamic-content", 10000)
    if not found then
        browser:close()
        return { error = "Selector timeout: " .. tostring(wait_err) }
    end
    
    -- 5. Get the rendered HTML and clean up
    local html = page:content()
    browser:close()
    
    -- 6. Pass the html back to the pipeline (triggers extraction or catches in after_fetch)
    return {
        status = 200,
        body = html,
        url = request.url
    }
end
```

## Browser Persistence & Performance

Launching a new browser for every fetch is slow. Because Spyweb **persists the Lua state** for each job, you can keep a browser instance "warm" across multiple scraper runs.

### Persistent Browser Pattern

Store the browser in a global variable (without `local`) to keep it alive between iterations.

```lua
-- This runs once when the job is loaded
if not browser then
    print("[CDP] Launching persistent browser...")
    browser = cdp.launch({ 
        headless = true,
        keep_alive = true -- Prevents the process from being killed when the variable is GC'd
    })
end

function override_fetch(request)
    -- Attach to a page (reuses blank tabs by default)
    local page = browser:attach()
    
    -- Safety: Check if navigation succeeded
    local ok, err = page:open(request.url)
    if not ok then
        page:close()
        return { error = "Failed to load " .. request.url }
    end
    
    -- Ensure content is ready
    if page:wait_for_selector(".item", 5000) then
        local html = page:content()
        page:close() -- Closes the tab, but the browser stays open
        
        -- Pass the html back to the pipeline (triggers extraction or catches in after_fetch)
        return {
            status = 200,
            body = html,
            url = request.url
        }
    else
        page:close()
        return { error = "Content timeout" }
    end
end
```

---

## Core API Reference (`cdp` table)

### `cdp.launch(options)`
- `options`: (Table, **required**)
    - `executable`: (String) Manual path to browser binary. Skips auto-detection if provided.
    - `headless`: (Boolean) Default `true`.
    - `user_data_dir`: (String) Path to profile. Defaults to `~/.spyweb/<job-folder-name>`.
    - `args`: (Table) List of extra flags like `{"--proxy-server=..."}`.
    - `keep_alive`: (Boolean) If `true`, the browser process persists until Spyweb exits. Default `false`.

### `cdp.connect(ws_url, [headers])`
Connects to an existing browser via its DevTools WebSocket URL.
- `ws_url`: (String, **required**) The WebSocket URL.
- `headers`: (Table, **optional**) Custom HTTP headers for the connection (e.g., for Authorization).

**Example (Cloudflare Browser Rendering)**:
```lua
local ws_url = "wss://api.cloudflare.com/client/v4/accounts/<ID>/browser-rendering/devtools/browser"
local browser = cdp.connect(ws_url, {
    ["Authorization"] = "Bearer <TOKEN>"
})
```

### `cdp.get_browser()`
Returns the path to the auto-detected browser executable.

### `cdp.sleep(ms)`
Asynchronous sleep function for use within hooks.

---

## Specialized Browsers

While standard Chromium-based browsers are the most compatible, they are resource-intensive. A single Chrome instance can consume hundreds of megabytes of RAM, and its profile directory (`user_data_dir`) often balloons to 150MB+ immediately upon launch. 

For a lighter approach, consider these specialized alternatives designed for scraping and automation. They offer a much smaller footprint while speaking the same CDP protocol.

### [Lightpanda](https://lightpanda.io)
A high-performance, lightweight browser written in Zig, designed for speed and low resource usage.

1. **Launch the server**:
   ```bash
   lightpanda serve --port 9222
   ```
2. **Connect in Lua**:
   ```lua
   local browser = cdp.connect("ws://127.0.0.1:9222")
   ```

### [Obscura](https://github.com/h4ckf0r0day/obscura)
A headless browser specifically built for AI agents and advanced anti-detection.

1. **Launch the server**:
   ```bash
   obscura serve --port 9222 --stealth
   ```
2. **Connect in Lua**:
   ```lua
   local browser = cdp.connect("ws://127.0.0.1:9222/devtools/browser")
   ```

### Compatibility Note

These browsers implement CDP but may not support every method. Based on our assessment, here is what to expect:

| Helper | Lightpanda | Obscura |
|--------|-----------|---------|
| `page:screenshot()` | Returns a fake placeholder image | ❌ Not supported |
| `page:block_resources()` | ❌ Not supported | ❌ Not supported |
| `page:cookies()` (no args) | ✅ Works | ❌ Use `page:cookies({url})` instead |
| `page:type(text, {real=true})` | ✅ Works | ❌ Not supported |

There may be other gaps we haven't caught. Test your hooks thoroughly and fall back to a standard browser if something doesn't work.

---

## Browser API Reference (Core)

These methods are provided directly by the Spyweb Core on the `Browser` object.

- `browser:attach([opts])`: (Async) Returns a `Page` object.
    - `opts.url`: Initial URL (default: `about:blank`).
    - `opts.reuse`: Reuses a blank tab if `true` (default: `true` for default context).
    - `opts.browserContextId`: Attaches to a specific isolated context.
- `browser:new_context()`: (Async) Creates an isolated session. Returns a `Context` object.
- `browser:close()`: Closes the browser and kills the process.
- `browser:get_user_data_dir()`: Returns the profile path.
- `browser:call(method, params)`: (Async) Raw browser-level CDP command.
- `browser:wait_event(event, [timeout_ms], [predicate])`: (Async) Wait for browser-level notification.
- `browser:attach_session(target_id)`: (Async) Returns a session ID.
- `browser:call_session(session_id, method, params)`: (Async) Call method on a specific session.
- `browser:wait_session_event(session_id, event, [opts])`: (Async) Wait for session-level events.

---

## Context API Reference (Core)

Returned by `browser:new_context()`.

- `context.id`: The unique string ID of the context.
- `context:attach([url])`: (Async) Creates a new page within this isolated context.
- `context:close()`: (Async) Closes the context and all associated pages.

---

## Page API Reference

The `Page` object combines native transport methods with high-level Lua helpers.

### Native Methods (Core)
- `page:call(method, params)`: (Async) Raw page-level CDP command.
- `page:call_save(method, params, path)`: (Async) Optimized binary call. Saves the `data` field of the response to `path` and returns the JSON without the massive data string.
- `page:wait_event(event, ...)`: (Async) Wait for a page notification. Accepts variadic args: timeout (number), predicate (function), or a table with `timeout_ms`/`timeout` and `predicate`.
- `page:close()`: (Async) Closes the specific tab.

### High-Level Helpers (Lua)
Injected via `cdp.lua` to provide a higher-level CDP abstraction.

- `page:open(url, [wait_until], [timeout_ms])`: Returns `true` or `nil, error`.
- `page:wait_for_selector(selector, [opts])`: Returns `true, element_info` or `nil, error`. `opts` can be timeout or `{timeout, poll_ms, visible, scroll}`.
- `page:wait_for_idle([timeout_ms], [quiet_ms])`: Returns `true` or `nil, error`.
- `page:evaluate(js)`: Returns the result of the JS expression.
- `page:click(selector, [opts])`: `opts.real = true` uses hardware mouse events.
- `page:type(selector, text, [opts])`: `opts.real = true` uses hardware keyboard events.
- `page:content()`: Returns the full rendered HTML.
- `page:screenshot(path, [opts])`: Saves a screenshot. Supports `{format = "png"|"jpeg", quality = 1..100, full_page = true, fullPage = true, fromSurface = true}`. Defaults to PNG.
- `page:block_resources(types)`: e.g., `{"image", "font", "media"}`.
- `page:wait_for_url(pattern, timeout_ms)`: Polls `location.href` until it matches the string `pattern`. Returns the URL or `nil, error`.
- `page:wait_for_response([predicate], [timeout_ms])`: Waits for `Network.responseReceived`. Optional `predicate` function receives params, returns `true` when match found.
- `page:scroll([opts])`: Scrolls the page. `opts` can include `max_scrolls` (default 20), `step`, `delay_ms` (default 250), `until_selector`, `until_bottom` (default `true`).
- `page:set_extra_headers(headers)`: Sets extra HTTP headers via `Network.setExtraHTTPHeaders`.
- `page:set_user_agent(user_agent, [opts])`: Overrides the User-Agent. `opts` can include `accept_language` and `platform`.
- `page:cookies([urls])`: Returns all cookies, or filtered by URL if `urls` is provided.
- `page:set_cookies(cookies)`: Sets cookies via `Network.setCookies`.

---

## Advanced Pattern: Hybrid Human-in-the-Loop

For sites with aggressive bot detection (Cloudflare, CAPTCHAs, etc.), you can implement a "Hybrid Recovery" flow. This involves switching from a fast headless browser (like Lightpanda) to a visible Chrome window when a block is detected.

### The Strategy:
1.  **Normal Mode**: Scrape headlessly for maximum speed.
2.  **Detection**: In `override_fetch`, check for a "block" selector (e.g., `#captcha-container`).
3.  **Transition**: Close the headless browser and launch a visible Chrome instance using the **same** `user_data_dir`.
4.  **Notification**: Use the `notify()` function to alert a human operator.
5.  **Intervention**: Wait in a `while` loop, polling for a "success" selector that appears after the human solves the puzzle.
6.  **Handback**: Once solved, capture the HTML, close the visual browser, and return to headless mode.

See [examples/hybrid-recovery/hooks.lua](../examples/hybrid-recovery/hooks.lua) for a complete, production-ready implementation of this pattern.
