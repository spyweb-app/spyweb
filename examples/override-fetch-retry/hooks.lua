-- override_fetch with retry, proxy failover, jitter, timeout, and size limit

-- Drop this function into your hooks.lua to replace the built-in HTTP
-- client with a custom multi-proxy fetching loop that retries on failure.

-- NOTE: override_fetch runs every time the job polls. When all proxies are
-- down the 6-attempt retry loop fires every cycle. Use a global flag for
-- transient state (per worker lifetime) or a database/store for persistent
-- state (across worker restarts) if you want to short-circuit after one
-- full failure cycle.

function override_fetch(request, ctx)
    local max_retries = 3
    local retry_delay = 2          -- seconds between retry waves
    local request_timeout = 15      -- seconds per individual request
    --  define your proxies or load from database dynamically instead
    local proxies = {
        "http://user:pass@proxy1.example.com:8080",
        "http://user:pass@proxy2.example.com:8080",
    }

    for attempt = 1, max_retries do   -- outer retry wave: up to 3 full rounds
        for idx, proxy_url in ipairs(proxies) do  -- try each proxy; first success wins
            local res, err = http_request({
                url = request.url,
                method = request.method,
                headers = request.headers,
                proxy = proxy_url,
                timeout = request_timeout,
                max_body_size = 5,   -- limit response to 5 MB
            })

            if res then -- proxy worked → short-circuit out of the whole function
                log(string.format(
                    "proxy %d OK — %d bytes in %dms",
                    idx, res.size, res.time_ms
                ))
                return res -- pass the resposne to next pipeline (after_fetch)
            end  

            log(string.format(
                "attempt %d / proxy %d failed: %s (%s)",
                attempt, idx, err.error, err.kind
            ))
        end  -- both proxies failed this attempt

        -- jittered delay (±500ms) before next retry wave; skip after final attempt
        if attempt < max_retries then
            local jitter = math.random(-500, 500) / 1000
            sleep((retry_delay + jitter) * 1000)
        end
    end  -- all 6 calls (3 attempts × 2 proxies) exhausted

    return { error = "all retries exhausted" }  -- fall through: every attempt failed
end
