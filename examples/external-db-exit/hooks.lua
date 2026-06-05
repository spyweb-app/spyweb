-- The Clean Exit pattern
function before_store(items, ctx)
    if #items == 0 then
        return nil
    end

    -- Stash items on ctx.shared for defer.lua's on_success to push to external API
    ctx.shared.external_items = items

    -- Return nil to exit the pipeline!
    -- spyweb's internal DB never sees these items, and the desktop notifier never fires.
    return nil
end
