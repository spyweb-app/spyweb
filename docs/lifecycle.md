# ⚙️ Execution Lifecycle & Orchestration

> Global once-per-process setup belongs in the optional root `startup.lua`. It runs after the database opens and before job hooks load, and it is not hot-reloaded. See [startup.md](startup.md).

## Job-Loop Hooks (bracket the entire iteration)

Two hooks wrap the whole worker phase, both opt in to a queue table **by signature** (declare a parameter or use vararg; zero-param runs with no table):

| Hook | Fires | Table |
|---|---|---|
| `on_start(queue)` | once per job iteration, **before any worker** | `{ urls = seed }` - mutate `queue.urls` to commit this cycle's batch (dense string list ≤ 10,000, or `{}` for zero work); untouched = config seed intact |
| `on_finished(queue)` | once per job iteration, **after all workers complete** | `{ urls = {}, processed, failed, pushed, duplicates }` - final when called, mutations inert |

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
After all workers complete their cycles, `on_finished(queue)` fires (see job-loop hooks above) before sleep. In a single-worker job, it fires immediately after `on_finally` with no workers in between.

### Flow (clockwise)

```
    on_start ──→ before_fetch ──→ override_fetch ──→ fetch ──→ after_fetch
        │            nil→skip         nil→fetch fail   auto       nil→skip
        │                ▲                                        │
        │                │                                        ▼
        │           sleep(interval)                       override_extract
        │                │                                  nil→empty
        │                ▼                                        │
        │           on_finished ←──── (workers drain batch) ◄── extract
        │                │                                        auto
        │                ▼                                        │
        │           on_finally                                after_extract
        │               ▲                                    nil→skip rst
        │               │                                        │
        │          on_success/error                             ▼
        │               ▲                                   filter_item
        │               │                                  nil→skip item
        │               │                                        ▼
        │          notify+webhook                              before_store
        │               ▲                                  nil→skip s/n
        │               │                                        ▼
        │          before_webhook  nil→skip                 dedup+insert
        │               ▲                                      auto
        │               │                                        │
        │          before_notify ◄─────────────────────────────────┘
        │          nil→skip notif
        └── (loop: on_finished → sleep → on_start)
```

All per-cycle lifecycle hooks (`on_success`/`on_error` → `on_finally`) fire on every worker cycle regardless of nil skips. `on_start` and `on_finished` fire exactly once per job iteration, before/after all workers respectively. `sleep` then loops back to `on_start`.

---

## 1. Hook-Scoped Cleanup: `defer(fn)`

`defer(fn)` is a global function available inside pipeline stage hooks (`before_fetch`, `override_fetch`, etc.). It registers a callback to be executed **immediately after the current hook finishes**, regardless of whether it returned successfully or raised an error. It is **not supported** in the job-loop hooks (`on_start`, `on_finished`) — calling it there raises deterministically and the hook fails (seed intact for `on_start`).

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

## 3. Job-Loop Hooks: `on_start()` / `on_finished()`

> **Arity gate:** Both hooks opt in to their queue table **by signature** - declare a parameter (any name) or use vararg to receive it. A zero-param function (`function on_finished() end`) runs with no table.

> **Single-worker jobs:** If `workers = 1`, both hooks still fire (once, around the one inline cycle). Use `on_finally` in `defer.lua` for per-cycle cleanup.

### `on_start(queue)` — before any worker

Fires **once per job iteration, before any worker starts**, in all three modes. Mutate `queue.urls` to commit this cycle's batch (dense string list, entries opaque, ≤ 10,000 items, or `{}` for explicit zero work). Untouched = config seed intact. Zero-param `on_start()` runs with no table for pure lifecycle work (auth token fetch, global reset). There is no `dynamic_queue` config key — mode is derived per cycle from batch presence.

### `on_finished(queue)` — after all workers

Fires **after all workers complete their cycles**, just before the job sleeps. The table is final when the hook fires. mutations are inert (no commit path).

| Field | Meaning |
|---|---|
| `urls` | Always `{}`; `on_finished` is not a commit point |
| `processed` | `run_once` invocations that returned this cycle (Ok + Err, including internal nil-skips) |
| `failed` | Subset of `processed` where `run_once` returned `Err` (hook/DB/infra errors; HTTP failures recovered via `after_fetch` are not failed) |
| `pushed` | Accepted `queue_push` calls this cycle |
| `duplicates` | Rejected `queue_push` calls this cycle (repeat URL or seed re-push) |

Errors inside `on_finished` are log-only: counters are already final and unaffected. No counter state leaks across cycles. `defer()` is not supported here either.

### Key Differences from `on_finally()`

| Aspect | `on_start` / `on_finished` | `on_finally(ctx)` |
|--------|---------------------------|-------------------|
| Scope | Once per job iteration (pre/post all workers) | Per-cycle (per-worker) |
| When it fires | Before workers start / after all complete | After each individual cycle ends |
| Table | `on_start`: may commit batch; `on_finished`: read-only stats | Receives per-cycle `ctx` |
| Where defined | `hooks.lua` or `defer.lua` | `defer.lua` |

### Example

```lua
function on_start(queue)
    -- commit nothing: config seed / legacy dispatch unchanged
end

function on_finished(queue)
    log(string.format("done: %d processed, %d failed",
        queue.processed, queue.failed))
end
```

### Use Cases
- Logging job-level completion metrics
- Releasing global resources shared across workers
- Running cleanup that should happen after all workers finish, not after each one
- Reading the final per-iteration `processed`/`failed` counters

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
| Commit a dynamic batch before workers start | `on_start(queue)` |
| Push aggregate metrics after a cycle finishes | `defer.lua` + `on_success` / `on_finally` |
| Alert on job failure via HTTP API | `defer.lua` + `on_error` |
| Clean up shared state at the end of a run | `defer.lua` + `on_finally` |
| Handle nested resources in a specific order | `defer()` multiple times (LIFO) |
| Read per-iteration processed/failed counters | `on_finished(queue)` |
| Log/track job-level completion across all workers | `on_finished(queue)` |

## Memory Hygiene

SpyWeb automatically wipes transient cycle state like `last_fetch` and `selector_matches` from the per-cycle context after `on_finally` returns to keep the memory footprint low during sleep intervals. Job-level state persists across cycles and is available in `on_finished()`.
