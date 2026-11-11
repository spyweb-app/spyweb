# SpyWeb project instructions

You are working in a SpyWeb user project: TOML configuration plus Lua hooks.
These instructions describe how to modify the project safely.

## Before writing code

1. From the project root, run:

       spyweb types

   This generates `spyweb-types.lua` and `.luarc.json`. Do not edit either
   generated file manually.
2. Read `spyweb-types.lua` before using SpyWeb bindings, hooks, or `ctx`
   fields. If a function or field is not declared there, do not invent it.
3. Read the relevant job configuration before changing its hooks:
   `jobs/<id>/config.toml`, or the root `jobs.toml`.
4. Search the offline documentation before guessing:

       rg -n -i "keyword or hook name" docs/

   Search both Markdown and HTML when needed. The offline docs are the
   primary behavioral reference; `spyweb-types.lua` is the API signature
   reference.

## Validation

Run these checks in order after making changes:

    spyweb check config
    spyweb test "<job>"
    spyweb debug "<job>"

Use `spyweb test` without a job name to run all project tests. Use
`spyweb test server` for the API server test suite. `spyweb debug` performs a
one-shot pre-store run and is the preferred iteration loop for a job.

Do not use `spyweb start` as a verification step; it is long-running.

## Project layout

    startup.lua                 # optional once-per-process setup
    jobs.toml                   # optional multi-job config
    jobs/<id>/config.toml       # per-job config
    jobs/<id>/hooks.lua         # pipeline and job-loop hooks
    jobs/<id>/defer.lua         # success, error, and finally hooks
    jobs/<id>/tests.lua         # test_* functions
    server/init.lua             # optional API server
    server/tests.lua            # optional server tests
    shared/                     # cross-job files
    docs/                       # offline documentation

## Important runtime rules

- `http_*`, `tls_probe`, `sleep`, CDP, and `fs_*` operations yield. Handle
  `(value, error)` results from `http_*` and `json_decode` before using the
  value.
- `db_query` and `db_exec` are available only when
  `engine.storage == "sqlite"`; guard database-specific code accordingly.
- Use `env_get("KEY")` for secrets. It reads `SPYWEB_KEY` unless the key
  already begins with `SPYWEB_`. Never hardcode credentials.
- File paths are job-directory relative. Use `shared/` for project shared
  files, and stay within the approved file extensions.
- `startup.lua` is project-root relative, runs once before hooks load, and
  cannot use job-scoped storage, `defer`, `queue_push`, or CDP.

## Hook and concurrency rules

- Preserve the pipeline order documented in `docs/`.
- Returning `nil` has hook-specific meaning. Check the hook contract before
  changing a return value; `nil` may skip later stages, drop an item, or be an
  error depending on the hook.
- `filter_item` returns the item to keep; returning `nil` drops it.
- `before_store` controls whether store and notification continue.
- `before_notify` and `before_webhook` can skip their respective actions.
- `on_start(queue)` must mutate `queue.urls` in place. Rebinding a local table
  does not update the queue. `on_finished(queue)` receives final counters and
  cannot change the completed run.
- Workers share one Lua VM. Use `ctx.shared` for state within one worker
  cycle and `_G` only for deliberately batch-wide state. Clear batch-wide
  state in `on_finished`.
- Never perform read → await → write on shared state. Use a synchronous
  segment or `global_store_incr` for atomic counters.
- Pipeline-hook `defer(fn)` is synchronous, hook-scoped, and LIFO. Do not call
  async bindings inside pipeline-hook `defer()` callbacks. Server-handler
  `defer()` callbacks support async bindings. Pipeline `defer()` is unavailable
  in `on_start` and `on_finished`.

## Configuration and testing reminders

- A job requires `name`, `selector`, `fields`, and either `url` or `urls`.
- `urls` enables URL-queue mode and takes precedence over `url`.
- Set `hash_fields` to stable identity fields when other fields change over
  time; otherwise deduplication hashes all fields.
- Tests are global functions named `test_*` in `hooks.lua`, `defer.lua`, or
  `tests.lua`. Local test functions are not discovered.
- Tests run in fresh Lua VMs with temporary databases.
- Use `spyweb.assert_eq` and `spyweb.assert_ne` for project assertions.

## Do not

- Invent bindings, hook names, or fields; check `spyweb-types.lua` and `docs/`.
- Use `db_*` APIs on a `redb` build.
- Store worker-local state in `_G`.
- Edit generated `spyweb-types.lua` or `.luarc.json`.
- Commit secrets.
- Treat `spyweb start` as a test command.

If the offline docs do not answer a behavioral question, use the published
documentation search as a fallback:

    curl -s 'https://docs.spyweb.app/api/search?q=<query>&limit=5'

The discovery file is `https://docs.spyweb.app/llms.txt`.
