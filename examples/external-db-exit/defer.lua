-- defer.lua
-- This file handles post-cycle orchestration and metrics logging.
-- Unlike hooks.lua, it runs once at the end of a full scraping cycle
-- and has full access to the shared global variables set during hooks.

-- We can log telemetry or trigger webhooks on run completion
function on_success()
    -- You can inspect the spyweb_telemetry global here
    if spyweb_telemetry then
        print(string.format("[defer.lua] Cycle completed successfully in %d ms", spyweb_telemetry.total_duration_ms))
    end
end

-- If the scraper fails with a fatal error, this hook fires
function on_error(err)
    print("[defer.lua] Scraper cycle encountered a fatal error: " .. tostring(err))
    
    -- Example: Send an alert webhook using http_post
    -- http_post("https://api.my-infrastructure.com/alerts", '{"error": "' .. tostring(err) .. '"}')
end

-- This hook ALWAYS fires at the very end of a cycle, regardless of success or error.
function on_finally()
    print("[defer.lua] Cycle finally hook executed. Cleaning up shared resources.")
end
