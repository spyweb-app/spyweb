# ⚙️ Execution Lifecycle & Orchestration

SpyWeb provides two mechanisms to manage execution lifecycles. One is for local, synchronous resource cleanup, and the other is a powerful, asynchronous orchestration layer for post-cycle handlers.

---

## 1. Hook-Scoped Cleanup: `defer(fn)`

`defer(fn)` is a global function available inside any hook. It registers a callback to be executed **immediately after the current hook finishes**, regardless of whether it returned successfully or raised an error.

### Key Characteristics
- **Scope:** Hook-Level Scoping. It is tied to the top-level hook (like `before_fetch`), not the local Lua function.
- **Execution:** **LIFO order** (Last-In, First-Called).
- **Isolation:** Each hook stage has its own isolated defer queue.
- **Error Safety:** If a deferred function errors, it is logged and the remaining queue still fires.

### 💡 Hook-Level Scoping vs. Function Scoping
Unlike `defer` in Go, which is scoped to the surrounding function, SpyWeb's `defer` is scoped to the **Pipeline Hook**.

If you call `defer()` inside a helper function, the cleanup **will not run** when that helper returns. Instead, it will wait until the entire Hook stage (e.g., `before_fetch`) is finished.

```lua
local function my_helper()
    defer(function() print("2. Cleanup runs LAST") end)
    print("1. Helper finished")
end

function before_fetch(request)
    my_helper()
    print("1.5 Hook still running")
    return request
end
-- Output:
-- 1. Helper finished
-- 1.5 Hook still running
-- 2. Cleanup runs LAST
```

### ⚠️ The Async Trap
`defer()` callbacks are **synchronous**. 
- **Rule:** Do NOT use async bindings (like `http_post`, `cdp.launch`, `page:open`) inside a `defer()` callback.
- **Behavior:** If you call an async function inside `defer()`, Lua will initiate it but SpyWeb will **not** wait for it to complete.
- **Exceptions:** Resource close methods (`browser:close()`, `context:close()`, and `page:close()`) are synchronous (they perform their network calls as background tasks) and are **fully safe** to use inside `defer()`.
- **Solution:** For async orchestration and cross-hook lifecycle logic (like network requests), use `defer.lua` (see below).

### Example: Safely Closing a Browser
```lua
function override_fetch(request)
    local browser = cdp.launch({ headless = true })
    
    -- Always kills the browser after hook exits, preventing zombie processes
    defer(function() browser:close() end) 
    
    local page = browser:attach()
    local ok, err = page:open(request.url)
    
    if not ok then
        -- browser:close() safely fires before returning
        return { error = "Navigation failed: " .. tostring(err) }
    end

    local found, wait_err = page:wait_for_selector(".dynamic-content", 10000)
    if not found then
        -- browser:close() safely fires before returning
        return { error = "Selector timeout: " .. tostring(wait_err) }
    end
    
    return { status = 200, body = page:content() }
end
```

---

## 2. Cycle-Scoped Orchestration: `defer.lua`

`defer.lua` is an optional file placed in your job directory (alongside `hooks.lua`). While `defer()` is for local janitorial work, `defer.lua` is a powerful **orchestration layer** designed for logic that requires a "whole cycle is done" guarantee.

### Good Uses for `defer.lua`
- External HTTP calls and Webhooks
- Telemetry flushing and metrics aggregation
- Alerting and failure notifications
- Releasing resources shared across hooks
- Persistence, snapshots, or writing audit logs

### Lifecycle Hooks
| Function | When it fires | Use Case |
|----------|---------------|----------|
| `on_success()` | Cycle completed without any fatal errors | Post-run telemetry, aggregate success APIs |
| `on_error(err)` | Cycle failed with a fatal error | Critical failure alerts (Discord/Slack/PagerDuty) |
| `on_finally()` | **Always** fires at the end of every cycle | State reset, writing audit logs |

### Shared State & Async Support
`defer.lua` shares the **same Lua VM** as `hooks.lua`. You can pass data between them simply by using standard **Lua globals** (variables declared without the `local` keyword). Furthermore, unlike the `defer()` binding, all functions in `defer.lua` **fully support async bindings** (like `http_post`).

```lua
-- hooks.lua
function after_extract(items)
    items_found = #items
    return items
end

-- defer.lua
function on_finally()
    local count = items_found or 0
    
    -- Async network calls are fully supported here
    -- Use http_post/http_get for simple calls, or http_request for full control
    http_post("https://metrics.example.com/push", '{"items": ' .. count .. '}', {
        ["Content-Type"] = "application/json"
    })
    
    -- Reset state for the next cycle
    items_found = nil
end
```

---

## Which one should I use?

| Goal | Mechanism |
|------|-----------|
| Close a browser/page opened in a single hook | `defer()` |
| Push aggregate metrics after a cycle finishes | `defer.lua` + `on_success` / `on_finally` |
| Alert on job failure via HTTP API | `defer.lua` + `on_error` |
| Clean up shared state at the end of a run | `defer.lua` + `on_finally` |
| Handle nested resources in a specific order | `defer()` multiple times (LIFO) |

## Memory Hygiene
SpyWeb automatically wipes transient globals like `last_fetch` and `selector_matches` after `on_finally` returns to keep the memory footprint low during long sleep intervals.
