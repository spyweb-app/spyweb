_G.log = {}
local CDP_WS_URL = "CDP_WS_URL"
local CDP_EXECUTABLE = "CDP_EXECUTABLE"

function override_fetch(request, ctx)
    local browser, err
    if CDP_WS_URL ~= "CDP_WS_URL" then
        browser, err = cdp.connect(CDP_WS_URL)
    elseif CDP_EXECUTABLE ~= "CDP_EXECUTABLE" then
        browser, err = cdp.launch({ executable = CDP_EXECUTABLE, headless = true })
    else
        browser, err = cdp.launch({ headless = true })
    end
    if not browser then
        error("No browser: " .. tostring(err))
    end

    defer(function() browser:close() end)

    local page = browser:attach()
    local ok, nav_err = page:open("https://example.com")
    assert(ok, "Nav failed: " .. tostring(nav_err))

    local content = page:content()
    assert(#content > 0)
    table.insert(_G.log, "content_length:" .. tostring(#content))

    _G.cdp_tested = true
    return { status = 200, body = content, url = request.url }
end
