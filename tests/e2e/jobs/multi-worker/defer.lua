function on_finally(ctx)
    _G.finished_workers = _G.finished_workers or {}
    table.insert(_G.finished_workers, ctx.worker_id)
end
