# Multi-Worker Jobs

Multi-worker mode lets a single job run multiple concurrent scraping cycles. This is useful for monitoring many URLs with one job, or for load-testing a single endpoint with multiple requests in flight.

Worker tasks are async and overlap I/O. Lua execution is serialized by the job's Lua lock; a hook that awaits releases the lock while it waits, so workers interleave at await points, and synchronous Lua segments run atomically.

Think of it like having multiple scraper bots working for the same job at the same time.

---

## When to Use Multi-Worker

All URLs in a multi-worker job **must use the same pipeline and extraction logic**. If URLs need different selectors, fields, or hook behavior, use separate jobs instead. (Exception: `override_extract` can handle different structures programmatically.)

Multi-worker mode is designed for **I/O-bound concurrency**. Use it when:

- **Pipeline Consolidation:** Many URLs with identical extraction logic (e.g., 100 product categories with the same selector and fields), processed in a single job instead of 100 separate ones.
- **Shared State Coordination:** All workers share the same Lua VM. Use this to maintain in-memory caches, global rate-limiters, or IP rotation state without the overhead of database roundtrips.
- **Parallel Queue Processing:** You have a large list of `urls` and want to maximize throughput by keeping multiple targets in flight at once.
- **I/O Wait Mitigation:** Your targets or proxies have high latency. Workers ensure the engine stays productive even when several tasks are blocked waiting for network handshakes or slow server responses. Hooks that await (`sleep`, `http_get`, CDP) release the job's Lua lock while they wait, so other workers run in the meantime. Synchronous Lua executes to completion under the lock.
- **High-Frequency Monitoring:** For volatile data (e.g., restock alerts or price changes), multiple workers reduce the time gap between checks, ensuring one worker is fetching while another is processing.

**When NOT to use it:**

- **Anti-Bot Pressure:** High concurrency from a single IP is a strong signal for anti-bot detection. A single worker with a modest `interval` is safer for aggressive targets.
- **Sequential Logic:** If a specific execution order is required (e.g., Step B *must* see the result of Step A), workers are too non-deterministic; use a single worker or separate jobs.
- **Resource Constraints:** Each worker adds a small memory overhead to the Lua VM and the network stack. For simple hourly checks, a single worker is most efficient.

---

## Operational Modes

A job picks one of three modes based on its config:

| Mode | Config | What happens |
|------|--------|-------------|
| **URL queue** | `urls` is set (any `workers`) | URLs are distributed across workers. Each worker picks the next unvisited URL. Workers finish when the queue is empty. |
| **Multi-worker single URL** | `workers > 1`, no `urls` | Every worker independently scrapes the same target URL. Each one runs a full cycle (`before_fetch` → fetch → extract → ...). |
| **Single worker** | default (`workers = 1`, no `urls`) | One inline cycle, no task overhead. This is the classic mode. |

> **`urls` takes priority.** If you set `urls`, the job always uses URL queue mode regardless of `workers`.

---

## URL Queue Mode

Use this when you have many URLs to scrape and want them split across workers.

```toml
name = "Product Monitor"
urls = [
  "https://shop.example.com/electronics",
  "https://shop.example.com/clothing",
  "https://shop.example.com/home",
  "https://shop.example.com/sports",
]
workers = 4
selector = ".item"
fields = ["title:h2", "price:.price"]
```

**How it works:**
- All URLs go into a shared queue.
- Each worker grabs the next available URL, runs its cycle, then grabs another.
- When the queue is empty, workers shut down.
- If one worker's cycle fails, it logs the error and moves to the next URL - other workers are unaffected.

**No `url` field needed:** When you set `urls`, the `url` field is ignored. The job gets its entry points entirely from the list.

---

## Multi-Worker Single URL Mode

**Use this mode when you want to programmatically or dynamically assign URLs in Lua instead of defining a static list in your config.**

```toml
name = "Dynamic URL Dispatcher"
url = "https://example.com" # Entry point (can be overridden in Lua)
workers = 4
```

**How it works:**
- The engine spawns `workers` number of tasks simultaneously.
- While they technically start with the same `url`, you use the `before_fetch` hook to override it based on your own logic (e.g., pulling from a database, an API, or a calculated list).
- This is the most flexible mode for complex jobs where the target URLs are not known upfront.

```lua
-- Example: Programmatically assigning different pages to different workers
function before_fetch(request, ctx)
    -- Instead of a static list in TOML, we calculate the URL dynamically
    request.url = "https://example.com/api/data?page=" .. ctx.worker_id
    
    log("Worker " .. ctx.worker_id .. " dynamically assigned to page " .. ctx.worker_id)
    return request
end
```

**Why use this?**
- **Dynamic URL Assignment:** You don't have to hardcode URLs in your TOML. You can generate them on the fly in Lua.
- **Concurrent Processing:** All `N` workers run at the same time; their hooks interleave at await points, while synchronous Lua segments run one at a time.
- **Improved Throughput:** Network latency on one dynamic target doesn't stall the others.

---

## Worker Stagger

Workers are not all launched at the exact same instant. The first worker starts immediately, the next starts 200ms later, the next 400ms later, and so on. 

**Technical Rationale:**
- **Rate Limit Avoidance:** Prevents a sudden burst of simultaneous requests that could trigger anti-bot protections.
- **Network Reliability:** Prevents **Socket Exhaustion** (`EADDRNOTAVAIL`) and DNS burst failures that occur when dozens of workers attempt to open connections in the same millisecond.

```text
Worker 1: start
Worker 2: start +200ms
Worker 3: start +400ms
Worker 4: start +600ms
```

The stagger only affects the initial launch of workers within a single job iteration. In URL queue mode, after a worker finishes one URL and grabs the next from the queue, it runs again immediately - no re-staggering between URLs. Each job iteration (after sleep) spawns fresh workers with stagger again.

---

## Using `worker_id`

Each worker gets a 1-based ID that you can read from `ctx.worker_id` in any hook.

**Persistence in Queue Mode:**
In **URL Queue Mode**, workers are persistent "lanes." If Worker 1 assigns a specific proxy or header in `before_fetch`, it can maintain that identity across every URL it pulls from the queue. This is useful for pinning specific workers to specific accounts or tunnels.

**Key rules:**
- `worker_id` ranges from `1` to `workers`.
- It is **read-only**. Writing `ctx.worker_id = ...` raises an error.
- For single-worker jobs, `worker_id` is always `1`.

---

## Dynamic Batch via `on_start`

The [`on_start(queue)` hook](lifecycle.md#3-job-loop-hooks-on_start--on_finished) fires **once per job iteration, before any worker starts**, and can commit this cycle's batch:

```lua
function on_start(queue)
    local due = db_query([[
        SELECT id, url FROM targets
        WHERE (strftime('%s','now') - last_check) >= interval_sec
    ]])
    if #due == 0 then
        queue.urls = {}  -- nothing due: explicit zero-work cycle
        return
    end
    queue.urls = {}
    for _, row in ipairs(due) do
        table.insert(queue.urls, "monitor:" .. row.id)  -- opaque entries
    end
end
```

Workers then drain the committed batch via URL queue dispatch — every worker processes distinct targets, no DB claim race. Batch entries are opaque strings, not validated URLs: `before_fetch` parses them and rewrites `request.url`. A rewrite bug fails loudly (fetch of `monitor:123` errors) instead of silently hitting a placeholder.

The queue table is opt-in by signature, zero-param `on_start()` runs with no table for pure lifecycle work (auth token fetch, global reset). Mode is derived per cycle from the committed batch: leave `queue.urls` untouched and the config seed / legacy dispatch runs exactly as before. There is no `dynamic_queue` config key.

---

## Job-Level Completion with `on_finished`

Define `on_finished(queue)` to run logic **after all workers have completed** their cycles, just before the job goes to sleep. (See [Execution Lifecycle](lifecycle.md#3-job-loop-hooks-on_start--on_finished) for more details).

### Recommended Usage
- **When to use:** Use `on_finished` specifically for **multi-worker batch jobs**. If your job runs with a single worker (`workers = 1`), `on_success`, `on_error`, and `on_finally` are more idiomatic and sufficient for handling request lifecycles.
- **Placement:** `on_finished` can live in `hooks.lua` or `defer.lua`, either works. Keep it near your other batch-level logic.

### The stats table

The table is **final** when the hook fires — there is no commit path from `on_finished`, mutations are inert.

| Field | Meaning |
|---|---|
| `urls` | Always `{}` |
| `processed` | `run_once` invocations that returned this cycle (Ok + Err, including internal nil-skips) |
| `failed` | Subset of `processed` where `run_once` returned `Err` (hook/DB/infra errors; HTTP failures recovered via `after_fetch` are not failed) |
| `pushed` | Accepted `queue_push` calls this cycle |
| `duplicates` | Rejected `queue_push` calls this cycle (repeat URL or seed re-push) |

Domain counts ("monitors actually checked") are the script's business: buffer in a Lua global during the cycle, read here.

```lua
function on_finished(queue)
    log(string.format("Iteration done: %d processed, %d failed (%d domain-checked)",
        queue.processed, queue.failed, _G.domain_checked or 0))
    _G.domain_checked = nil  -- clear for next cycle
end
```

Unlike per-URL hooks, `on_finished`:
- Receives the final stats table when declared with a parameter (arity gate — `function on_finished() end` gets no table).
- Runs **once per job iteration**, not once per worker.
- Is the right place for job-level metrics, summary notifications, or batch file flushing.
- Errors inside it are **log-only** — counters are already final and unaffected.

---

## Dynamic Growth with `queue_push`

A worker can add work to the current cycle while it is running:

```lua
function after_fetch(fetch_result, ctx)
    local next_url = parse_next_page(fetch_result.body)
    if next_url then
        queue_push(next_url)
    end
    return fetch_result
end
```

`queue_push(url)` appends to the cycle's live queue. It is valid only from a hook running inside an active queue cycle; calling it from `on_start`, `on_finished`, or outside a queue cycle raises `queue_push requires an active queue cycle`.

Pushes are handled by a single coordinator, so workers never race on the pending queue. The coordinator keeps the cycle alive until **both** the pending queue is empty and no worker is in flight. A URL pushed by a worker that is still finishing its hook is therefore processed before shutdown.

Rules:
- **Dedup is by exact string.** A repeat of a seed URL or an already-pushed URL is dropped silently and increments `duplicates`; it does not raise. `monitor:5` and `monitor:05` are different entries.
- **The cap is shared.** Initial batch entries plus accepted pushes must stay within `MAX_BATCH` (10,000). An over-cap push raises `queue_push rejected: cycle queue is full`.
- **Pushes never block.** The binding only reserves capacity and sends an unbounded message.
- **Cancellation is safe.** The coordinator lives inside the cycle future; if the cycle is dropped, workers are cancelled and the next cycle starts with clean queue state.
- **Counters are final in `on_finished`.** `pushed` counts accepted calls; `duplicates` counts rejected ones.

---

## Advanced Example: Database-Driven Multi-Target Monitoring

This pattern uses a custom SQLite table to manage multiple targets, claiming every due target in `on_start` and handing the whole batch to the workers (claim-all pattern). No per-worker claim race — workers pull distinct `monitor:<id>` entries from the committed batch.

```lua
-- Initialize the target table once at startup (top of hooks.lua)
db_exec([[
    CREATE TABLE IF NOT EXISTS targets (
        id TEXT PRIMARY KEY,
        url TEXT NOT NULL,
        interval_sec INTEGER DEFAULT 60,
        last_check INTEGER DEFAULT 0
    )
]])

-- Seed targets if the table is empty
db_exec("INSERT OR IGNORE INTO targets (id, url, interval_sec) VALUES (?, ?, ?)", {"example", "https://example.com", 60})
db_exec("INSERT OR IGNORE INTO targets (id, url, interval_sec) VALUES (?, ?, ?)", {"github", "https://github.com", 120})

function on_start(queue)
    local due = db_query([[
        SELECT id, url FROM targets
        WHERE (strftime('%s','now') - last_check) >= interval_sec
    ]])
    if #due == 0 then
        queue.urls = {}  -- nothing due: explicit zero-work cycle
        return
    end
    queue.urls = {}
    for _, row in ipairs(due) do
        table.insert(queue.urls, "monitor:" .. row.id)
    end
end

function before_fetch(request, ctx)
    local id = request.url:match("^monitor:(.+)$")
    if id then
        local row = db_query("SELECT url FROM targets WHERE id = ?", { id })
        if #row == 0 then return nil end  -- target deleted since on_start
        request.url = row[1].url
        ctx.shared.target_id = id
    end
    return request
end

function after_fetch(result, ctx)
    local id = ctx.shared.target_id
    if not id then return end
    if result.ok then
        log(string.format("[%s] SUCCESS", id))
    else
        log(string.format("[%s] FAILED: %s", id, result.error.message))
    end
    db_exec("UPDATE targets SET last_check = ? WHERE id = ?", { os.time(), id })
    _G.domain_checked = (_G.domain_checked or 0) + 1
end

function on_finished(queue)
    log(string.format("Iteration done: %d processed, %d failed (%d domain-checked)",
        queue.processed, queue.failed, _G.domain_checked or 0))
    _G.domain_checked = nil
end
```

The opaque entry format means two targets sharing the same URL cannot collide (dedup is by raw batch entry string). If a target's row is deleted between `on_start` and `before_fetch`, `before_fetch` returns nil and that slot is skipped cleanly.

---

## Shared State Semantics

All workers share the same Lua VM - global variables are visible to every worker. This is intentional and enables batch-level coordination.

- **Cycle Context (`ctx.shared`):** Use this to pass data between hooks in **one specific worker's cycle** (e.g., from `before_fetch` to `after_fetch`). It is destroyed as soon as that worker finishes its current URL.
- **Global State (`_G`):** Use this for **batch-wide state** accumulated across all workers. Unlike `ctx.shared`, data in `_G` persists until the entire job iteration is finished. Always clear/reset relevant `_G` variables in `on_finished()`.
- **Don't use `_G` for worker-local state:** If you set `_G.my_val = 1` in one worker, it will be visible to all other workers instantly, leading to race conditions.

**Hooks interleave at await points.** Worker tasks run concurrently, but Lua execution is serialized by the job's Lua lock. A hook that awaits (`sleep`, `http_get`, CDP, `page:*`) releases the lock while it waits, so another worker can execute Lua in the meantime. Synchronous Lua runs to completion under the lock, so a check-and-set with no await between is atomic:

```lua
-- WRONG: read -> await -> write on shared state is a cross-worker race
local v = cache[k]
http_get("https://api.example.com/" .. k)  -- the lock is released here
cache[k] = v + 1

-- CORRECT: no await between read and write -- one locked segment
cache[k] = (cache[k] or 0) + 1
```

**Serialized execution:** pure synchronous Lua. Everything a hook does before its first await, and after it resumes, executes under the lock, one worker at a time.

**Need truly independent state?** Split the work into separate jobs. Each job has its own Lua VM, its own lock, and its own browser profile folder, so jobs never contend with each other.

**Storage is serialised.** All storage operations (`store_set`, `store_get`, `global_store_*`) go through a single Lua mutex, so they are safe to use from any worker.

**Browser instances can be shared.** A persistent browser stored in a global is available to all workers.

---

## Worker Fault Tolerance

The multi-worker architecture is designed for high reliability. 

- **Independent Execution:** Each worker runs in its own concurrent task. If one worker encounters a fatal error (e.g., a Lua `error()` in `before_fetch` or a network crash), it logs the error and terminates that specific task.
- **Queue Resilience:** The shared URL queue is protected by a poison-safe mutex. A single worker panic will **not** poison the queue or crash other active workers.
- **Batch Continuity:** The job loop (`run_job_loop`) does not stop if an individual worker panics. It continues to manage the remaining workers and will still trigger `on_finished()` once all active tasks are drained.

---

## Troubleshooting & Common Pitfalls

| Pitfall | Cause | Solution |
| :--- | :--- | :--- |
| **`ctx` is nil in deferred task** | `defer()` tasks run asynchronously. They do not have automatic access to the `ctx` table of the hook that scheduled them. | If you need context data in a deferred task, pass a copy into the closure: `local ctx_copy = ctx; defer(function() log(ctx_copy.worker_id) end)`. |
| **Global variable "ghosts"** | Storing worker-local data in `_G`. | Always use `ctx` for worker-specific state. Use `_G` only for batch-level aggregations and **ensure you clear it in `on_finished`**. |
| **Unexpected URL behavior** | Misunderstanding dispatch modes. | If `urls` is set, the job **ignores** the `url` config. If you need dynamic URLs, use `before_fetch` to override the URL. |

---

## Concurrency Settings Comparison

To optimize performance, it is important to distinguish between process-wide thread limits and per-job worker concurrency.

| Setting | Type | Scope | Description |
| :--- | :--- | :--- | :--- |
| **`SPYWEB_THREADS`** | Env Var | Process | Total OS threads available to the application runtime. |
| **`workers`** | Config | Per-Job | Number of concurrent workers (scrapers) for a single job. |

**Best Practice:**
- If `SPYWEB_THREADS` is not set, the application defaults to using **2 threads**.
- For production workloads, set `SPYWEB_THREADS` explicitly to match your available CPU core count.
- Tune `workers` based on the target website's rate limits and the complexity of your `hooks.lua` logic. If `workers` exceeds your `SPYWEB_THREADS` count, you will see contention on CPU-bound extraction tasks.

---

## Config Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `workers` | u32 | `1` | Number of concurrent scraping workers |
| `urls` | string[] | *none* | Multiple entry URLs (overrides `url` for queue mode) |
