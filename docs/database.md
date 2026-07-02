# Storage Backends

Spyweb is designed to be self-contained and portable. To achieve this, it uses an embedded database stored in a single `data` file in the working directory, requiring no external database server like PostgreSQL or MySQL.

You can choose between two storage backends depending on your needs. The choice is made at the time of download by picking the specific ZIP variant.

---

## At a Glance

| Feature | **KV** (redb, default) | **SQL** (SQLite, queryable) |
| :--- | :--- | :--- |
| **Binary Size** | ~7MB | ~7MB |
| **Philosophy** | "Minimalist & Zero-Configuration" | "Transparent & Queryable" |
| **Performance** | High (Low overhead) | Moderate (WAL optimized) |
| **External Access** | None (Internal format) | High (Any SQLite tool) |
| **Lua SQL Support** | No | **Yes** (`db_query`, `db_exec`) |
| **Concurrency** | Multi-reader, Single-writer | Serialized (Mutex) |
| **Data Format** | Key-Value (B-Tree) | Relational (SQL) |

---

## redb (The Default)

**redb** is the default backend for Spyweb. It is a high-performance, type-safe embedded key-value store written in pure Rust.

### When to use it
- You want a zero-maintenance, "hands-off" storage experience.
- You only care about the data as it appears in the Web Dashboard or Webhooks.
- You don't need to run custom SQL queries or manage a relational schema.
- You prefer an internal-only data format that stays out of your way.

### Technical Details
- **File:** `data` (A single binary file in the working directory).
- **Format:** Stores records as JSON strings keyed by `(job_id, reversed_timestamp)`.
- **Performance:** Extremely low overhead and very fast for sequential writes.

---

## SQLite (Full database control)

The **SQLite** backend is available in the `-sql` variants of Spyweb. It transforms your data store into a standard relational database.

### When to use it
- You want total control over your data structure, including creating your own custom tables and schemas.
- You want to use external tools (DBeaver, TablePlus, `sqlite3`) to analyze or modify your data.
- You want to build complex logic in Lua using raw SQL (JOINs, aggregations, etc.).
- You need to store and relate scraped data with your own custom state or external datasets.

### Technical Details
- **Files:** `data` (Main DB), `data-wal` (Write-Ahead Log), `data-shm` (Shared Memory) — all in the working directory.
- **Performance:** Configured with `PRAGMA journal_mode=WAL` and `PRAGMA synchronous=NORMAL` for high-concurrency and fast writes.

### Database Schema
The database contains three primary tables for both variants:

1. **`records`**: Stores the actual scraped data.
   - `job_id`: The ID of the job.
   - `rev_ts`: The nanosecond timestamp. Records are ordered newest-first.
   - `json`: The full extracted record in JSON format.
2. **`seen`**: Used for deduplication. Stores hashes of seen items.
3. **`lua_user`**: Used by the Lua `store_*` and `global_store_*` functions for persistent KV state.

### Lua SQL Bindings
When using the SQLite variant, your Lua scripts gain two powerful functions:

#### `db_query(sql, params)`
Executes a SELECT statement and returns an array of tables.
```lua
local rows = db_query("SELECT json FROM records WHERE job_id = ? LIMIT 5", { "my-job" })
for _, row in ipairs(rows) do
    local data = json_decode(row.json)
    print(data.title)
end
```

#### `db_exec(sql, params)`
Executes any SQL statement and returns the number of rows affected.
```lua
-- file: myjob/defer.lua
-- Top-level: runs once at load time (startup/reload)
db_exec("CREATE TABLE IF NOT EXISTS markers (id TEXT PRIMARY KEY, val INTEGER)")

function on_finished()
    -- Update it every cycle
    db_exec("INSERT OR REPLACE INTO markers (id, val) VALUES (?, ?)", { "last_page", 10 })
end
```

### Advanced Example: Shared Task Queue
One of the most powerful uses of the SQLite backend is creating your own **Task Queue** to coordinate multiple workers where you want multiple workers to process a list until it is finished.

#### 1. Initialize the Schema (top of `hooks.lua`)
Instead of hard-coding URLs in your config, you treat the database as your source of truth.

```lua
-- Create a table to track our targets
db_exec([[
    CREATE TABLE IF NOT EXISTS queue (
        id TEXT PRIMARY KEY,
        url TEXT NOT NULL,
        status TEXT DEFAULT 'pending' -- pending, processing, completed, failed
    )
]])

-- You can populate this table via an external script, or have 
-- a dedicated 'Discovery' job in SpyWeb whose only purpose is to 
-- find URLs and insert them into this table for other jobs to process.
```

#### 2. Worker Pick & Lock (`before_fetch`)
Workers use an atomic `UPDATE` to "claim" a task, ensuring no two workers ever fetch the same URL.

```lua
function before_fetch(request, ctx)
    -- ATOMIC: Find a pending task and mark it as 'processing' in one step.
    -- RETURNING allows us to get the URL for the row we just updated.
    local rows = db_query([[
        UPDATE queue 
        SET status = 'processing' 
        WHERE id = (
            SELECT id FROM queue 
            WHERE status = 'pending' 
            LIMIT 1
        )
        RETURNING id, url
    ]])

    if #rows == 0 then return nil end -- Queue finished
    
    local task = rows[1]
    request.url = task.url
    ctx.shared.current_task_id = task.id
    return request
end
```

#### 3. Mark Completion (`on_finally`)
Update the status based on whether the cycle succeeded or failed.

```lua
function on_finally(ctx)
    local task_id = ctx.shared.current_task_id
    if not task_id then return end

    local final_status = 'completed'
    if ctx.telemetry.map.fetch.status ~= "success" then
        final_status = 'failed'
    end

    db_exec("UPDATE queue SET status = ? WHERE id = ?", { final_status, task_id })
end
```

---

## Limitations & Warnings

### Format Compatibility
**The two backends are not binary compatible.** You cannot point the redb version of Spyweb at an SQLite `data` file or vice versa. If you decide to switch variants, you will need to manually export/import your data or start with a fresh database.


