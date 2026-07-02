# ⚙️ Execution Lifecycle & Orchestration

## Pipeline Stages

Every scraping cycle runs through these stages in order. "Automatic" stages are built-in; the rest can be customized via Lua hooks in `hooks.lua`.

| Stage | Type | nil returns | Receives |
|---|---|---|---|
| `before_fetch` | hook | skip cycle | `request, ctx` |
| `override_fetch` | hook | error (nil is not a valid return) | `request, ctx` |
| *fetch* | automatic | — | — |
| `after_fetch` | hook | skip cycle | `result, ctx` |
| `override_extract` | hook | nil/empty items - skip after `after_extract` | `response, ctx` |
| *extract* | automatic | — | — |
| `after_extract` | hook | nil/empty items -  skip remaining pipeline | `items, ctx` |
| `filter_item` | hook (per-item) | skip item | `item, ctx` |
| `before_store` | hook | skip store | `items, ctx` |
| *dedup + insert* | automatic | — | — |
| `before_notify` | hook | skip notify | `items, ctx` |
| `before_webhook` | hook | skip webhook | `payload, ctx` |
| *notify + webhook* | automatic | — | — |

---
After the pipeline stages finish (or exit via nil), the following fire from `defer.lua`:
| Hook | Fires | Receives |
|---|---|---|
| `on_success(ctx)` | cycle completed with no fatal errors | `ctx` |
| `on_error(err, ctx)` | cycle failed with a fatal error | `err, ctx` |
| `on_finally(ctx)` | always after on_success or on_error | `ctx` |

---
After all workers complete their cycles, this fires from `hooks.lua` or `defer.lua`:
| Hook | Fires |
|---|---|
| `on_finished()` | once per job iteration, before sleep |

### Flow (clockwise)

```
    before_fetch ──→ override_fetch ──→ fetch ──→ after_fetch
        ▲            nil→fetch fail       auto       nil→skip
        │                                              │
        │                                              ▼
   sleep(interval)                             override_extract
        │                                        nil→empty
        ▲                                              │
        │                                              ▼
    on_finished                                     extract
        │                                            auto
        ▲                                              │
        │                                              ▼
      on_finally                                  after_extract
         │                                        nil→skip rst
        ▲                                              │
        │                                              ▼
   on_success/error                               filter_item
        │                                         nil→skip item
        ▲                                              │
        │                                              ▼
    notify+webhook                               before_store
        │                                        nil→skip s/n
        ▲                                              │
        │                                              ▼
   before_webhook  nil→skip                      dedup+insert
        │                                            auto
        ▲                                              │
        │                                              │
   before_notify ←─────────────────────────────────────┘
   nil→skip notif
```

All lifecycle hooks (`on_success`/`on_error` → `on_finally` → `on_finished` → `sleep`) fire every cycle regardless of nil skips. `sleep` then loops back to `before_fetch`.

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

function before_fetch(request, ctx)
    my_helper()
    print("1.5 Hook still running")
    return request
end
-- Output:
-- 1. Helper finished
-- 1.5 Hook still running
-- 2. Cleanup runs LAST
```

### Defer is Synchronous

`defer()` callbacks run synchronously. Async bindings (like `http_post`, `sleep`, `cdp.launch`, `page:open`) cannot be called inside them - Luau will error with `"attempt to yield across metamethod/C-call boundary"` at runtime. The error is logged but does not crash the job; remaining deferred functions in the queue still fire.

Only resource close methods (`browser:close()`, `context:close()`, `page:close()`) are safe here - they perform network calls as background tasks internally.

For async orchestration and cross-hook lifecycle logic, use `defer.lua` instead (see below).

### Example: Safely Closing a Browser
```lua
function override_fetch(request, ctx)
    local browser = cdp.launch({ headless = true })
    
    -- Always kills the browser after hook exits
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
| `on_success(ctx)` | Cycle completed without any fatal errors | Post-run telemetry, aggregate success APIs |
| `on_error(err, ctx)` | Cycle failed with a fatal error | Critical failure alerts (Discord/Slack/PagerDuty) |
| `on_finally(ctx)` | **Always** fires at the end of every cycle | State reset, writing audit logs |

### Shared State & Async Support
`defer.lua` shares the **same Lua VM** as `hooks.lua`. Every lifecycle hook receives the same per-cycle `ctx` as the pipeline hooks, so you can pass data between them through `ctx.shared`. Unlike the `defer()` binding, all functions in `defer.lua` **fully support async bindings** (like `http_post`).

```lua
-- hooks.lua
function after_extract(items, ctx)
    ctx.shared.item_count = #items
    return items
end

-- defer.lua
function on_finally(ctx)
    local count = ctx.shared.item_count or 0

    local payload = json_encode({ items = count })
    http_post("https://metrics.example.com/push", payload, {
        ["Content-Type"] = "application/json"
    })
end
```

Standard **Lua globals** are also visible across both files, but `ctx.shared` is automatically cleaned up at cycle end - no need to manually reset state.

---

## 3. Job-Loop Hook: `on_finished()` (multi-worker setup)

> **Single-worker jobs:** If `workers = 1`, `on_finished` fires immediately after `on_finally` with no workers in between. There's no meaningful gap, use `on_finally` in `defer.lua` instead.

`on_finished()` is a global function defined in `hooks.lua` or `defer.lua` that fires **after every complete job iteration** - after all workers finish their cycles, just before the job goes to sleep until the next interval.

### Key Differences from `on_finally()`

| Aspect | `on_finished()` | `on_finally(ctx)` |
|--------|-----------------|-------------------|
| Scope | Across all workers in a job iteration | Per-cycle (per-worker) |
| When it fires | After all workers complete, before sleep | After each individual cycle ends |
| `ctx` parameter | **None** - receives no arguments | Receives per-cycle `ctx` |
| Where defined | `hooks.lua` or `defer.lua` | `defer.lua` |

### Example

```lua
function on_finished()
    log("Job iteration complete, sleeping until next interval")
end
```

### Use Cases
- Logging job-level completion metrics
- Releasing global resources shared across workers
- Running cleanup that should happen after all workers finish, not after each one

---

### `ctx.worker_id`

When multiple workers are configured, each worker receives its own 1-based `worker_id` via the cycle context:

```lua
function before_fetch(request, ctx)
    log("Worker " .. ctx.worker_id .. " processing " .. request.url)
    return request
end
```

- `worker_id` ranges from `1` to `workers`.
- It is a **read-only** reserved field - attempting to write `ctx.worker_id = ...` raises an error.
- For single-worker jobs (`workers = 1`), `ctx.worker_id` is always `1`.

---

## Which one should I use?

| Goal | Mechanism |
|------|-----------|
| Close a browser/page opened in a single hook | `defer()` |
| Push aggregate metrics after a cycle finishes | `defer.lua` + `on_success` / `on_finally` |
| Alert on job failure via HTTP API | `defer.lua` + `on_error` |
| Clean up shared state at the end of a run | `defer.lua` + `on_finally` |
| Handle nested resources in a specific order | `defer()` multiple times (LIFO) |
| Log/track job-level completion across all workers | `on_finished()` in `hooks.lua` |

## Memory Hygiene

SpyWeb automatically wipes transient cycle state like `last_fetch` and `selector_matches` from the per-cycle context after `on_finally` returns to keep the memory footprint low during sleep intervals. Job-level state persists across cycles and is available in `on_finished()`.
