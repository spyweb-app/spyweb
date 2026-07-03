_G.log = {}

function override_fetch(request, ctx)
    defer(function() table.insert(_G.log, "defer:error") error("simulated") end)
    defer(function() table.insert(_G.log, "defer:second") end)
    defer(function()
        table.insert(_G.log, "defer:first")
        defer(function() table.insert(_G.log, "defer:re-entrant") end)
    end)
    if _G.trigger_on_error then
        return { error = "simulated pipeline error for on_error test" }
    end

    if _G.test_defer_async then
        defer(function()
            local t1 = os.clock()
            local res = http_get("http://127.0.0.1:1/test")
            local elapsed = os.clock() - t1
            table.insert(_G.log, "defer:http_type:" .. type(res))
            table.insert(_G.log, "defer:http_elapsed:" .. tostring(elapsed))
        end)
        defer(function()
            local t1 = os.clock()
            sleep(200)
            local elapsed = os.clock() - t1
            table.insert(_G.log, "defer:sleep_elapsed:" .. tostring(elapsed))
        end)
    end

    return { status = 200, body = "<html>ok</html>", url = request.url }
end

function before_store(items, ctx)
    ctx.shared.passed_from_hooks = #items
    return nil
end
