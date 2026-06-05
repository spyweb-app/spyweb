-- defer.lua
-- This file handles post-cycle orchestration and metrics logging.
-- It runs once at the end of a full scraping cycle
-- and has full access to the shared global variables or ctx set during hooks.

-- We can log telemetry or trigger webhooks on run completion
function on_success(ctx)
    -- You can inspect ctx.telemetry here
    if ctx.telemetry then
        print(string.format("[defer.lua] Cycle completed successfully in %d ms", ctx.telemetry.total_duration_ms))
    end

    -- Push items to external API (items were stashed on ctx.shared by hooks.lua's before_store)
    if ctx.shared and ctx.shared.external_items then
        local payload = json_encode({ records = ctx.shared.external_items })
        http_post("https://api.my-infrastructure.com/ingest", payload, {
            ["Content-Type"] = "application/json"
        })
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
