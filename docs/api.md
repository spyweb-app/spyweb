# REST API

SpyWeb runs a built-in web server on `127.0.0.1:7979` which serves both the dashboard UI and a JSON API for querying your scraped records.

## Authentication

If the `SPYWEB_API_KEY` environment variable is set, all `/api/*` endpoints require authentication. You must provide your key in the `X-SpyWeb-Key` header.

```bash
curl -H "X-SpyWeb-Key: your_secret_key" http://127.0.0.1:7979/api/jobs
```

If the environment variable is not set, the API remains open and no header is required.

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /` | HTML records viewer (The UI dashboard) |
| `GET /api/records?job_id=<id>` | JSON records for a job |
| `GET /api/records?job_id=<id>&limit=50&after=<timestamp>` | Paginated records |
| `GET /api/jobs` | List all configured jobs |


## 💡 Bring Your Own UI
The default admin dashboard is located in `ui/index.html`. Because spyweb serves this file dynamically from the filesystem on every request rather than embedding it into the binary or caching it in memory, you can easily build your own custom dashboard using **Vue, React, Svelte**, or vanilla JS. 

As long as your build process outputs an `index.html` into the `ui/` directory, spyweb will serve it instantly — no restart required!

## Webhook Payload

When a job finds new items, it can POST a JSON payload to a configured webhook URL. 

### Default JSON Structure
```json
{
  "job_name": "Product Tracker",
  "item_count": 2,
  "items": [
    {
      "fields": {
        "title": "Item A",
        "price": "$49.99",
        "link": "https://example.com/a"
      },
      "matches": ["sale"]
    },
    {
      "fields": {
        "title": "Item B",
        "price": "$20.00",
        "link": "https://example.com/b"
      },
      "matches": ["deal"]
    }
  ]
}
```

> Use the `before_webhook(payload)` Lua hook to completely reshape this JSON before it is sent. This allows you to match specific API formats like **Discord embeds**, **Slack blocks**, or **Pushover** notifications without an external middleware.
