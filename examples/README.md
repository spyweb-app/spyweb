# spyweb Examples

This directory contains example job configurations demonstrating different spyweb features.

## How It Works

spyweb loads jobs from two places:

1. **`jobs.toml`** (root) — Multiple simple jobs in one file
2. **`jobs/` directory** — Each subdirectory is a job with `config.toml` + optional `hooks.lua`

To use any example, copy the directory into your `jobs/` folder:

```bash
# From project root
cp -r examples/paginated-scraper jobs/
```

Then start spyweb — it auto-discovers the new job:

```bash
# Linux / macOS
./spyweb start

# Windows
spyweb.exe start
```

Edit any config file while spyweb is running and it will **hot-reload** automatically.

---

## Examples

### [`basic-scraper/`](basic-scraper/)

A minimal config-only job with no Lua hooks. Uses built-in keyword filtering.

**What it does:**
- Scrapes a page every 5 minutes
- Extracts `title` and `link` fields from each `.athing` container
- Filters items matching `rust`, `go`, `backend`, or `infrastructure`
- Sends desktop notifications for new matches

**How CSS Selectors Map to Config:**

Given this target HTML on a webpage:
```html
<div class="job-list">
    <!-- Item 1 -->
    <div class="athing">
        <h2 class="titleline"><a href="https://example.com/job/1">Senior Rust Engineer</a></h2>
        <span class="company">TechCorp</span>
    </div>
    <!-- Item 2 -->
    <div class="athing">
        <h2 class="titleline"><a href="https://example.com/job/2">Go Developer</a></h2>
        <span class="company">StartupInc</span>
    </div>
</div>
```

Your `config.toml` maps perfectly to it:
```toml
# 1. Target the repeating container for each item
selector = ".athing"

# 2. Extract fields RELATIVE to that container
fields = [
  "title:.titleline > a",       # Extracts the text inside the link ("Senior Rust Engineer")
  "link:.titleline > a@href",   # Extracts the 'href' attribute ("https://example.com/job/1")
  "company:.company"            # Extracts the text ("TechCorp")
]

# 3. (Optional) Filter the results
keywords = ["rust", "go", "backend", "infrastructure"]
```

**Key concepts:** Field shorthand syntax (`"title:h2"`), keyword filtering, notification templates.

---

### [`paginated-scraper/`](paginated-scraper/)

Demonstrates **Lua-powered pagination** — the URL changes each run by appending `?page=N`.

**What it does:**
- Uses a Lua `page` counter that persists across runs while the process stays alive
- Cycles through pages 1–5, then resets
- Checks HTTP status before extracting
- Filters items by price (drops items over $1000)
- Uppercases titles before storage
- Caps notifications to 5 items max

**Key concepts:**

| Hook | Purpose |
|------|---------|
| `before_fetch` | Append `?page=N` to URL, increment counter |
| `after_fetch` | Skip extraction on failed requests or non-200 responses |
| `after_extract` | Log how many items were found |
| `filter_item` | Drop empty titles, filter by price, mutate fields |
| `before_store` | Skip if no items remain |
| `before_notify` | Cap notification to first 5 items |

**How pagination works:**

```lua
function before_fetch(request)
    page = page or 1
    request.url = request.url .. "?page=" .. page
    page = page + 1
    if page > 5 then page = 1 end
    -- You can use store_set("page", ...) instead if you want
    -- the cursor to survive hot-reload and restart.
    return request
end
```

Lua globals are fine here because restarting at page 1 is usually harmless. Use `store_*` if you want the cursor to survive hot-reload and process restart.

---

### [`lua-filtered/`](lua-filtered/)

Shows how to completely replace the built-in keyword filter with custom Lua logic.

**What it does:**
- Maintains a blocked companies list
- Drops job postings with "intern" in the title
- Only keeps items mentioning specific technologies
- Normalizes salary fields for consistent display
- Silences notifications unless 2+ new items found

**Key concept:** When `filter_item()` exists in your hook, the built-in keyword filter is **skipped entirely** — even if you have `keywords` in your config.

---

### [`js-rendering/`](js-rendering/)

Demonstrates how to render JavaScript-heavy or client-side rendered pages using the built-in Chrome DevTools Protocol (CDP) module.

**What it does:**
- Overrides the default HTTP client using `override_fetch`
- Launches a local headless Chromium browser (or uses an existing one)
- Navigates to the target page and waits for a specific CSS selector to appear
- Captures a screenshot of the page and extracts the fully rendered HTML DOM

---

### [`hybrid-recovery/`](hybrid-recovery/)

Demonstrates a hybrid automation pattern to bypass bot detection. If a headless scrape run fails or hits a CAPTCHA/block page, it automatically spawns a visible browser window to allow human intervention.

**What it does:**
- Runs headlessly for normal iterations
- Checks for blocker elements or CAPTCHAs in `override_fetch`
- Closes the headless browser and spawns a visible browser process on detection
- Uses `notify()` to alert the operator
- Polls for a successful page state before closing the browser and handing the rendered HTML back to the pipeline

---

### [`external-db-exit/`](external-db-exit/)

Demonstrates how to bypass SpyWeb's internal database entirely and send extracted items directly to your own external API or database.

**What it does:**
- Uses `before_store` to intercept items before they enter the internal database
- Formats the items to match a custom schema
- Sends the payload to an external endpoint via `http_post`
- Returns `nil` to drop the items from SpyWeb's pipeline, preventing internal storage or notifications from triggering
- Includes a companion `defer.lua` file to demonstrate cycle-scoped orchestration hooks (`on_success`, `on_error`, `on_finally`)

---

## Pipeline Stage Reference

Every scrape run goes through these stages in order:

```
before_fetch(request)         ← modify URL, headers, or return nil to skip
    ↓
override_fetch(request)       ← bypass built-in HTTP client
    ↓
[HTTP fetch]                  ← automatic
    ↓
after_fetch(fetch_result)     ← inspect request/response/error, mutate response.body, or return nil
    ↓
override_extract(response)    ← bypass built-in CSS extraction
    ↓
[CSS extraction]              ← automatic (raw parser → DOM fallback)
    ↓
after_extract(items)          ← batch filter/modify all items at once
    ↓
filter_item(item)             ← per-item filter (OR keyword_filter, not both)
    ↓
before_store(items)           ← last chance before DB insert
    ↓
[dedup + insert]              ← automatic, atomic
    ↓
before_notify(items)          ← reshape or silence notifications
    ↓
before_webhook(payload)       ← reshape or silence webhook POSTs
    ↓
[notify + webhook]            ← automatic
```

### Return Values

| Return | Effect |
|--------|--------|
| Return the value | Continue with (possibly modified) data |
| Return `nil` or `false` | Drop/skip (behavior depends on the hook) |
| Hook errors | Logged and skipped — never kills the job |
| Hook not defined | No-op passthrough |

### Important Behaviors

- **`filter_item` vs keywords** — They are mutually exclusive. If `filter_item()` exists, the built-in keyword filter does not run.
- **`before_notify`** — Items are already stored when this runs. Dropping here only silences the notification.
- **Lua storage** — `store_*` is job-scoped persistent state. `global_store_*` is shared across jobs. Both survive hot-reload and restart.
- **Globals persist** — Plain Lua globals survive across runs only while the job process stays alive. Hot-reload resets them.
