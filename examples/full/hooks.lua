-- Add the diagnostic line below if you are using a Lua LSP and getting warnings.
-- These functions MUST be global (not local) or SpyWeb binary cannot see them.
-- The lua-language-server will complain about "lowercase-global" - that is expected.
---@diagnostic disable: lowercase-global

-- Full Pipeline Example
-- This hook demonstrates all 9 stages of the SpyWeb lifecycle.

-- NOTE: Only define the hooks you actually need. 
-- SpyWeb pre-detects which functions exist at startup and skips the 
-- processing logic entirely for any that are missing.

-- Built-in Functions:
--   http_get(url, headers)    -> performs an async GET request
--   http_post(url, body, h)   -> performs an async POST request
--   dump(value)               -> returns a readable string for nested Lua values/tables
--   log(message)              -> appends to 'hook.log' in the job folder (persistent)
--   print(message)            -> outputs to the spyweb terminal/stderr (real-time)
--   require("module")         -> loads .lua files from current folder or 'shared/' folder
--   store_get/set/delete      -> optional persistent key/value storage scoped to this job
--   global_store_get/set/delete -> optional persistent key/value storage shared by all jobs

-- Lua globals persist while the job process stays alive.
-- Use store_* instead if the state should survive restart too.
iter = 1

-- 1. Modify the outgoing request (URL, headers, etc.)
function before_fetch(request)
    print("[1] before_fetch - iteration: " .. iter)
    
    -- Safety stop example
    if iter > 10 then
        print("[!] Stopping: reached iteration limit")
        return nil -- Abort the request
    end

    iter = iter + 1

    return request
end

-- 2. Override the entire fetch phase (e.g. use a different HTTP client or CDP)
-- function override_fetch(request)
--     print("[2] override_fetch - bypassing built-in fetch")
--     return {
--         status = 200,
--         body = "<html>Synthetic body</html>",
--         url = request.url
--     }
-- end

-- 3. Inspect or modify the raw response body/status before extraction
function after_fetch(fetch_result)
    if not fetch_result.ok then
        print("[3] after_fetch - error: " .. fetch_result.error.message)
        return nil
    end

    print("[3] after_fetch - status: " .. fetch_result.response.status)
    -- fetch_result.response.body = fetch_result.response.body:gsub("badword", "goodword")
    return fetch_result
end

-- 4. Override the entire extraction phase (e.g. parse JSON or XML)
-- function override_extract(response)
--     print("[4] override_extract - bypassing built-in CSS extraction")
--     return {
--         { fields = { title = "Custom Item", body = response.body } }
--     }
-- end

-- 5. Modify the entire list of extracted items at once.
-- This runs even if 0 items were found (useful for detecting site changes).
function after_extract(items)
    print("[5] after_extract - found " .. #items .. " items (out of " .. selector_matches .. " selector matches)")

    -- Example: Drop items with price more than 50,000
    local filtered = {}
    for _, item in ipairs(items) do
        -- Strip non-numeric characters (like $ or ,) before converting to number
        local price_str = (item.fields.price or ""):gsub("[^%d%.]", "")
        local price = tonumber(price_str)

        if not price or price <= 50000 then
            table.insert(filtered, item)    
        else
            print("[!] Dropping expensive item: " .. item.fields.price)
        end
    end
    return filtered
end

-- 6. Clean or transform a single item (runs for every item)
-- Returning nil here drops the specific item.
function filter_item(item)
    -- print("[6] filter_item: " .. item.fields.title)
    return item
end

-- 7. Last chance to modify items before they are checked against the Database (dedup)
function before_store(items)
    print("[7] before_store - " .. #items .. " items passing to DB")
    return items
end

-- 8. Modify items before they trigger a desktop notification
-- Only 'new' items (passed dedup) reach here.
function before_notify(items)
    print("[8] before_notify - " .. #items .. " NEW items found")
    return items
end

-- 9. Modify or abort the JSON payload sent to your webhook
function before_webhook(payload)
    print("[9] before_webhook - Preparing POST to " .. payload.job_name)
    return payload
end
