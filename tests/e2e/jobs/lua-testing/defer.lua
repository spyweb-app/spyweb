function test_discovered_from_defer_lua()
    spyweb.assert_eq(type(json_encode), "function")
end

function test_active_ctx_available()
    local ok, ctx = pcall(function()
        return _G.defer
    end)
    spyweb.assert_eq(ok, true)
end

function defer_helper()
    return "ready-from-defer"
end
