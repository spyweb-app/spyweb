_G.leaked = nil

function test_discovered_from_hooks_lua()
    spyweb.assert_eq(type(http_get), "function")
end

function test_mutates_global()
    _G.leaked = "i-was-here"
    spyweb.assert_eq(_G.leaked, "i-was-here")
end

function hooks_helper()
    return "ready-from-hooks"
end
