_G.log = {}
local base = "http://127.0.0.1:MOCK_PORT"

function before_fetch(request, ctx)
    local res = http_get(base .. "/echo?q=hello")
    local data = json_decode(res.body)
    table.insert(_G.log, "get_query:" .. data.query)
    table.insert(_G.log, "get_status:" .. tostring(res.status))

    local post_res = http_post(base .. "/echo", json_encode({ foo = "bar" }),
        { ["Content-Type"] = "application/json" })
    local post_data = json_decode(post_res.body)
    table.insert(_G.log, "post_method:" .. post_data.method)
    table.insert(_G.log, "post_query:" .. post_data.query)

    local req_res = http_request({
        method = "DELETE",
        url = base .. "/echo",
        headers = { ["X-Custom"] = "val" }
    })
    table.insert(_G.log, "req_status:" .. tostring(req_res.status))

    local put_res = http_request({ method = "PUT", url = base .. "/echo", body = json_encode({ put = "test" }), headers = { ["Content-Type"] = "application/json" } })
    local put_data = json_decode(put_res.body)
    table.insert(_G.log, "put_method:" .. put_data.method)

    local patch_res = http_request({ method = "PATCH", url = base .. "/echo", body = json_encode({ patch = "test" }) })
    local patch_data = json_decode(patch_res.body)
    table.insert(_G.log, "patch_method:" .. patch_data.method)

    local head_res = http_request({ method = "HEAD", url = base .. "/echo" })
    local head_status = head_res.status
    table.insert(_G.log, "head_status:" .. tostring(head_status))

    local mp_res = http_multipart(base .. "/multipart",
        { field1 = "value1", file1 = { content = "binary data", filename = "test.txt", content_type = "text/plain" } }
    )
    table.insert(_G.log, "mp_response:" .. tostring(mp_res.status))

    local err_res = http_get(base .. "/status/500")
    table.insert(_G.log, "error_status:" .. tostring(err_res.status))

    http_post(base .. "/webhook", json_encode({ event = "cycle_complete" }))

    _G.http_tested = true
    return request
end

function override_fetch(request, ctx)
    return { status = 200, body = "<html>ok</html>", url = request.url }
end
