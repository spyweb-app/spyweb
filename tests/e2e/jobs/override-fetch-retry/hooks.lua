_G.log = {}

function override_fetch(request, ctx)
    local proxies = {
        "http://127.0.0.1:1",
        "http://127.0.0.1:2",
    }

    for attempt = 1, 3 do
        for i, proxy_url in ipairs(proxies) do
            local res, err = http_request({
                url = request.url,
                method = request.method,
                headers = request.headers,
                proxy = proxy_url,
                timeout = 1,
            })
            if res then
                table.insert(_G.log, "retry_success")
                return res
            end
            table.insert(_G.log, "retry_fail:" .. attempt .. ":" .. i .. ":" .. (err.kind or "none"))
            sleep(50)
        end
    end

    return { error = "all retries exhausted" }
end

function after_fetch(fetch_result, ctx)
    table.insert(_G.log, "after_fetch_ok:" .. tostring(fetch_result.ok))
    if fetch_result.error then
        table.insert(_G.log, "after_fetch_error:" .. fetch_result.error.message)
    end
    return fetch_result
end
