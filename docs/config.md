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
- **"God Mode":** Because it follows the HTML5 spec, it will "fix" badly formatted HTML. This can sometimes move elements around (e.g., closing a tag early if it shouldn't be there), which might change the tree structure.
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
- **Pipeline Transparency:** It explicitly shows each stage of the extraction (Fetch → Extract → Hooks) so you can see exactly where an item might be getting dropped.
- **Saves Artifacts:** Just like the config flag, it generates the response HTML and JSON fields in the job's directory for deep inspection.


## Validation Rules

- `name`, `url`, `selector`, and `fields` are required and cannot be blank
- `fields` must contain at least one entry
- `interval` must be greater than `0`
- `url` must be a valid absolute URL
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
- **`before_fetch`**: Handle pagination or custom authentication.
- **`override_fetch`**: Use a headless browser or external API for fetching.
- **`after_fetch`**: Recover from network errors or clean the HTML body.
- **`override_extract`**: Parse JSON/XML APIs instead of HTML.
- **`filter_item`**: Apply complex custom filtering logic per item.

### Safe File I/O
SpyWeb provides safe, non-blocking file operations for hooks. These operations are strictly scoped to the job's directory and feature automatic 10MB rotation with a 5-file history.

- **`log(message)`**: Appends a timestamped line to `hook.log`.
- **`fs_append(filename, content)`**: Appends raw content to a file. Useful for CSV/JSONL exports.
- **`fs_overwrite(filename, content)`**: Replaces a file's content. Ideal for saving `latest_state.json`.

> **Security Note:** Only `.csv`, `.json`, `.jsonl`, `.txt`, and `.log` extensions are allowed. Absolute paths and directory traversal (`../`) are strictly prohibited.

For a full reference of all 9 hook stages and built-in Lua functions like `dump()`, `http_get()`, and `store_set()`, see the [Master Guide](https://docs.spyweb.app/).
