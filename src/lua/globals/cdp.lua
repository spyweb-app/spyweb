-- Injects high-level methods onto a page table returned by browser:attach()
-- This keeps the page table extensible (it's a plain Lua table) while
-- providing Playwright-like helpers for the common cases.
function cdp._log(message)
  message = "[cdp] " .. tostring(message)
  if type(cdp._log_terminal) == "function" then
    pcall(cdp._log_terminal, message)
  end
  if type(log) == "function" then
    pcall(log, message)
  end
end

function cdp._inject_page(page)
  local function fail(message, false_value)
    cdp._log(message)
    if false_value then
      return false, message
    end
    return nil, message
  end

  local function exception_message(result, fallback)
    if type(result) == "table" and result.exceptionDetails then
      local details = result.exceptionDetails
      if details.text then return details.text end
      if details.exception and details.exception.description then return details.exception.description end
      if details.exception and details.exception.value then return tostring(details.exception.value) end
    end
    return fallback
  end

  local function selector_probe_js(selector, opts)
    opts = opts or {}
    local selector_json = json_encode(selector)
    local scroll_json = json_encode(opts.scroll == true)
    return string.format([[
      (function() {
        const selector = %s;
        const shouldScroll = %s;

        function visible(el) {
          const style = window.getComputedStyle(el);
          const rect = el.getBoundingClientRect();
          return style && style.visibility !== "hidden" &&
            style.display !== "none" &&
            rect.width > 0 &&
            rect.height > 0;
        }

        function findInRoot(root) {
          let el = null;
          try {
            el = root.querySelector(selector);
          } catch (e) {
            return { found: false, error: String(e) };
          }
          if (el) {
            return {
              found: true,
              visible: visible(el),
              tagName: el.tagName || null,
              text: (el.innerText || el.textContent || "").slice(0, 200)
            };
          }

          const nodes = root.querySelectorAll ? root.querySelectorAll("*") : [];
          for (const node of nodes) {
            if (node.shadowRoot) {
              const shadowMatch = findInRoot(node.shadowRoot);
              if (shadowMatch.found || shadowMatch.error) return shadowMatch;
            }
          }

          return { found: false };
        }

        function findInDocument(doc) {
          const direct = findInRoot(doc);
          if (direct.found || direct.error) return direct;

          const frames = doc.querySelectorAll("iframe,frame");
          for (const frame of frames) {
            try {
              if (frame.contentDocument) {
                const frameMatch = findInDocument(frame.contentDocument);
                if (frameMatch.found || frameMatch.error) return frameMatch;
              }
            } catch (e) {
              // Cross-origin frames are not queryable from Runtime.evaluate.
            }
          }

          return { found: false };
        }

        const result = findInDocument(document);
        result.url = location.href;
        result.readyState = document.readyState;
        result.scrollY = window.scrollY;
        result.scrollHeight = Math.max(
          document.body ? document.body.scrollHeight : 0,
          document.documentElement ? document.documentElement.scrollHeight : 0
        );

        if (result.found) {
          try {
            const el = document.querySelector(selector);
            if (el && el.scrollIntoView) el.scrollIntoView({ block: "center", inline: "nearest" });
          } catch (e) {}
        } else if (shouldScroll) {
          window.scrollBy(0, Math.max(250, Math.floor(window.innerHeight * 0.8)));
        }

        return result;
      })()
    ]], selector_json, scroll_json)
  end

  -- Navigate and wait for page load
  --   wait_until: raw CDP event name, nil for default, or false to skip waiting
  --   timeout_ms: max ms to wait for the event (default 30000)
  function page:open(url, wait_until, timeout_ms)
    if wait_until == nil then wait_until = "Page.loadEventFired" end
    timeout_ms = timeout_ms or 30000
    self:call("Page.enable", {})
    self:call("Page.navigate", { url = url })
    if wait_until then
      local ok, err = pcall(function()
        self:wait_event(wait_until, timeout_ms)
      end)
      if not ok then
        local state = self:evaluate("document.readyState")
        if state ~= "interactive" and state ~= "complete" then
          return fail("open did not reach " .. tostring(wait_until) .. ": " .. tostring(err), true)
        end
        cdp._log("open missed " .. tostring(wait_until) .. " but document.readyState is " .. tostring(state))
      end
    end
    return true
  end

  -- Poll the DOM until a CSS selector exists
  --   opts: number timeout_ms, or table { timeout_ms, poll_ms, scroll, visible }
  function page:wait_for_selector(selector, opts)
    if type(opts) == "number" then
      opts = { timeout_ms = opts }
    else
      opts = opts or {}
    end
    local timeout_ms = opts.timeout_ms or opts.timeout or 10000
    local poll_ms = opts.poll_ms or 100
    local scroll = opts.scroll
    if scroll == nil then scroll = true end
    local require_visible = opts.visible == true
    local start = os.clock() * 1000
    local last = nil
    while true do
      local result = self:call("Runtime.evaluate", {
        expression = selector_probe_js(selector, { scroll = scroll }),
        returnByValue = true
      })
      last = result.result.value
      if last == true then
        return true, last
      end
      if type(last) == "table" and last.error then
        return fail("Invalid selector '" .. selector .. "': " .. last.error, true)
      end
      if type(last) == "table" and last.found and (not require_visible or last.visible == true) then
        return true, last
      end
      if (os.clock() * 1000 - start) > timeout_ms then
        local detail = ""
        if type(last) == "table" then
          detail = string.format(
            " (url=%s, readyState=%s, scrollY=%s, scrollHeight=%s, found=%s, visible=%s)",
            tostring(last.url),
            tostring(last.readyState),
            tostring(last.scrollY),
            tostring(last.scrollHeight),
            tostring(last.found),
            tostring(last.visible)
          )
        end
        return fail("Timeout waiting for selector: " .. selector .. detail, true)
      end
      cdp.sleep(poll_ms)
    end
  end

  function page:wait_for_url(pattern, timeout_ms)
    timeout_ms = timeout_ms or 10000
    local start = os.clock() * 1000
    while true do
      local url = self:evaluate("location.href")
      if string.find(url, pattern) then
        return url
      end
      if (os.clock() * 1000 - start) > timeout_ms then
        return fail("Timeout waiting for URL pattern: " .. pattern .. " (current=" .. tostring(url) .. ")", false)
      end
      cdp.sleep(100)
    end
  end

  function page:wait_for_response(predicate, timeout_ms)
    timeout_ms = timeout_ms or 30000
    self:call("Network.enable", {})
    local ok, event_or_err = pcall(function()
      return self:wait_event("Network.responseReceived", timeout_ms, function(params)
      if predicate == nil then return true end
      return predicate(params)
    end)
    end)
    if ok then return event_or_err end
    local err = tostring(event_or_err)
    if string.find(err, "Timeout waiting", 1, true) then
      return fail("Timeout waiting for response after " .. tostring(timeout_ms) .. "ms", false)
    end
    error(event_or_err)
  end

  function page:wait_for_idle(timeout_ms, quiet_ms)
    timeout_ms = timeout_ms or 10000
    quiet_ms = quiet_ms or 500
    local start = os.clock() * 1000
    local last_busy = start
    while true do
      local state = self:evaluate([[
        (function() {
          const entries = performance.getEntriesByType("resource");
          const recent = entries.filter((e) => e.responseEnd === 0 || performance.now() - e.responseEnd < 250);
          return { readyState: document.readyState, recent: recent.length };
        })()
      ]])
      local now = os.clock() * 1000
      if state.readyState ~= "complete" or state.recent > 0 then
        last_busy = now
      end
      if now - last_busy >= quiet_ms then
        return true
      end
      if now - start > timeout_ms then
        return fail("Timeout waiting for page idle", true)
      end
      cdp.sleep(100)
    end
  end

  function page:scroll(opts)
    opts = opts or {}
    local max_scrolls = opts.max_scrolls or 20
    local step = opts.step or nil
    local delay_ms = opts.delay_ms or 250
    local until_selector = opts.until_selector
    local until_bottom = opts.until_bottom
    if until_bottom == nil then until_bottom = true end

    for i = 1, max_scrolls do
      if until_selector then
        local found = self:evaluate(string.format("document.querySelector(%s) !== null", json_encode(until_selector)))
        if found then return true end
      end

      local info = self:evaluate(string.format([[
        (function() {
          const before = window.scrollY;
          const step = %s || Math.max(250, Math.floor(window.innerHeight * 0.8));
          window.scrollBy(0, step);
          const maxY = Math.max(
            document.body ? document.body.scrollHeight : 0,
            document.documentElement ? document.documentElement.scrollHeight : 0
          ) - window.innerHeight;
          return { before: before, after: window.scrollY, maxY: maxY };
        })()
      ]], step and tostring(step) or "null"))

      cdp.sleep(delay_ms)
      if until_bottom and info.after >= info.maxY then
        return true
      end
    end

    return fail("Scroll finished without reaching requested condition", true)
  end

  -- Get the outer HTML of the page
  function page:content()
    local result = self:call("Runtime.evaluate", {
      expression = "document.documentElement.outerHTML",
      returnByValue = true
    })
    return result.result.value
  end

  -- Evaluate JavaScript and return the value
  function page:evaluate(js)
    local result = self:call("Runtime.evaluate", {
      expression = js,
      returnByValue = true
    })
    return result.result.value
  end

  function page:screenshot(path, opts)
    opts = opts or {}
    self:call("Page.enable", {})
    local params = {
      format = opts.format or "png",
      fromSurface = opts.fromSurface ~= false
    }
    if opts.quality then params.quality = opts.quality end
    if opts.full_page or opts.fullPage then
      local metrics = self:call("Page.getLayoutMetrics", {})
      local size = metrics.contentSize
      params.captureBeyondViewport = true
      params.clip = {
        x = 0,
        y = 0,
        width = size.width,
        height = size.height,
        scale = 1
      }
    end
    local result = self:call("Page.captureScreenshot", params)
    cdp._write_base64(path, result.data)
    return path
  end

  function page:set_extra_headers(headers)
    self:call("Network.enable", {})
    self:call("Network.setExtraHTTPHeaders", { headers = headers or {} })
  end

  function page:set_user_agent(user_agent, opts)
    opts = opts or {}
    local params = {
      userAgent = user_agent,
      acceptLanguage = opts.accept_language,
      platform = opts.platform
    }
    self:call("Network.setUserAgentOverride", params)
  end

  function page:cookies(urls)
    self:call("Network.enable", {})
    if urls then
      return self:call("Network.getCookies", { urls = urls }).cookies
    end
    local result = self:call("Network.getAllCookies", {})
    return result.cookies
  end

  function page:set_cookies(cookies)
    self:call("Network.enable", {})
    self:call("Network.setCookies", { cookies = cookies or {} })
  end

  function page:block_resources(types_or_patterns)
    self:call("Network.enable", {})
    local patterns = {}
    local by_type = {
      image = { "*.png", "*.jpg", "*.jpeg", "*.gif", "*.webp", "*.avif", "*.svg", "*.ico" },
      font = { "*.woff", "*.woff2", "*.ttf", "*.otf", "*.eot" },
      media = { "*.mp4", "*.webm", "*.mp3", "*.wav", "*.ogg", "*.avi", "*.mov" },
      stylesheet = { "*.css" },
      script = { "*.js" }
    }
    for _, item in ipairs(types_or_patterns or {}) do
      if string.sub(item, 1, 1) == "*" or string.find(item, "://") then
        table.insert(patterns, item)
      elseif by_type[item] then
        for _, pattern in ipairs(by_type[item]) do
          table.insert(patterns, pattern)
        end
      else
        table.insert(patterns, item)
      end
    end

    if #patterns > 0 then
      self:call("Network.setBlockedURLs", { urls = patterns })
    end
    return patterns
  end

  -- Click a CSS selector
  function page:click(selector, opts)
    opts = opts or {}
    if opts.real then
      local result = self:call("Runtime.evaluate", {
        expression = string.format([[
        (function() {
          const el = document.querySelector(%s);
          if (!el) throw new Error("selector not found: " + %s);
          el.scrollIntoView({ block: "center", inline: "nearest" });
          const rect = el.getBoundingClientRect();
          return {
            x: rect.left + rect.width / 2,
            y: rect.top + rect.height / 2
          };
        })()
      ]], json_encode(selector), json_encode(selector)),
        returnByValue = true
      })
      if result.exceptionDetails then
        return fail("click failed: " .. exception_message(result, "selector not found: " .. selector), true)
      end
      local box = result.result.value
      self:call("Input.dispatchMouseEvent", { type = "mouseMoved", x = box.x, y = box.y })
      self:call("Input.dispatchMouseEvent", { type = "mousePressed", x = box.x, y = box.y, button = "left", clickCount = 1 })
      self:call("Input.dispatchMouseEvent", { type = "mouseReleased", x = box.x, y = box.y, button = "left", clickCount = 1 })
      return true
    end

    local result = self:call("Runtime.evaluate", {
      expression = string.format([[
      (function() {
        const el = document.querySelector(%s);
        if (!el) throw new Error("selector not found: " + %s);
        el.click();
      })()
    ]], json_encode(selector), json_encode(selector)),
      returnByValue = true
    })
    if result.exceptionDetails then
      return fail("click failed: " .. exception_message(result, "selector not found: " .. selector), true)
    end
    return true
  end

  -- Type text into a selector
  function page:type(selector, text, opts)
    opts = opts or {}
    if opts.real then
      local ok, err = self:click(selector, { real = true })
      if not ok then return false, err end
      self:call("Input.insertText", { text = text })
      return true
    end

    local result = self:call("Runtime.evaluate", {
      expression = string.format([[
      (function() {
        const el = document.querySelector(%s);
        if (!el) throw new Error("selector not found: " + %s);
        el.focus();
        el.value = %s;
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      })()
    ]], json_encode(selector), json_encode(selector), json_encode(text)),
      returnByValue = true
    })
    if result.exceptionDetails then
      return fail("type failed: " .. exception_message(result, "selector not found: " .. selector), true)
    end
    return true
  end

  -- Return page so callers can chain or store
  return page
end

-- Standalone helpers for backward compat and non-page use
function cdp_navigate(browser, url, wait_until)
  local page = browser:attach()
  return page:open(url, wait_until)
end

function cdp_get_html(page)
  return page:content()
end

function cdp_wait_for_selector(browser, selector, timeout_ms)
  local page = browser.attach and browser:attach() or browser
  return page:wait_for_selector(selector, timeout_ms)
end

function cdp_fulfill_request(browser, request_id, body_html)
  local encoded = cdp_base64_encode(body_html)
  browser:call("Fetch.fulfillRequest", {
    requestId = request_id,
    responseCode = 200,
    responseHeaders = {
      { name = "Content-Type", value = "text/html; charset=utf-8" }
    },
    body = encoded
  })
end

function cdp_base64_encode(data)
  local b = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
  return ((data:gsub(".", function(x)
    local r, byte = "", string.byte(x)
    for i = 8, 1, -1 do r = r .. (byte % 2 ^ i - byte % 2 ^ (i - 1) > 0 and "1" or "0") end
    return r
  end) .. "0000"):gsub("%d%d%d?%d?%d?%d?", function(x)
    if #x < 6 then return "" end
    local c = 0
    for i = 1, 6 do c = c + (x:sub(i, i) == "1" and 2 ^ (6 - i) or 0) end
    return b:sub(c + 1, c + 1)
  end) .. ({ "", "==", "=" })[#data % 3 + 1])
end
