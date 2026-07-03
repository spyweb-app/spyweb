_G.log = {}

function before_fetch(request, ctx)
    if _G.skip_cycle then
        return nil
    end
    table.insert(_G.log, "before_fetch")
    request.headers["X-Test"] = "e2e"
    if _G.use_method then
        request.method = _G.use_method
    end
    return request
end

function override_fetch(request, ctx)
    table.insert(_G.log, "override_fetch")
    if _G.use_method then
        table.insert(_G.log, "method:" .. request.method)
    end
    if _G.make_fetch_error then
        return { error = "simulated fetch failure" }
    end
    return { status = 200, body = "<html><div class='item'><h2>test</h2></div></html>", url = request.url }
end

function after_fetch(result, ctx)
    table.insert(_G.log, "after_fetch")
    if _G.recover_from_error and result.error then
        return { status = 200, body = "<html><div class='item'><h2>recovered</h2></div></html>", url = result.request.url }
    end
    return result
end

function override_extract(response, ctx)
    table.insert(_G.log, "override_extract")
    return { { title = "extracted-item" } }
end

function after_extract(items, ctx)
    table.insert(_G.log, "after_extract")
    table.insert(items, { title = "enriched" })
    return items
end

function filter_item(item, ctx)
    table.insert(_G.log, "filter_item")
    if _G.drop_item and item.title == "enriched" then
        return nil
    end
    return item
end

function before_store(items, ctx)
    table.insert(_G.log, "before_store")
    if _G.read_ctx_state then
        table.insert(_G.log, "selector_matches:" .. tostring(ctx.selector_matches))
        if ctx.last_fetch then
            table.insert(_G.log, "last_fetch_ok:" .. tostring(ctx.last_fetch.ok))
            if ctx.last_fetch.response then
                table.insert(_G.log, "last_fetch_status:" .. tostring(ctx.last_fetch.response.status))
            end
        end
    end
    return items
end

function before_notify(items, ctx)
    table.insert(_G.log, "before_notify")
    if _G.skip_notify then
        return nil
    end
    return items
end

function before_webhook(payload, ctx)
    table.insert(_G.log, "before_webhook")
    if _G.reshape_webhook then
        payload.reshaped = true
        return payload
    end
    return payload
end

function on_finished()
    table.insert(_G.log, "on_finished")
end
