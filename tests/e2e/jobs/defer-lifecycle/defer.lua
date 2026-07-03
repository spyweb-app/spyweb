function on_success(ctx)
    table.insert(_G.log, "on_success")
    table.insert(_G.log, "shared_items:" .. tostring(ctx.shared.passed_from_hooks))
    defer(function() table.insert(_G.log, "defer:in-on_success") end)
end

function on_error(err, ctx)
    table.insert(_G.log, "on_error:" .. tostring(err))
end

function on_finally(ctx)
    table.insert(_G.log, "on_finally")
    table.insert(_G.log, "tel_duration:" .. tostring(ctx.telemetry.total_duration_ms))
end
