_G.log = {}
_G.worker_logs = {}

function before_fetch(request, ctx)
    local wid = ctx.worker_id
    if not _G.worker_logs[wid] then
        _G.worker_logs[wid] = {}
    end
    table.insert(_G.worker_logs[wid], "start:" .. request.url)

    local ok, err = pcall(function() ctx.worker_id = 999 end)
    local readonly = not ok
    table.insert(_G.worker_logs[wid], "readonly:" .. tostring(readonly))
    table.insert(_G.log, "readonly:" .. tostring(readonly))

    return request
end

function override_fetch(request, ctx)
    return { status = 200, body = "<html><div class='item'><h2>" .. request.url .. "</h2></div></html>", url = request.url }
end

function on_finished()
    _G.final_log = ""
    for w, entries in pairs(_G.worker_logs) do
        for _, e in ipairs(entries) do
            _G.final_log = _G.final_log .. "w" .. tostring(w) .. ":" .. e .. " "
        end
    end
end
