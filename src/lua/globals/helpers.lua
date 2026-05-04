function dump(value, indent, seen)
    seen = seen or {}
    indent = indent or 0
    local pad = string.rep("  ", indent)

    if value == nil then return "nil" end
    local t = type(value)

    if t == "string" then
        return string.format("%q", value)
    elseif t == "number" or t == "boolean" then
        return tostring(value)
    elseif t == "table" then
        if seen[value] then return "<cycle>" end
        seen[value] = true

        local entries = {}
        -- Note: using pairs to support both array and hash parts
        for k, v in pairs(value) do
            local key_str
            if type(k) == "string" and k:match("^[%a_][%w_]*$") then
                key_str = k
            else
                key_str = "[" .. dump(k, 0, {}) .. "]"
            end
            table.insert(entries, string.format("%s  %s = %s,", pad, key_str, dump(v, indent + 1, seen)))
        end
        seen[value] = nil

        if #entries == 0 then return "{}" end
        return "{\n" .. table.concat(entries, "\n") .. "\n" .. pad .. "}"
    else
        -- Handle UserData or other types by showing their tostring representation
        local s = tostring(value)
        if s == t then
            return "<" .. t .. ">"
        else
            return "<" .. s .. ">"
        end
    end
end

function copy(t)
    if type(t) ~= "table" then return t end
    local res = {}
    for k, v in pairs(t) do
        res[k] = v
    end
    return res
end

function deep_copy(t, seen)
    if type(t) ~= "table" then return t end
    seen = seen or {}
    if seen[t] then return seen[t] end

    local res = {}
    seen[t] = res
    for k, v in pairs(t) do
        -- We don't deep_copy keys per the discussion (most scraping keys are primitives)
        res[k] = deep_copy(v, seen)
    end
    return res
end
