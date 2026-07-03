_G.log = {}

function before_fetch(request, ctx)
    -- Write and read text
    fs_overwrite("test.txt", "hello world")
    table.insert(_G.log, "read:" .. fs_read("test.txt"))

    -- Append
    fs_append("test.txt", "\nline 2")
    table.insert(_G.log, "appended:" .. fs_read("test.txt"))

    -- Binary via fs_read_binary (file must have allowed extension)
    fs_overwrite("data.txt", "binary\0data stuff")
    local bin = fs_read_binary("data.txt")
    table.insert(_G.log, "binary_len:" .. tostring(#bin))

    -- Path traversal rejected
    local ok1, err1 = pcall(fs_read, "../outside.txt")
    table.insert(_G.log, "traversal_rejected:" .. tostring(not ok1))

    -- Absolute path rejected
    local ok2, err2 = pcall(fs_read, "/etc/passwd")
    table.insert(_G.log, "absolute_rejected:" .. tostring(not ok2))

    -- JSON file
    fs_overwrite("data.json", json_encode({ a = 1 }))
    table.insert(_G.log, "json:" .. fs_read("data.json"))

    -- shared/ fallback (Rust test writes shared/global.txt beforehand)
    local shared_content = fs_read("shared/global.txt")
    table.insert(_G.log, "shared:" .. tostring(shared_content))

    -- Write to shared/ explicitly
    fs_overwrite("shared/e2e_test.txt", "from shared write")
    table.insert(_G.log, "shared_write:" .. fs_read("shared/e2e_test.txt"))

    return request
end

function override_fetch(request, ctx)
    return { status = 200, body = "<html>ok</html>", url = request.url }
end
