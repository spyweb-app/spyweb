-- ============================================================
-- Custom Filtering Hook
-- ============================================================
-- When filter_item() exists in your hook.lua, the built-in
-- keyword filter is skipped entirely. This gives you full
-- control over which items make it to the database.
-- ============================================================

-- Blocked companies list (persists across runs)
blocked_companies = {
    ["spam corp"] = true,
    ["scam inc"] = true,
}

function filter_item(item)
    local title = (item.fields.title or ""):lower()
    local company = (item.fields.company or ""):lower()
    local salary = item.fields.salary or ""

    -- Drop items from blocked companies
    if blocked_companies[company] then
        return nil
    end

    -- Drop items with "intern" in the title
    if string.find(title, "intern") then
        return nil
    end

    -- Only keep items that mention specific technologies
    local dominated_by = false
    local wanted = {"rust", "go", "python", "kubernetes"}
    for _, tech in ipairs(wanted) do
        if string.find(title, tech) or string.find(title, tech:sub(1,1):upper() .. tech:sub(2)) then
            dominated_by = true
            break
        end
    end

    if not dominated_by then
        return nil
    end

    -- Normalize the salary field for consistent display
    local min_sal = tonumber(string.match(salary, "%d+"))
    if min_sal then
        item.fields.salary = "$" .. min_sal .. "k+"
    end

    return item
end

function before_notify(items)
    -- Only notify if we have more than 2 new items
    if #items < 2 then
        return nil  -- silence notification
    end
    return items
end
