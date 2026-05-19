-- Add the diagnostic line below if you are using a Lua LSP and getting warnings.
---@diagnostic disable: lowercase-global

-- JS Rendering Example using Native CDP
-- This example demonstrates how to use the built-in Chrome DevTools Protocol
-- to render JavaScript-heavy pages without external services.

-- We keep the browser in a global variable to persist it across scraper iterations.
-- This makes subsequent fetches much faster.
if not browser then
    print("[CDP] Launching persistent browser...")
    browser = cdp.launch({
        headless = true,
        keep_alive = true, -- Keep process alive if the Lua variable is GC'd
        -- executable = "/path/to/custom/chrome", -- Optional: manual path
    })
end

-- We override the fetch phase to use our browser instead of the default HTTP client.
function override_fetch(request)
    print("[CDP] Navigating to: " .. request.url)
    
    -- Attach to a new page (tab)
    local page = browser:attach()
    
    -- Automatically close the page when the hook exits
    defer(function() page:close() end)
    
    -- 1. Open the URL and wait for the load event
    local ok, err = page:open(request.url)
    if not ok then
        return { error = "Failed to open page: " .. tostring(err) }
    end

    -- 2. Wait for a specific selector that indicates JS has finished rendering
    -- The second argument is a timeout in milliseconds.
    print("[CDP] Waiting for content...")
    local found, selector_err = page:wait_for_selector(".js-rendered-item", 10000)
    
    if not found then
        print("[CDP] Warning: Selector not found, returning partial HTML. Error: " .. tostring(selector_err))
    end

    -- 3. Optional: Take a screenshot using the high-level helper
    page:screenshot("last_render.png")
    print("[CDP] Screenshot saved to last_render.png")

    -- 4. Get the fully rendered HTML
    local html = page:content()
    
    return {
        status = 200,
        body = html,
        url = request.url
    }
end

-- You can also use after_extract to perform actions after data is found
function after_extract(items)
    print("[CDP] Extraction complete. Found " .. #items .. " items.")
    return items
end
