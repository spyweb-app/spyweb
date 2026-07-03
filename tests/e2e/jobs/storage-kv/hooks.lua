_G.log = {}

function before_fetch(request, ctx)
    store_set("key", "val")
    local v = store_get("key")
    assert(v == "val", "expected 'val', got " .. tostring(v))

    local missing = store_get("nonexistent")
    assert(missing == nil, "nonexistent key should return nil")

    store_delete("key")

    global_store_set("get_test", "hello")
    local got = global_store_get("get_test")
    assert(got == "hello", "global_store_get: expected hello, got " .. tostring(got))
    global_store_delete("get_test")

    global_store_set("global_counter", 100)
    local read_counter = global_store_get("global_counter")
    assert(read_counter == "100", "global_store_get: expected 100, got " .. tostring(read_counter))
    local new_val = global_store_incr("global_counter", 0, 1)
    assert(new_val == 101, "expected 101, got " .. tostring(new_val))
    global_store_delete("global_counter")

    _G.store_tested = true
    return request
end

function override_fetch(request, ctx)
    local c = store_get("counter")
    store_set("counter", (c or 0) + 1)
    return { status = 200, body = "<html>ok</html>", url = request.url }
end

function after_fetch(result, ctx)
    local c = store_get("counter")
    table.insert(_G.log, "counter:" .. tostring(c or "nil"))
    return result
end
