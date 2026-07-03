-- Multi-worker URL queue with atomic DB-driven scheduling.
--
-- Uses the built-in SQLite database to coordinate which URL each
-- worker fetches next, avoiding duplicate work across workers.
--
-- Requires: config.toml with workers > 1

-- Initialize target table on first load (no-op if already exists)
db_exec([[
    CREATE TABLE IF NOT EXISTS targets (
        id          TEXT PRIMARY KEY,
        url         TEXT NOT NULL,
        interval_sec INTEGER DEFAULT 60,
        last_hit    INTEGER DEFAULT 0,
        last_status TEXT DEFAULT ''
    )
]])

-- Seed demo targets (INSERT OR IGNORE so it only runs once)
db_exec("INSERT OR IGNORE INTO targets (id, url, interval_sec) VALUES (?, ?, ?)",
        { "example",    "https://example.com",         10 })
db_exec("INSERT OR IGNORE INTO targets (id, url, interval_sec) VALUES (?, ?, ?)",
        { "google",     "https://www.google.com",      30 })
db_exec("INSERT OR IGNORE INTO targets (id, url, interval_sec) VALUES (?, ?, ?)",
        { "github",     "https://www.github.com",      60 })
db_exec("INSERT OR IGNORE INTO targets (id, url, interval_sec) VALUES (?, ?, ?)",
        { "httpbin",    "https://httpbin.org/get",     15 })
db_exec("INSERT OR IGNORE INTO targets (id, url, interval_sec) VALUES (?, ?, ?)",
        { "cloudflare", "https://www.cloudflare.com",  25 })

function before_fetch(request, ctx)
    local now = os.time()

    -- Atomically claim the next due target.
    -- UPDATE ... RETURNING is a single statement — no two workers
    -- can ever claim the same row, even with workers = 100.
    local rows = db_query([[
        UPDATE targets
        SET last_hit = ?
        WHERE id = (
            SELECT id FROM targets
            WHERE (? - last_hit) >= interval_sec
            LIMIT 1
        )
        RETURNING id, url
    ]], { now, now })

    if #rows == 0 then
        return nil  -- nothing due this cycle
    end

    request.url = rows[1].url
    ctx.shared.target_id = rows[1].id
    return request
end

function after_fetch(fetch_result, ctx)
    local id = ctx.shared.target_id
    if id == nil then return nil end

    local now = os.time()
    local ok = fetch_result.ok

    if ok then
        db_exec("UPDATE targets SET last_status = ? WHERE id = ?",
                { "ok", id })
        log(string.format("[%s] OK", id))
    else
        local msg = fetch_result.error.message or "unknown"
        db_exec("UPDATE targets SET last_status = ? WHERE id = ?",
                { "error: " .. msg, id })
        log(string.format("[%s] FAIL: %s", id, msg))
    end
    return nil
end
