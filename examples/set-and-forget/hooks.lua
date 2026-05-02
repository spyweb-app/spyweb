-- ==========================================================================
-- SPYWEB SENTINEL: The "Set & Forget" VPS Monitor
-- ==========================================================================
-- This script transforms SpyWeb into an autonomous monitoring agent ideal 
-- for long-term VPS deployments. 
--
-- SMART FEATURES:
-- 1. ntfy.sh Integration: Sends push alerts to your phone (no registration).
-- 2. Failure Masking: Ignores random network blips; only alerts on real issues.
-- 3. Circuit Breaker: Stops hitting the site if you get banned or it goes down.
-- 4. Self-Healing: Automatically tries to recover after a 24-hour cooldown.
-- 5. VIP Alerts: Only sends push notifications for "PRICE DROP" items.
-- ==========================================================================


-- --------------------------------------------------------------------------
-- 1. CONFIGURATION
-- --------------------------------------------------------------------------
local NTFY_TOPIC = "my-spyweb-alerts" -- CHANGE THIS to your unique topic
local FAIL_THRESHOLD = 30             -- Alert after 30 consecutive failures
local STOP_THRESHOLD = 50             -- Stop fetching after 50 failures

-- --------------------------------------------------------------------------
-- 2. HELPERS
-- Subscribe to a topic on ntfy.sh to receive these alerts on your phone.
-- --------------------------------------------------------------------------
function send_push(title, message, priority)
    log("Pushing alert: " .. title)
    http_post("https://ntfy.sh/" .. NTFY_TOPIC, message, {
        ["Title"] = title,
        ["Priority"] = tostring(priority or 3),
        ["Tags"] = "spyweb,sentinel"
    })
end

-- --------------------------------------------------------------------------
-- 3. STATE (Persists across hot-reload and process restart)
-- --------------------------------------------------------------------------
local ONE_DAY = 24 * 60 * 60 -- 24 hours in seconds

-- --------------------------------------------------------------------------
-- 4. HOOKS
-- --------------------------------------------------------------------------

-- Circuit Breaker & Self-Healing: Stop fetching on fatal errors, 
-- but try to revive the job automatically after 24 hours.
function before_fetch(request)
    local fail_count = tonumber(store_get("fail_count")) or 0
    local stop_time = tonumber(store_get("stop_time")) or 0

    if fail_count >= STOP_THRESHOLD then
        local now = os.time()
        local elapsed = now - stop_time
        
        if elapsed < ONE_DAY then
            return nil -- Still in cooldown
        else
            log("Cooldown finished. Attempting auto-recovery probe...")
            store_set("stop_time", tostring(now)) -- Reset the 24h clock for the probe
        end
    end
    return request
end

-- Track failures and only alert when a real problem persists.
-- Aborts extraction (returns nil) on any failure.
function after_fetch(fetch_result)
    local fail_count = tonumber(store_get("fail_count")) or 0
    local status = fetch_result.response and fetch_result.response.status or nil
    local is_failure = not fetch_result.ok or status ~= 200

    if is_failure then
        -- Handle specific fatal errors immediately
        if status == 403 then
            log("💀 [FATAL] 403 Forbidden detected. We are likely banned. Entering cooldown.")
            store_set("fail_count", tostring(STOP_THRESHOLD))
            store_set("stop_time", tostring(os.time()))
            send_push("💀 BANNED (403)", "Target site has blocked our IP. Backing off for 24h.", 5)
            return nil
        end

        if status == 404 then
            log("⚠️ [WARNING] 404 Not Found. The URL might have changed or been removed.")
            send_push("⚠️ URL Changed (404)", "The target page is missing. Check your config.", 4)
            -- We don't increment fail_count for 404 to avoid circuit breaking on a dead link
            return nil
        end

        -- Standard failure (timeout, network drop, etc)
        fail_count = fail_count + 1
        store_set("fail_count", tostring(fail_count))
        log("Run failed. Fail count: " .. fail_count .. " | Error: " .. ((fetch_result.error and fetch_result.error.message) or status))

        if fail_count == FAIL_THRESHOLD then
            local err = status and ("HTTP " .. status) or fetch_result.error.message
            send_push("🚨 Job Failing", "Ongoing issues: " .. err, 4)
        elseif fail_count == STOP_THRESHOLD then
            store_set("stop_time", tostring(os.time()))
            send_push("💀 FATAL: Backing Off", "Reached " .. STOP_THRESHOLD .. " failures. Entering 24h cooldown.", 5)
        end
        return nil -- Stop the pipeline here
    end

    -- Success! Reset counters if we were previously failing.
    if fail_count >= FAIL_THRESHOLD then
        log("✅ Recovery detected. Clearing failure counters.")
        send_push("✅ Job Recovered", "Monitoring is back online.", 3)
    end
    store_delete("fail_count")
    store_delete("stop_time")
    return fetch_result
end

-- --------------------------------------------------------------------------
-- 5. PRICE DROP ALERTS
-- --------------------------------------------------------------------------
-- Monitor for price drops or sales. This demonstrates using the persistent
-- store to track state (lowest price) across different scrape runs.

function before_notify(items)
    for _, item in ipairs(items) do
        -- Clean currency symbols (e.g., "$1,200" -> "1200")
        local price_str = (item.fields.price or "0"):gsub("[^%d%.]", "")
        local price = tonumber(price_str) or 0
        local title = (item.fields.title or ""):upper()
        
        -- Check persistent store for the last seen minimum price
        local storage_key = "min_price_" .. (item.fields.id or item.fields.title)
        local last_min = tonumber(store_get(storage_key) or "999999")

        if price > 0 and price < last_min then
            send_push("📉 PRICE DROP: " .. item.fields.title, 
                      "Now $" .. price .. "! (Was $" .. last_min .. ")", 4)
            store_set(storage_key, tostring(price))
        elseif title:find("SALE") or title:find("OFF") then
            send_push("🎁 ON SALE: " .. item.fields.title, "Check it out!", 3)
        end
    end
    return items
end

-- --------------------------------------------------------------------------
-- 6. HEARTBEAT (Optional)
-- --------------------------------------------------------------------------
-- Enable this to get a once-a-day "Hi" so you know the VPS is still running.
-- Note: Must be integrated into an active hook (e.g., before_fetch).
-- last_heartbeat = tonumber(store_get("last_heartbeat") or "0")
-- local now = os.time()
-- if now - last_heartbeat >= (24 * 60 * 60) then
--     send_push("💓 Heartbeat", "Hey! Just letting you know I'm alive and kicking.", 2)
--     store_set("last_heartbeat", tostring(now))
-- end
