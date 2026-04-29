-- Custom function to format spyweb items to your external system's format
local function format_payload(items)
    local payload = "{\"records\": ["
    for i, item in ipairs(items) do
        payload = payload .. "{\"title\": \"" .. item.fields.title .. "\", \"url\": \"" .. item.fields.link .. "\"}"
        if i < #items then
            payload = payload .. ","
        end
    end
    payload = payload .. "]}"
    return payload
end

-- The Clean Exit pattern
function before_store(items)
    if #items == 0 then
        return nil
    end

    -- 1. Format the items for your custom backend
    local payload = format_payload(items)
    
    -- 2. Push to your external DB, Queue, or API using spyweb's http_post
    local res = http_post("https://api.my-infrastructure.com/ingest", payload)
    
    -- 3. Return nil to exit the pipeline! 
    -- By returning nil, spyweb drops the items. 
    -- spyweb's internal DB never sees them, and the desktop notifier never fires.
    return nil 
end
