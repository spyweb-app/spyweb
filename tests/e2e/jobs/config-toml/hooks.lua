_G.log = {}

function before_fetch(request, ctx)
    if request.headers["X-Test-Header"] == "from-config" then
        table.insert(_G.log, "headers:ok")
    end
    return request
end

function override_fetch(request, ctx)
    return {
        status = 200,
        headers = { ["content-type"] = "text/html" },
        body = '<div class="item"><div class="title">target word</div><div class="desc">description for target</div></div><div class="item"><div class="title">other</div><div class="desc">no keyword here</div></div>',
        url = request.url
    }
end

function after_extract(items, ctx)
    table.insert(_G.log, "debug_hook_ran")
    return items
end

function filter_item(item, ctx)
    if item.fields.title and item.fields.title:find("target") then
        if item.matches and #item.matches > 0 then
            table.insert(_G.log, "keywords:ok")
        end
    end
    return item
end

function before_webhook(payload, ctx)
    table.insert(_G.log, "webhook_hook_called")
    return payload
end

function before_notify(items, ctx)
    if #items > 0 then
        table.insert(_G.log, "notify_hook_called")
    end
    return items
end

_G.config_tested = true
