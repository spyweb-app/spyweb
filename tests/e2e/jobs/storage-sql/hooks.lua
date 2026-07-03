_G.log = {}

function before_fetch(request, ctx)
    -- Correctly pass parameters as a table to db_exec and db_query
    db_exec("CREATE TABLE IF NOT EXISTS test_items (id INTEGER PRIMARY KEY, name TEXT, value REAL)")

    local affected = db_exec("INSERT INTO test_items (name, value) VALUES (?, ?)", { "alpha", 1.5 })
    table.insert(_G.log, "insert_affected:" .. tostring(affected))

    db_exec("INSERT INTO test_items (name, value) VALUES (?, ?)", { "beta", 2.5 })
    db_exec("INSERT INTO test_items (name, value) VALUES (?, ?)", { "gamma", 3.5 })

    local rows = db_query("SELECT * FROM test_items WHERE value > ?", { 2.0 })
    table.insert(_G.log, "rows_count:" .. tostring(#rows))
    
    if #rows > 0 then
        table.insert(_G.log, "select_found:1")
        table.insert(_G.log, "row_name:" .. rows[1].name)
    end

    local updated = db_exec("UPDATE test_items SET value = ? WHERE name = ?", { 99.9, "alpha" })
    table.insert(_G.log, "update_affected:" .. tostring(updated))

    local alpha = db_query("SELECT value FROM test_items WHERE name = ?", { "alpha" })
    if #alpha > 0 then
        table.insert(_G.log, "alpha_new_value:" .. tostring(alpha[1].value))
    end

    local deleted = db_exec("DELETE FROM test_items WHERE name = ?", { "gamma" })
    table.insert(_G.log, "delete_affected:" .. tostring(deleted))

    local all = db_query("SELECT * FROM test_items ORDER BY id")
    table.insert(_G.log, "final_count:" .. tostring(#all))

    return request
end

function override_fetch(request, ctx)
    return { status = 200, body = "<html>ok</html>", url = request.url }
end
