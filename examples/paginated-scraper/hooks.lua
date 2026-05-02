-- ============================================================
-- Paginated Scraper Hook
-- ============================================================
-- This script demonstrates how to use Lua globals to implement
-- pagination. The page counter persists across runs while the
-- current job process stays alive.
--
-- The scraper calls before_fetch() before every HTTP request.
-- We append ?page=N to the URL, cycling through pages 1-5.
-- ============================================================

max_pages = 5

function before_fetch(request)
    page = page or 1

    -- Append page parameter to the base URL
    if string.find(request.url, "?") then
        request.url = request.url .. "&page=" .. page
    else
        request.url = request.url .. "?page=" .. page
    end

    -- Cycle through pages
    page = page + 1
    if page > max_pages then
        page = 1
    end
    -- You can use store_set("page", tostring(page)) instead if you want
    -- pagination state to survive hot-reload and process restart.

    -- You can also add or modify headers here
    -- request.headers["X-Page"] = tostring(page)

    return request
end

function after_fetch(fetch_result)
    -- You can inspect or modify the response body here
    -- Return nil to skip extraction for this run
    if not fetch_result.ok then
        print("Request failed for " .. fetch_result.request.url .. ": " .. fetch_result.error.message)
        return nil
    end

    if fetch_result.response.status ~= 200 then
        print("Got status " .. fetch_result.response.status .. ", skipping extraction")
        return nil
    end

    return fetch_result
end

function after_extract(items)
    -- Batch hook — see all extracted items at once
    -- Good for cross-item deduplication or sorting logic
    -- Return the array (modified or filtered) to continue
    local current_page = page - 1
    if current_page <= 0 then current_page = max_pages end
    print("Extracted " .. #items .. " items from page " .. current_page)
    return items
end

function filter_item(item)
    -- Per-item filter — replaces the built-in keyword filter entirely
    -- Return nil to drop the item, return the item to keep it

    local title = item.fields.title or ""
    local price = item.fields.price or ""

    -- Skip items with empty titles
    if title == "" then
        return nil
    end

    -- Example: parse price and skip items over $1000
    local price_num = tonumber(string.match(price, "%d+"))
    if price_num and price_num > 1000 then
        return nil
    end

    -- You can mutate fields before they hit the database
    item.fields.title = string.upper(title)

    return item
end

function before_store(items)
    -- Last chance to drop items before dedup + insert
    -- Return nil to skip storing AND notifying entirely
    --
    -- WARNING: items dropped here have no DB record and will
    --          appear as new items again on the next run

    if #items == 0 then
        return nil
    end

    return items
end

function before_notify(items)
    -- Reshape what gets notified (items are already stored at this point)
    -- Return nil to silence notification entirely
    -- Return modified array to change what gets sent

    -- Example: only notify for the first 5 items
    local capped = {}
    for i = 1, math.min(#items, 5) do
        capped[i] = items[i]
    end

    return capped
end
