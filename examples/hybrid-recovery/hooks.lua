-- ==========================================================================
-- SMART HYBRID SENTINEL: Production-Grade Recovery Flow
-- ==========================================================================

---@diagnostic disable: lowercase-global

-- --------------------------------------------------------------------------
-- 1. CONFIGURATION
-- --------------------------------------------------------------------------
local HEADLESS_BROWSER_PATH = "/usr/bin/lightpanda"
local VISUAL_BROWSER_PATH = "/usr/bin/google-chrome"

-- SELECTORS
local DATA_SELECTOR = ".item-row"            -- What we actually want
local DETECTION_SELECTOR = "#captcha-iframe"  -- What blocks us
local SUCCESS_SELECTOR = ".dashboard-loaded"  -- What human reaches after solving

-- TIMEOUTS
local DATA_WAIT_MS = 8000                     -- How long to wait for data in headless
local RECOVERY_TIMEOUT_MIN = 15               -- Total time allowed for human help

-- --------------------------------------------------------------------------
-- 2. HELPERS
-- --------------------------------------------------------------------------
function get_browser(headless)
    return cdp.launch({
        executable = headless and HEADLESS_BROWSER_PATH or VISUAL_BROWSER_PATH,
        headless = headless,
        keep_alive = true
    })
end

-- --------------------------------------------------------------------------
-- 3. THE GATEKEEPER (before_fetch)
-- --------------------------------------------------------------------------
function before_fetch(request)
    local state = store_get("recovery_state") or "NORMAL"
    
    if state == "RECOVERING" then
        print("⏳ [HYBRID] Recovery in progress. Checking for human success...")
        
        if not visual_browser then
            print("❌ [HYBRID] Visual browser lost. Resetting.")
            store_set("recovery_state", "NORMAL")
            return request
        end

        local page = visual_browser:attach({ reuse = true })
        
        -- Check if the puzzle is solved by looking for the SUCCESS_SELECTOR
        local solved = page:evaluate(string.format("document.querySelector('%s') !== null", SUCCESS_SELECTOR))
        
        if solved then
            print("✅ [HYBRID] Puzzle solved! Promoting run to extraction.")
            store_set("recovery_state", "SOLVED")
            return request
        end

        -- Check for timeout
        local start_time = tonumber(store_get("recovery_start") or 0)
        if os.time() - start_time > (RECOVERY_TIMEOUT_MIN * 60) then
            print("❌ [HYBRID] Human intervention timed out. Killing visual browser.")
            visual_browser:close()
            visual_browser = nil
            store_set("recovery_state", "NORMAL")
            notify({ title = "Recovery Timed Out", body = "No one solved the puzzle in time." })
            return nil
        end

        -- SHORT-CIRCUIT: Still waiting. Return nil to free the worker thread.
        return nil 
    end

    return request
end

-- --------------------------------------------------------------------------
-- 4. THE WORKER (override_fetch)
-- --------------------------------------------------------------------------
function override_fetch(request)
    local state = store_get("recovery_state") or "NORMAL"

    -- CASE A: We just recovered from a block
    if state == "SOLVED" then
        local page = visual_browser:attach({ reuse = true })
        local html = page:content()
        
        print("🚢 [HYBRID] Capturing recovered session data.")
        
        page:close()
        visual_browser:close()
        visual_browser = nil
        store_set("recovery_state", "NORMAL")
        
        return { status = 200, body = html, url = request.url }
    end

    -- CASE B: Normal automated run
    if not browser then
        browser = get_browser(true)
    end

    local page = browser:attach()
    local ok, err = page:open(request.url)
    
    if not ok then
        page:close()
        return { error = "Initial navigation failed: " .. tostring(err) }
    end

    -- STEP 1: Wait for actual data to appear
    print("[CDP] Waiting for data selector: " .. DATA_SELECTOR)
    local found, wait_err = page:wait_for_selector(DATA_SELECTOR, DATA_WAIT_MS)
    
    if found then
        -- SUCCESS: Data is present.
        local html = page:content()
        page:close()
        return { status = 200, body = html, url = request.url }
    end

    -- STEP 2: Data didn't show up. Was it a BOT BLOCK?
    print("[CDP] Data timeout. Checking for bot detection: " .. DETECTION_SELECTOR)
    local blocked = page:evaluate(string.format("document.querySelector('%s') !== null", DETECTION_SELECTOR))
    
    if blocked then
        print("💀 [HYBRID] Bot block confirmed. Triggering human intervention.")
        
        -- Cleanup headless
        page:close()
        browser:close()
        browser = nil
        
        -- Launch Visual
        visual_browser = get_browser(false)
        local v_page = visual_browser:attach()
        v_page:open(request.url)
        
        -- Set State
        store_set("recovery_state", "RECOVERING")
        store_set("recovery_start", tostring(os.time()))
        
        notify({
            title = "Scraper Blocked!",
            body = "A bot-block was detected. Please solve it manually to continue.",
            timeout = 0
        })

        return nil
    end

    -- STEP 3: Not blocked, but no data. This is a real site error or layout change.
    page:close()
    return { error = "Timeout: Data not found and no block detected. Selector: " .. DATA_SELECTOR }
end
