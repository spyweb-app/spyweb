function test_discovered_from_tests_lua()
    spyweb.assert_eq(1 + 1, 2)
end

function test_assert_fail_detected()
    spyweb.assert_ne("left", "right")
end

function test_uses_defer_lua_helper()
    spyweb.assert_eq(defer_helper(), "ready-from-defer")
end

function test_uses_hooks_lua_helper()
    spyweb.assert_eq(hooks_helper(), "ready-from-hooks")
end

function test_deliberately_fails()
    spyweb.assert_eq(1, 2, "this test should fail")
end

function test_sleep_works()
    local start = os.clock()
    sleep(5)
    local elapsed = (os.clock() - start) * 1000
    spyweb.assert_eq(elapsed >= 4, true)
end

function test_notify_available()
    local notify_type = type(notify)
    spyweb.assert_eq(notify_type, "function")
    local ok, err = pcall(notify, "test title", "test body", 1)
end

function test_log_works()
    local log_type = type(log)
    spyweb.assert_eq(log_type, "function")
    local ok, err = pcall(log, "e2e log test")
    spyweb.assert_eq(ok, true)
end
