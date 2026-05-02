-- Add the diagnostic line below if you are using a Lua LSP and getting warnings.
-- These functions MUST be global (not local) or SpyWeb binary cannot see them.
-- The lua-language-server will complain about "lowercase-global" - that is expected.
---@diagnostic disable: lowercase-global

-- Full Pipeline Example
-- This hook demonstrates all 7 stages of the SpyWeb lifecycle.

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

-- State persists for the lifetime of the job process.
-- Use this for pagination or rate limiting.
iter = 1

-- 1. Modify the outgoing request (URL, headers, etc.)
function before_fetch(request)
    print("[1] before_fetch - iteration: " .. iter)
    
    -- Safety stop example
    if iter > 10 then
        print("[!] Stopping: reached iteration limit")
        return nil -- Abort the request
    end

    return request
end

-- 2. Inspect or modify the raw response body/status before extraction
function after_fetch(fetch_result)
    if not fetch_result.ok then
        print("[2] after_fetch - error: " .. fetch_result.error.message)
        return nil
    end

    print("[2] after_fetch - status: " .. fetch_result.response.status)
    -- fetch_result.response.body = fetch_result.response.body:gsub("badword", "goodword")
    return fetch_result
end

-- 3. Modify the entire list of extracted items at once.
-- This runs even if 0 items were found (useful for detecting site changes).
function after_extract(items)
    print("[3] after_extract - found " .. #items .. " items")

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

-- 4. Clean or transform a single item (runs for every item)
-- Returning nil here drops the specific item.
function filter_item(item)
    -- print("[4] filter_item: " .. item.fields.title)
    return item
end

-- 5. Last chance to modify items before they are checked against the Database (dedup)
function before_store(items)
    print("[5] before_store - " .. #items .. " items passing to DB")
    return items
end

-- 6. Modify items before they trigger a desktop notification
-- Only 'new' items (passed dedup) reach here.
function before_notify(items)
    print("[6] before_notify - " .. #items .. " NEW items found")
    return items
end

-- 7. Modify or abort the JSON payload sent to your webhook
function before_webhook(payload)
    print("[7] before_webhook - Preparing POST to " .. payload.job_name)
    
    -- Increment state for the next run
    iter = iter + 1
    
    return payload
end
