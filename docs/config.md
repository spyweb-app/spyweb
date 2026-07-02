# Configuration Reference

Jobs in spyweb are configured via TOML. You can place a single `jobs.toml` in the root directory (for multiple basic jobs) or create a directory structure like `jobs/my-job/config.toml`.

SpyWeb validates config statically before startup or reload. It checks required fields, duplicate job names and IDs, duplicate field names, URL shape for `url` / enabled `webhook` / enabled `proxy`, `interval > 0`, and that `search_fields` / `hash_fields` only reference extracted field names.

> **Note on Versioning:** You can check your current engine version and active Lua runtime (Luau vs Lua 5.4) using the `./spyweb version` command.

## Job Config (`config.toml`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | **required** | Display name for the job |
| `url` | string | **required** | Target URL to scrape |
| `selector` | string | **required** | CSS selector for item containers |
| `fields` | array | **required** | Fields to extract (see below) |
| `enabled` | bool | `true` | Enable/disable this job |
| `interval` | u32 | `600` | Seconds between scrape runs |
| `keywords` | string[] | *none* | Filter items matching any keyword |
| `search_fields` | string[] | *none* | Limit keyword search to specific fields |
| `debug` | bool | `false` | Save raw HTML and extracted JSON for debugging |
| `headers` | table | *none* | Custom HTTP headers |
| `workers` | u32 | `1` | Per-job worker concurrency |
| `urls` | string[] | *none* | Multiple entry URLs (overrides `url` for multi-page entry) |
| `proxy` | table | *none* | Proxy configuration |
| `webhook` | table | *none* | Webhook configuration |
| `notification` | table | *auto* | Desktop notification settings |
| `hash_fields` | string[] | *all* | Fields used for deduplication hash |
 
## The Dual Parser System
 
SpyWeb uses a unique hybrid extraction engine designed to handle the "messy" reality of the modern web. Instead of restricting extraction to a single parsing mode, SpyWeb attempts to be literal first and compliant second.
 
### 1. Raw Parser (Primary)
The Raw Parser is the default engine. It is a high-performance, non-standard string scanner.
- **Integrity:** It does not "fix" the source HTML. If a site has illegal nesting (like an `<a>` inside another `<a>`), the Raw Parser preserves it exactly as written.
- **Performance:** It is extremely fast because it avoids building a complex standard-compliant tree.
- **Limitation:** It only supports simple CSS selectors (tags, classes, IDs, and direct children `>`). It **cannot** handle complex pseudo-selectors like `:nth-child`, `:not()`, or `:has()`.
 
### 2. DOM Parser (Fallback)
If the Raw Parser finds **zero items** for the main `selector`, SpyWeb automatically switches to the DOM Parser.
- **Compliance:** It uses a full HTML5-compliant engine. It "understands" complex structures and allows for advanced CSS selectors.
- **Standard Parsing Compliance:** Because it follows the HTML5 spec, it will resolve and "fix" badly formatted HTML. This can sometimes restructure elements (e.g., closing an unclosed tag early), which might alter the resulting DOM tree.
- **Usage:** This mode is triggered automatically if the Raw Parser fails or if a selector is used that the Raw Parser does not recognize.
 
### Selector Tips: Self-Selection
In both parsers, data can be extracted from the "item" element itself.
- If the `selector` is `.item-container`, the `href` can be extracted by using `link:.item-container@href`.
- An **empty selector** in shorthand also targets the root: `link:@href`.
 
| Parser | Strength | Best For |
| :--- | :--- | :--- |
| **Raw** | Literal Accuracy | Sites with broken HTML, maximum speed. |
| **DOM** | Complex Selection | Sites requiring advanced CSS logic (like `:has`). |
 
### Debugging & Troubleshooting
If fields are returning empty or unexpected values, enable `debug = true` in the job configuration. The generated JSON output (e.g., `job-name-fields.json`) includes metadata for every item:
- **`parser`**: Indicates if the entire item was found via `raw` or `dom`.
- **`field_parsers`**: Shows which engine was used for each specific field.
 
If a field is empty but the parser is `raw`, the CSS selector is likely incorrect for the literal source. If the parser is `dom-fallback`, the selector was too complex for the Raw Parser, and the DOM Parser also failed to find a match (potentially due to "God Mode" restructuring).

### The Debug Command
While the `debug = true` flag saves files during normal scheduled runs, you can also trigger a **manual one-shot debug run** via the CLI:

```bash
./spyweb debug "Job Name"
```

This command is ideal for iterative development because:
- **Ignores `enabled` flag:** It will run the job even if it is set to `enabled = false` in your config.
- **Instant Feedback:** It prints the final extracted items and their fields directly to your terminal.
- **Pipeline Transparency:** It explicitly shows the core stages of the pipeline (Fetch → Extract → Hook filtering, up to `before_store`) so you can see exactly where an item might be getting dropped, while safely bypassing the store, notification, and webhook phases.
- **Saves Artifacts:** Just like the config flag, it generates the response HTML and JSON fields in the job's directory for deep inspection.


## Validation Rules

- `name`, `url`, `selector`, and `fields` are required and cannot be blank
- `fields` must contain at least one entry
- `interval` must be greater than `0`
- `workers` must be greater than `0` if set
- `url` must be a valid absolute URL
- `urls` must contain at least one valid absolute URL if set
- `webhook.url` must be a valid absolute URL when webhook is enabled
- `proxy.urls` must be valid absolute URLs when proxy is enabled
- field names must be unique within a job
- `search_fields` and `hash_fields` must reference existing extracted field names
- job `name` values must be unique across the full config set
- normalized job IDs (`name` converted through `id()`) must also be unique

## Field Syntax

```toml
# Shorthand — "name:selector" (defaults to text content)
fields = ["title:h2", "link:a@href"]

# Full form — explicit selector and attribute
fields = [
  { name = "title", selector = "h2", att = "text" },
  { name = "link", selector = "a", att = "href" },
]
```

## Proxy Config

```toml
[proxy]
enabled = true
rotate = "RoundRobin"  # or "Sticky", "Random"
urls = [
  "socks5://proxy1:1080",
  "http://proxy2:8080",
]
```
## Deduplication (`hash_fields`)

By default, SpyWeb hashes **all extracted fields** to determine if an item is "new." This is the most reliable default because it ensures you catch every update (like a price change or a modified description).

However, if you are explicitly scraping **volatile data** that changes independently of the item itself, you might receive unwanted duplicate. Common examples include:
- **`date_posted`**: Changes from "2 hours ago" to "3 hours ago" even if the content is the same.
- **`view_count`**: Increments every time the page is refreshed.

In these specific scenarios, you can use `hash_fields` to specify only the fields that uniquely identify the item (like a **URL** or **Product ID**). This ensures that an item is only considered "new" and stored in the database if its core identity changes—allowing SpyWeb to ignore changes in those volatile fields:

```toml
# Use the link as the unique identifier for deduplication
hash_fields = ["link"]
```

**Note:** If all specified `hash_fields` are empty for a particular item, SpyWeb will automatically fall back to hashing all fields to ensure no items are accidentally missed.
## Webhook Config

```toml
[webhook]
enabled = true
url = "https://your-webhook.example.com/endpoint"
headers = { "Authorization" = "Bearer your-token" }
```

## Notification Templates

```toml
[notification]
enabled = true
timeout = 5000
title = "Found {item_count} new items from {job_name}"
body = """
Title: {title}
Link: {link}
Keywords: {matches}
"""
```

Available tags: `{job_name}`, `{url}`, `{item_count}`, `{timestamp}`, `{matches}`, `{match_count}`, and any extracted field name.

> ⚠️ **Note on Notification Limits:** Most desktop operating systems restrict notification bodies to about **4 lines** before truncating them. If your template body uses multiple fields per item (like title + link + keywords), you will likely only see a single record in the pop-up. If you use a leaner template (e.g., just the `{title}`), you can often fit 2-3 scraped records in a single desktop notification before it gets cut off.

## Lua Hooks (`hooks.lua`)

For advanced workflows, you can place a `hooks.lua` file in the same directory as your `config.toml`. SpyWeb will automatically detect and run these hooks during the scraping pipeline.

Common use cases for hooks:
- **`before_fetch(request, ctx)`**: Handle pagination, custom authentication, change HTTP method, override timeout/proxy/max_body_size per-request. See `docs/index.html` for full field reference.
- **`override_fetch(request, ctx)`**: Use a headless browser or external API for fetching.
- **`after_fetch(result, ctx)`**: Recover from network errors or clean the HTML body.
- **`override_extract(response, ctx)`**: Parse JSON/XML APIs instead of HTML.
- **`after_extract(items, ctx)`**: Transform or enrich extracted items.
- **`filter_item(item, ctx)`**: Apply complex custom filtering logic per item.
- **`before_store(items, ctx)`**: Intercept items before database insertion.
- **`before_notify(items, ctx)`**: Modify items before desktop notification.
- **`before_webhook(payload, ctx)`**: Reshape the webhook JSON payload.
- **`on_finished()`**: Run after every complete job iteration (all workers done, before sleep). Receives no arguments.

All hooks except `on_finished()` receive the per-cycle `ctx` table, which provides:
- **`ctx.worker_id`** — 1-based index for multi-worker jobs (always `1` for single-worker)
- **`ctx.shared`** — a table for passing data between hooks within the same cycle

### Safe File I/O
SpyWeb provides safe, non-blocking file operations for hooks. Writes go to the job's directory. Reads scan the job's directory first, then fall back to the shared `./shared/` folder for cross-job data sharing.

- **`log(message)`**: Appends a timestamped line to `hooks.log`.
- **`fs_append(filename, content)`**: Appends raw content to a file. Useful for CSV/JSONL exports.
- **`fs_overwrite(filename, content)`**: Replaces a file's content. Ideal for saving `latest_state.json`.
- **`fs_read(filename)`**: Reads a text file and returns its content as a string, or `nil` if the file does not exist. Falls back to `./shared/` if not found locally.
- **`fs_read_binary(filename)`**: Reads any file (including binary files like images) from the job directory. Falls back to `./shared/` if not found locally. Returns binary-safe Lua string; returns `nil` if file doesn't exist.
 
> **Security Note:** Writes (`fs_append`, `fs_overwrite`) and text reads (`fs_read`) are restricted to `.csv`, `.json`, `.jsonl`, `.txt`, and `.log` extensions. **`fs_read_binary`** additionally supports media and assets including `.png`, `.jpg`, `.jpeg`, `.gif`, `.svg`, `.webp`, `.bmp`, `.ico`, `.pdf`, `.zip`, `.woff`, `.woff2`, `.ttf`, `.otf`, etc. Absolute paths and directory traversal (`../`) are strictly prohibited. Paths are resolved relative to the job directory. Use `shared/` prefix to read/write from the project root's shared folder.

### Global Functions Reference

All globals listed below are available inside any hook function.

| Function | Async | Description |
|----------|-------|-------------|
| `http_get(url, [headers])` | ✅ | HTTP GET request. Returns `(res, err)` — two-return pattern. |
| `http_post(url, body, [headers])` | ✅ | HTTP POST request. Returns `(res, err)` — two-return pattern. |
| `http_request({ method, url, body?, headers?, proxy?, timeout?, max_body_size? })` | ✅ | Generic HTTP request with optional proxy/timeout/max_body_size. Returns `(res, err)`. |
| `http_multipart(url, fields, [headers])` | ✅ | Multipart file uploads. Returns `(res, err)` — two-return pattern. |
| `sleep(ms)` | ✅ | Sleep for N milliseconds |
| `notify(title, body, [timeout])` | ✅ | Send desktop notification |
| `log(message)` | ✅ | Append timestamped line to `hooks.log` |
| `fs_append(path, content)` | ✅ | Append to a file in the job directory |
| `fs_overwrite(path, content)` | ✅ | Overwrite a file in the job directory |
| `fs_read(path)` | ✅ | Read a text file (returns string or nil), falls back to `shared/` |
| `fs_read_binary(path)` | ✅ | Read a binary file, falls back to `shared/` |
| `store_set(key, value)` | ✅ | Set per-job storage key |
| `store_get(key)` | ✅ | Get per-job storage value |
| `store_delete(key)` | ✅ | Delete a per-job storage key |
| `global_store_set(key, value)` | ✅ | Set global (cross-job) storage key |
| `global_store_get(key)` | ✅ | Get global (cross-job) storage value |
| `global_store_incr(key, default, delta)` | ✅ | Atomically increment a global counter |
| `global_store_delete(key)` | ✅ | Delete a global storage key |
| `json_encode(val)` | ❌ | Encode a Lua value to JSON string |
| `json_decode(str)` | ❌ | Decode a JSON string to Lua value (10MB input limit) |
| `env_get(key)` | ❌ | Read an environment variable |
| `defer(fn)` | ❌ | Register hook-scoped cleanup callback |
| `require(name)` | ❌ | Load a Lua module from the job directory or project root (Luau only) |
| `dump(value)` | ❌ | Pretty-print a Lua value (debugging) |
| `copy(table)` | ❌ | Shallow copy of a Lua table |
| `deep_copy(table)` | ❌ | Deep copy of a Lua table |

For usage examples and detailed hook stage documentation, see the [Master Guide](https://docs.spyweb.app/).
