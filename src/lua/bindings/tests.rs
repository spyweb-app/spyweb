use super::*;
use crate::services::db::Db;
use crate::services::server::types;
use mlua::Lua;
use std::sync::Arc;

struct TestDb {
    _dir: tempfile::TempDir,
    db: Option<Arc<Db>>,
}

impl TestDb {
    fn new(name: &str) -> Self {
        let dir = tempfile::TempDir::with_prefix(name).unwrap();
        let path = dir.path().join("db.sqlite");
        let path_str = path.to_str().unwrap().to_string();
        let db = Arc::new(Db::open(&path_str).unwrap());
        Self {
            _dir: dir,
            db: Some(db),
        }
    }

    fn db(&self) -> Arc<Db> {
        Arc::clone(self.db.as_ref().unwrap())
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        self.db = None;
    }
}

#[test]
fn test_http_bindings() {
    let server = types::MockServer::start(|request| {
        if request.method == "GET" {
            if request.header("X-Custom-Auth") == Some("secret-123") {
                types::Response::text("GET-OK-AUTH!")
            } else {
                types::Response::text("GET-OK!")
            }
        } else if request.method == "HEAD" {
            types::Response::text("HEAD-OK!").with_header("X-Test", "head-check")
        } else if request.method == "POST"
            && request
                .header("Content-Type")
                .is_some_and(|ct| ct.starts_with("multipart/form-data"))
        {
            let body = request.body_bytes().unwrap_or_default().to_vec();
            let body_str = String::from_utf8_lossy(&body);
            if body_str.contains("my-file-content") && body_str.contains("my-caption") {
                types::Response::text("MULTIPART-OK!")
            } else {
                types::Response::text("MULTIPART-RECEIVED")
            }
        } else if request.method == "POST" {
            let body = request.body_bytes().unwrap_or_default().to_vec();
            if request.header("Content-Type") == Some("application/json") {
                types::Response::text("POST-OK-JSON!")
            } else if body == b"my-body" {
                types::Response::text("POST-OK!")
            } else if body == b"binary-data\x00\xff" {
                types::Response::text("BINARY-OK!")
            } else {
                types::Response::text("POST-UNKNOWN!")
            }
        } else {
            types::Response::empty_404()
        }
    });

    smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, None, "test").unwrap();

        let code_get = format!(r#"http_get("http://127.0.0.1:{}/")"#, server.port());
        let res: mlua::Table = lua.load(&code_get).eval_async().await.unwrap();
        assert_eq!(res.get::<String>("body").unwrap(), "GET-OK!");
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        let code_get_headers = format!(
            r#"http_get("http://127.0.0.1:{}/", {{ ["X-Custom-Auth"] = "secret-123" }})"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_get_headers).eval_async().await.unwrap();
        assert_eq!(res.get::<String>("body").unwrap(), "GET-OK-AUTH!");
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        let code_post = format!(
            r#"http_post("http://127.0.0.1:{}/", "my-body")"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_post).eval_async().await.unwrap();
        assert_eq!(res.get::<String>("body").unwrap(), "POST-OK!");
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        let code_post_headers = format!(
            r#"http_post("http://127.0.0.1:{}/", "my-body", {{ ["Content-Type"] = "application/json" }})"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_post_headers).eval_async().await.unwrap();
        assert_eq!(res.get::<String>("body").unwrap(), "POST-OK-JSON!");
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        // Test http_request with GET
        let code_req_get = format!(
            r#"http_request({{ url = "http://127.0.0.1:{}/" }})"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_req_get).eval_async().await.unwrap();
        assert_eq!(res.get::<String>("body").unwrap(), "GET-OK!");
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        // Test http_request with POST
        let code_req_post = format!(
            r#"http_request({{ method = "POST", url = "http://127.0.0.1:{}/", body = "my-body" }})"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_req_post).eval_async().await.unwrap();
        assert_eq!(res.get::<String>("body").unwrap(), "POST-OK!");
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        // Test http_request with HEAD
        let code_req_head = format!(
            r#"http_request({{ method = "HEAD", url = "http://127.0.0.1:{}/" }})"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_req_head).eval_async().await.unwrap();
        assert_eq!(res.get::<u16>("status").unwrap(), 200);
        assert_eq!(res.get::<String>("body").unwrap(), "");

        // Test http_request with binary body
        let code_binary = format!(
            r#"http_request({{ method = "POST", url = "http://127.0.0.1:{}/", body = "binary-data\x00\xff" }})"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_binary).eval_async().await.unwrap();
        assert_eq!(res.get::<String>("body").unwrap(), "BINARY-OK!");
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        // Test http_multipart with text fields
        let code_multi_text = format!(
            r#"http_multipart("http://127.0.0.1:{}/", {{ caption = "my-caption", desc = "hello" }})"#,
            server.port()
        );
        let res: mlua::Table = lua.load(&code_multi_text).eval_async().await.unwrap();
        // Just verify it reached the server and got a response
        assert_eq!(res.get::<u16>("status").unwrap(), 200);

        // Test http_multipart with file field
        let code_multi_file = format!(
            r#"
            local res = http_multipart("http://127.0.0.1:{0}/", {{
                file = {{ content = "my-file-content", filename = "test.txt", type = "text/plain" }},
                caption = "my-caption"
            }})
            assert(res.status == 200)
            assert(res.body == "MULTIPART-OK!")
            "#,
            server.port()
        );
        lua.load(&code_multi_file).exec_async().await.unwrap();
    });
}

#[test]
fn test_lua_storage_bindings_are_exposed_and_work() {
    let tdb = TestDb::new("test_lua_storage_bindings");

    smol::block_on(async {
        let lua_job_one = Lua::new();
        register(&lua_job_one, tdb.db(), "job_one").unwrap();

        let lua_job_two = Lua::new();
        register(&lua_job_two, tdb.db(), "job_two").unwrap();

        for name in [
            "store_set",
            "store_get",
            "store_delete",
            "global_store_set",
            "global_store_get",
            "global_store_delete",
            "global_store_incr",
        ] {
            let func = lua_job_one
                .globals()
                .get::<Option<mlua::Function>>(name)
                .unwrap();
            assert!(
                func.is_some(),
                "expected Lua global {name} to be registered"
            );
        }

        let missing_job_value: Option<String> = lua_job_one
            .load(r#"return store_get("page")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(missing_job_value, None);

        let missing_global_value: Option<String> = lua_job_one
            .load(r#"return global_store_get("shared_counter")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(missing_global_value, None);

        lua_job_one
            .load(
                r#"
                store_set("page", "3")
                global_store_set("shared_counter", "10")
            "#,
            )
            .exec_async()
            .await
            .unwrap();

        let job_one_page: Option<String> = lua_job_one
            .load(r#"return store_get("page")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(job_one_page.as_deref(), Some("3"));

        let job_two_page: Option<String> = lua_job_two
            .load(r#"return store_get("page")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(job_two_page, None);

        let shared_from_job_two: Option<String> = lua_job_two
            .load(r#"return global_store_get("shared_counter")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(shared_from_job_two.as_deref(), Some("10"));

        lua_job_one
            .load(r#"store_delete("page")"#)
            .exec_async()
            .await
            .unwrap();
        lua_job_two
            .load(r#"global_store_delete("shared_counter")"#)
            .exec_async()
            .await
            .unwrap();

        let deleted_job_value: Option<String> = lua_job_one
            .load(r#"return store_get("page")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(deleted_job_value, None);

        let deleted_global_value: Option<String> = lua_job_one
            .load(r#"return global_store_get("shared_counter")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(deleted_global_value, None);
    });
}

#[test]
fn test_global_store_incr() {
    let tdb = TestDb::new("test_global_store_incr");

    smol::block_on(async {
        let lua = Lua::new();
        register(&lua, tdb.db(), "test_job").unwrap();

        let initial: i64 = lua
            .load(r#"return global_store_incr("incr_key", 0, 1)"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(initial, 1);

        let incremented: i64 = lua
            .load(r#"return global_store_incr("incr_key", 0, 5)"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(incremented, 6);

        let decremented: i64 = lua
            .load(r#"return global_store_incr("incr_key", 0, -3)"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(decremented, 3);

        let uses_default: i64 = lua
            .load(r#"return global_store_incr("nonexistent_key", 100, 10)"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(uses_default, 110);
    });
}

#[test]
fn test_env_get_binding() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();

    unsafe {
        std::env::set_var("SPYWEB_TEST_SECRET", "12345");
        std::env::set_var("OTHER_SECRET", "hacker");
    }

    let val1: String = lua
        .load(r#"return env_get("SPYWEB_TEST_SECRET")"#)
        .eval()
        .unwrap();
    assert_eq!(val1, "12345");

    let val2: String = lua.load(r#"return env_get("TEST_SECRET")"#).eval().unwrap();
    assert_eq!(val2, "12345");

    let val3: Option<String> = lua
        .load(r#"return env_get("OTHER_SECRET")"#)
        .eval()
        .unwrap();
    assert!(
        val3.is_none(),
        "Should not access variables without SPYWEB_ prefix"
    );
}

#[test]
fn test_dump_binding_formats_nested_tables_and_cycles() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();

    let dumped: String = lua
        .load(
            r#"
            local t = {
                status = 200,
                nested = {
                    ok = true,
                    message = "hello"
                }
            }
            t.self = t
            return dump(t)
        "#,
        )
        .eval()
        .unwrap();

    assert!(dumped.contains("status = 200"));
    assert!(dumped.contains("nested = {"));
    assert!(dumped.contains("ok = true"));
    assert!(dumped.contains(r#"message = "hello""#));
    assert!(dumped.contains("self = <cycle>"));
}

#[test]
fn test_copy_and_deep_copy_helpers() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();

    lua.load(
        r#"
        local original = {
            a = 1,
            b = { c = 2 },
            d = { 3, 4 }
        }

        local shallow = copy(original)
        assert(shallow ~= original, "shallow should be a new table")
        assert(shallow.a == original.a, "shallow.a should match")
        assert(shallow.b == original.b, "shallow.b should be the same reference")

        local deep = deep_copy(original)
        assert(deep ~= original, "deep should be a new table")
        assert(deep.a == original.a, "deep.a should match")
        assert(deep.b ~= original.b, "deep.b should be a new table")
        assert(deep.b.c == original.b.c, "deep.b.c should match")
        assert(deep.d ~= original.d, "deep.d should be a new table")
        assert(deep.d[1] == original.d[1], "deep.d[1] should match")

        original.self = original
        local deep_cycle = deep_copy(original)
        assert(deep_cycle.self == deep_cycle, "deep_cycle should preserve self-reference")
        assert(deep_cycle.self ~= original, "deep_cycle.self should not point to original")
    "#,
    )
    .exec()
    .unwrap();
}

#[test]
fn test_cdp_bindings_exposed() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();
    register_cdp(&lua, None).unwrap();

    let cdp: mlua::Table = lua.globals().get("cdp").unwrap();
    assert!(cdp.contains_key("connect").unwrap());
    assert!(cdp.contains_key("launch").unwrap());
    assert!(cdp.contains_key("get_browser").unwrap());
    assert!(cdp.contains_key("_inject_page").unwrap());

    lua.load(
        r#"
        assert(type(cdp_navigate) == "function", "cdp_navigate should exist")
        assert(type(cdp_get_html) == "function", "cdp_get_html should exist")
    "#,
    )
    .exec()
    .unwrap();
}

#[test]
fn test_cdp_inject_page_methods() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();
    register_cdp(&lua, None).unwrap();

    lua.load(
        r##"
        local last_event = nil
        local last_timeout = nil

        local page = {
            call = function(self, method, params)
                last_method = method
                last_params = params
                return { result = { value = true } }
            end,
            wait_event = function(self, event, timeout_ms)
                last_event = event
                last_timeout = timeout_ms
                return { method = event, params = {} }
            end
        }

        cdp._inject_page(page)

        -- All methods should be present
        assert(type(page.open) == "function", "open method missing")
        assert(type(page.content) == "function", "content method missing")
        assert(type(page.evaluate) == "function", "evaluate method missing")
        assert(type(page.click) == "function", "click method missing")
        assert(type(page.type) == "function", "type method missing")
        assert(type(page.wait_for_selector) == "function", "wait_for_selector method missing")

        -- open defaults to Page.loadEventFired with 30s timeout
        page:open("https://x.com")
        assert(last_method == "Page.navigate", "open should call Page.navigate")
        assert(last_params.url == "https://x.com", "open should pass url")
        assert(last_event == nil or last_event == "Page.loadEventFired", "open should wait for loadEventFired by default")
        assert(last_timeout == nil or last_timeout == 30000, "open should default to 30s timeout")

        -- open with custom CDP event name
        page:open("https://x.com", "Page.domContentEventFired")
        assert(last_event == "Page.domContentEventFired", "open should use custom CDP event")
        assert(last_timeout == 30000, "open should pass timeout with custom event")

        -- open with custom timeout
        page:open("https://x.com", "Page.loadEventFired", 15000)
        assert(last_event == "Page.loadEventFired", "open should accept raw CDP event")
        assert(last_timeout == 15000, "open should accept custom timeout")

        -- evaluate should return result.result.value
        local val = page:evaluate("1 + 1")
        assert(val == true, "evaluate should return result.result.value")
        assert(last_method == "Runtime.evaluate", "evaluate should call Runtime.evaluate")

        -- content should return result.result.value
        local html = page:content()
        assert(html == true, "content should return result.result.value")
        assert(last_method == "Runtime.evaluate", "content should call Runtime.evaluate")

        -- click should inject querySelector
        page:click(".btn")
        assert(last_method == "Runtime.evaluate", "click should call Runtime.evaluate")
        assert(
            string.find(last_params.expression, "querySelector") ~= nil,
            "click should use querySelector"
        )

        -- type should inject value assignment
        page:type("#input", "hello")
        assert(last_method == "Runtime.evaluate", "type should call Runtime.evaluate")
        assert(
            string.find(last_params.expression, "#input") ~= nil,
            "type should include selector in JS"
        )
        assert(
            string.find(last_params.expression, "hello") ~= nil,
            "type should include text in JS"
        )

        -- wait_for_selector polls for DOM presence
        local ok = page:wait_for_selector(".content", 5000)
        assert(ok == true, "wait_for_selector should return true when found")
        assert(last_method == "Runtime.evaluate", "wait_for_selector should use Runtime.evaluate")
        assert(
            string.find(last_params.expression, ".content") ~= nil,
            "wait_for_selector should query for the given selector"
        )

        -- User-defined methods work (page is a plain table)
        function page:custom() return "custom-value" end
        assert(page:custom() == "custom-value", "custom method should work")

        -- Override a built-in method
        function page:open(url)
            return "intercepted"
        end
        assert(page:open("https://x.com") == "intercepted", "override should work")
    "##,
    )
    .exec()
    .unwrap();
}

#[test]
fn test_cdp_page_network_and_screenshot_helpers() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();
    register_cdp(&lua, None).unwrap();

    lua.load(
        r##"
        local calls = {}
        local writes = {}
        cdp._write_base64 = function(path, data)
            table.insert(writes, { path = path, data = data })
        end

        local page = {
            call = function(self, method, params)
                table.insert(calls, { method = method, params = params })
                if method == "Page.captureScreenshot" then
                    return { data = "ZmFrZS1wbmc=" }
                elseif method == "Network.getAllCookies" then
                    return { cookies = {{ name = "sid", value = "abc" }} }
                elseif method == "Network.getCookies" then
                    return { cookies = {{ name = "url_sid", value = "def" }} }
                elseif method == "Page.getLayoutMetrics" then
                    return { contentSize = { width = 800, height = 1200 } }
                end
                return {}
            end,
            call_save = function(self, method, params, path)
                table.insert(calls, { method = method, params = params })
                table.insert(writes, { path = path, data = "ZmFrZS1wbmc=" })
            end,
            wait_event = function(self, event, timeout_ms, predicate)
                return { method = event, params = {} }
            end
        }

        cdp._inject_page(page)

        page:set_extra_headers({ ["Accept-Language"] = "en-US" })
        assert(calls[#calls].method == "Network.setExtraHTTPHeaders", "set_extra_headers should use Network.setExtraHTTPHeaders")
        assert(calls[#calls].params.headers["Accept-Language"] == "en-US", "set_extra_headers should pass headers")

        page:set_user_agent("SpyWebBot", { accept_language = "en-US", platform = "Linux" })
        assert(calls[#calls].method == "Network.setUserAgentOverride", "set_user_agent should use Network.setUserAgentOverride")
        assert(calls[#calls].params.userAgent == "SpyWebBot", "set_user_agent should pass user agent")
        assert(calls[#calls].params.acceptLanguage == "en-US", "set_user_agent should pass accept language")

        local all_cookies = page:cookies()
        assert(calls[#calls].method == "Network.getAllCookies", "cookies without urls should use Network.getAllCookies")
        assert(all_cookies[1].name == "sid", "cookies should return cookie list")

        local url_cookies = page:cookies({ "https://example.com" })
        assert(calls[#calls].method == "Network.getCookies", "cookies with urls should use Network.getCookies")
        assert(url_cookies[1].name == "url_sid", "cookies(urls) should return URL-scoped cookies")

        page:set_cookies({{ name = "sid", value = "123", domain = "example.com", path = "/" }})
        assert(calls[#calls].method == "Network.setCookies", "set_cookies should use Network.setCookies")
        assert(calls[#calls].params.cookies[1].name == "sid", "set_cookies should pass cookies")

        local patterns = page:block_resources({ "image", "font", "*.analytics.js" })
        assert(calls[#calls].method == "Network.setBlockedURLs", "block_resources should use Network.setBlockedURLs")
        assert(#patterns > 3, "block_resources should expand resource types")

        local path = page:screenshot("debug.png", { full_page = true })
        assert(path == "debug.png", "screenshot should return path")
        assert(writes[1].path == "debug.png", "screenshot should write to requested path")
        assert(writes[1].data == "ZmFrZS1wbmc=", "screenshot should write captured base64")
    "##,
    )
    .exec()
    .unwrap();
}

#[test]
fn test_cdp_page_wait_scroll_and_real_input_helpers() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();
    register_cdp(&lua, None).unwrap();

    lua.load(
        r##"
        local calls = {}
        local href_checks = 0
        cdp.sleep = function(ms) end

        local page = {
            call = function(self, method, params)
                table.insert(calls, { method = method, params = params })
                if method == "Runtime.evaluate" then
                    local expr = params.expression
                    if string.find(expr, "location.href") then
                        href_checks = href_checks + 1
                        if href_checks == 1 then
                            return { result = { value = "https://example.com/loading" } }
                        end
                        return { result = { value = "https://example.com/jobs" } }
                    elseif string.find(expr, "getBoundingClientRect") then
                        return { result = { value = { x = 25, y = 50 } } }
                    elseif string.find(expr, "performance.getEntriesByType") then
                        return { result = { value = { readyState = "complete", recent = 0 } } }
                    elseif string.find(expr, "window.scrollBy") then
                        return { result = { value = { before = 0, after = 1000, maxY = 1000 } } }
                    elseif string.find(expr, "document.querySelector") then
                        return { result = { value = false } }
                    end
                    return { result = { value = true } }
                end
                return {}
            end,
            wait_event = function(self, event, timeout_ms, predicate)
                local params = {
                    response = {
                        url = "https://example.com/api/jobs",
                        status = 200
                    }
                }
                if predicate then
                    assert(predicate(params) == true, "wait_for_response should pass predicate")
                end
                return { method = event, params = params }
            end
        }

        cdp._inject_page(page)

        local url = page:wait_for_url("/jobs", 1000)
        assert(url == "https://example.com/jobs", "wait_for_url should return matching URL")

        local response = page:wait_for_response(function(params)
            return params.response.status == 200
        end, 1000)
        assert(response.method == "Network.responseReceived", "wait_for_response should wait for responseReceived")

        assert(page:wait_for_idle(1000, 1) == true, "wait_for_idle should return true after quiet period")
        assert(page:scroll({ max_scrolls = 1 }) == true, "scroll should return true when bottom is reached")

        page:click(".button", { real = true })
        assert(calls[#calls - 2].method == "Input.dispatchMouseEvent", "real click should dispatch mouseMoved")
        assert(calls[#calls - 1].method == "Input.dispatchMouseEvent", "real click should dispatch mousePressed")
        assert(calls[#calls].method == "Input.dispatchMouseEvent", "real click should dispatch mouseReleased")

        page:type("#q", "hello", { real = true })
        assert(calls[#calls].method == "Input.insertText", "real type should use Input.insertText")
        assert(calls[#calls].params.text == "hello", "real type should pass text")
    "##,
    )
    .exec()
    .unwrap();
}

#[test]
fn test_cdp_page_helpers_log_and_continue_on_page_failures() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();
    register_cdp(&lua, None).unwrap();

    lua.load(
        r##"
        local logs = {}
        cdp._log_terminal = function(msg)
            table.insert(logs, msg)
        end
        cdp.sleep = function(ms) end

        local page = {
            call = function(self, method, params)
                if method == "Runtime.evaluate" then
                    return {
                        result = { value = { found = false, readyState = "complete", url = "https://example.com" } },
                        exceptionDetails = {
                            text = "selector not found"
                        }
                    }
                end
                return {}
            end,
            wait_event = function(self, event, timeout_ms, predicate)
                error("Timeout waiting for CDP event " .. event)
            end
        }

        cdp._inject_page(page)

        local found, selector_err = page:wait_for_selector(".missing", { timeout_ms = 1, scroll = false })
        assert(found == false, "wait_for_selector should return false on timeout")
        assert(type(selector_err) == "string", "wait_for_selector should return an error message")

        local clicked, click_err = page:click(".missing")
        assert(clicked == false, "click should return false when selector is missing")
        assert(type(click_err) == "string", "click should return an error message")

        local response, response_err = page:wait_for_response(nil, 1)
        assert(response == nil, "wait_for_response should return nil on timeout")
        assert(type(response_err) == "string", "wait_for_response should return an error message")

        local continued = true
        assert(continued == true, "Lua chunk should continue after helper failures")
        assert(#logs >= 3, "helper failures should be logged")
    "##,
    )
    .exec()
    .unwrap();
}

#[cfg(feature = "sqlite")]
#[test]
fn test_db_query_from_lua() {
    let tdb = TestDb::new("lua_sql_query");

    smol::block_on(async {
        let lua = Lua::new();
        register(&lua, tdb.db(), "test").unwrap();

        lua.load(
            r#"
            global_store_set("name", "spyweb")
            global_store_set("version", "1.0")
        "#,
        )
        .exec_async()
        .await
        .unwrap();

        let rows: mlua::Table = lua
            .load(
                r#"
                return db_query("SELECT key, value FROM lua_user ORDER BY key")
            "#,
            )
            .eval_async()
            .await
            .unwrap();

        assert_eq!(rows.raw_len(), 2);
        let row1: mlua::Table = rows.get(1).unwrap();
        assert_eq!(row1.get::<String>("key").unwrap(), "name");
        assert_eq!(row1.get::<String>("value").unwrap(), "spyweb");

        let row2: mlua::Table = rows.get(2).unwrap();
        assert_eq!(row2.get::<String>("key").unwrap(), "version");
    });
}

#[cfg(feature = "sqlite")]
#[test]
fn test_db_query_with_params_from_lua() {
    let tdb = TestDb::new("lua_sql_query_params");

    smol::block_on(async {
        let lua = Lua::new();
        register(&lua, tdb.db(), "test").unwrap();
        lua.load(r#"global_store_set("lang", "luau")"#)
            .exec_async()
            .await
            .unwrap();

        let rows: mlua::Table = lua
            .load(
                r#"
                return db_query("SELECT value FROM lua_user WHERE key = ?", { "lang" })
            "#,
            )
            .eval_async()
            .await
            .unwrap();

        assert_eq!(rows.raw_len(), 1);
        let row: mlua::Table = rows.get(1).unwrap();
        assert_eq!(row.get::<String>("value").unwrap(), "luau");
    });
}

#[cfg(feature = "sqlite")]
#[test]
fn test_db_query_bad_sql_from_lua() {
    let tdb = TestDb::new("lua_sql_query_bad");

    smol::block_on(async {
        let lua = Lua::new();
        register(&lua, tdb.db(), "test").unwrap();

        let result: mlua::Result<mlua::Table> = lua
            .load(r#"return db_query("not valid sql", {})"#)
            .eval_async()
            .await;
        assert!(result.is_err());
    });
}

#[cfg(feature = "sqlite")]
#[test]
fn test_db_exec_insert_from_lua() {
    let tdb = TestDb::new("lua_sql_exec_insert");

    smol::block_on(async {
        let lua = Lua::new();
        register(&lua, tdb.db(), "test").unwrap();

        lua.load(
            r#"
            local affected = db_exec("INSERT INTO lua_user (key, value) VALUES (?, ?)", { "role", "admin" })
            assert(affected == 1, "expected 1 row affected")
        "#,
        )
        .exec_async()
        .await
        .unwrap();

        let val: Option<String> = lua
            .load(r#"return global_store_get("role")"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(val.as_deref(), Some("admin"));
    });
}

#[cfg(feature = "sqlite")]
#[test]
fn test_db_exec_create_table_from_lua() {
    let tdb = TestDb::new("lua_sql_exec_create");

    smol::block_on(async {
        let lua = Lua::new();
        register(&lua, tdb.db(), "test").unwrap();

        lua.load(
            r#"
            db_exec("CREATE TABLE IF NOT EXISTS markers (id TEXT PRIMARY KEY, val INTEGER)")
            local affected = db_exec("INSERT OR REPLACE INTO markers (id, val) VALUES (?, ?)", { "page", 5 })
            assert(affected == 1, "expected 1 row affected")
        "#,
        )
        .exec_async()
        .await
        .unwrap();

        let rows: mlua::Table = lua
            .load(r#"return db_query("SELECT val FROM markers WHERE id = ?", { "page" })"#)
            .eval_async()
            .await
            .unwrap();
        assert_eq!(rows.raw_len(), 1);
        let row: mlua::Table = rows.get(1).unwrap();
        assert_eq!(row.get::<i64>("val").unwrap(), 5);
    });
}

#[cfg(feature = "sqlite")]
#[test]
fn test_db_exec_bad_sql_from_lua() {
    let tdb = TestDb::new("lua_sql_exec_bad");

    smol::block_on(async {
        let lua = Lua::new();
        register(&lua, tdb.db(), "test").unwrap();

        let result: mlua::Result<u64> = lua
            .load(r#"return db_exec("not sql", {})"#)
            .eval_async()
            .await;
        assert!(result.is_err());
    });
}

#[test]
fn test_fs_read_binding() {
    let current_dir = std::env::current_dir().unwrap();
    let test_dir = current_dir.join("target").join("test_fs_read_binding");
    let job_dir = test_dir.join("myjob");
    std::fs::create_dir_all(&job_dir).unwrap();

    let result = smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, Some(job_dir.clone()), "test").unwrap();

        lua.load("fs_overwrite(\"test.json\", \"hello world\")")
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;

        let content: Option<String> = lua
            .load(r#"return fs_read("test.json")"#)
            .eval_async()
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(content.as_deref(), Some("hello world"));

        let missing: Option<String> = lua
            .load(r#"return fs_read("nope.json")"#)
            .eval_async()
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(missing, None);

        let err: mlua::Result<String> = lua
            .load(r#"return fs_read("/etc/passwd")"#)
            .eval_async()
            .await;
        assert!(err.is_err(), "absolute paths should be rejected");

        Ok::<_, String>(())
    });

    let _ = std::fs::remove_dir_all(&test_dir);
    result.unwrap();
}

#[test]
fn test_require_deep_path() {
    let test_dir = tempfile::tempdir().unwrap();
    let job_dir = test_dir.path().join("jobs/test_job");
    let deep_dir = job_dir.join("mylua/deep/child");
    std::fs::create_dir_all(&deep_dir).unwrap();

    // Create module at a deep nested path
    let module_path = deep_dir.join("shared-across-everything.lua");
    std::fs::write(&module_path, "return { val = 99 }").unwrap();

    smol::block_on(async {
        let lua = mlua::Lua::new();
        super::system::register(&lua, Some(job_dir), "test_job").unwrap();

        // require with dotted path should resolve the deep structure
        let result: mlua::Table = lua
            .load(r#"return require("mylua.deep.child.shared-across-everything")"#)
            .eval_async()
            .await
            .expect("require should resolve deep dotted path");

        let val: i32 = result.get("val").unwrap();
        assert_eq!(val, 99);
    });
}

#[test]
fn test_json_encode_decode_roundtrip() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();

    let result: String = lua
        .load(r#"return json_encode({ name = "spy", count = 3, ok = true })"#)
        .eval()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["name"], "spy");
    assert_eq!(parsed["count"], 3);
    assert_eq!(parsed["ok"], true);

    let back: mlua::Table = lua
        .load(r#"return json_decode('{"x": 10, "y": "hello"}')"#)
        .eval()
        .unwrap();
    assert_eq!(back.get::<i32>("x").unwrap(), 10);
    assert_eq!(back.get::<String>("y").unwrap(), "hello");

    let nested: mlua::Table = lua
        .load(r#"return json_decode('[1, "two", true]')"#)
        .eval()
        .unwrap();
    assert_eq!(nested.get::<i32>(1).unwrap(), 1);
    assert_eq!(nested.get::<String>(2).unwrap(), "two");
    assert_eq!(nested.get::<bool>(3).unwrap(), true);

    let err = lua
        .load(r#"return json_decode("not json")"#)
        .eval::<mlua::Value>();
    assert!(err.is_err(), "invalid JSON should error");
}

#[test]
fn test_sleep_basic() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None, "test").unwrap();

    smol::block_on(async {
        let start = std::time::Instant::now();
        lua.load("sleep(10)").exec_async().await.unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() >= 8,
            "sleep(10) should take at least ~10ms, took {:?}",
            elapsed
        );
    });
}

#[test]
fn test_fs_append() {
    let current_dir = std::env::current_dir().unwrap();
    let test_dir = current_dir.join("target").join("test_fs_append");
    let job_dir = test_dir.join("myjob");
    std::fs::create_dir_all(&job_dir).unwrap();

    let result = smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, Some(job_dir.clone()), "test").unwrap();

        lua.load(r#"fs_append("data.txt", "line1")"#)
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;

        lua.load(r#"fs_append("data.txt", "line2")"#)
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;

        let content =
            std::fs::read_to_string(job_dir.join("data.txt")).map_err(|e| e.to_string())?;
        assert_eq!(content, "line1line2", "fs_append should concatenate");

        let err: mlua::Result<()> = lua
            .load(r#"fs_append("/etc/passwd", "nope")"#)
            .exec_async()
            .await;
        assert!(err.is_err(), "absolute paths should be rejected");

        Ok::<_, String>(())
    });

    let _ = std::fs::remove_dir_all(&test_dir);
    result.unwrap();
}

#[test]
fn test_fs_read_binary() {
    let current_dir = std::env::current_dir().unwrap();
    let test_dir = current_dir.join("target").join("test_fs_read_binary");
    let job_dir = test_dir.join("myjob");
    std::fs::create_dir_all(&job_dir).unwrap();

    let result = smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, Some(job_dir.clone()), "test").unwrap();

        let bin_data = vec![0x00, 0xFF, 0x41, 0x42, 0x43, 0x01];
        std::fs::write(job_dir.join("data.json"), &bin_data).unwrap();

        let content: mlua::Table = lua
            .load(r#"return fs_read_binary("data.json")"#)
            .eval_async()
            .await
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        for pair in content.pairs::<usize, u8>() {
            bytes.push(pair.map_err(|e| e.to_string())?.1);
        }
        assert_eq!(bytes, bin_data, "binary content should roundtrip");

        let missing: mlua::Value = lua
            .load(r#"return fs_read_binary("nope.json")"#)
            .eval_async()
            .await
            .map_err(|e| e.to_string())?;
        assert!(missing.is_nil(), "missing file should return nil");

        let err: mlua::Result<()> = lua
            .load(r#"fs_read_binary("/etc/passwd")"#)
            .exec_async()
            .await;
        assert!(err.is_err(), "absolute paths should be rejected");

        let png_data = vec![0x89, 0x50, 0x4E, 0x47];
        std::fs::write(job_dir.join("image.png"), &png_data).unwrap();
        let content: mlua::Table = lua
            .load(r#"return fs_read_binary("image.png")"#)
            .eval_async()
            .await
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        for pair in content.pairs::<usize, u8>() {
            bytes.push(pair.map_err(|e| e.to_string())?.1);
        }
        assert_eq!(bytes, png_data, "png should be readable");

        Ok::<_, String>(())
    });

    let _ = std::fs::remove_dir_all(&test_dir);
    result.unwrap();
}

#[test]
fn test_fs_overwrite_dedicated() {
    let current_dir = std::env::current_dir().unwrap();
    let test_dir = current_dir
        .join("target")
        .join("test_fs_overwrite_dedicated");
    let job_dir = test_dir.join("myjob");
    std::fs::create_dir_all(&job_dir).unwrap();

    let result = smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, Some(job_dir.clone()), "test").unwrap();

        lua.load(r#"fs_overwrite("state.json", "first")"#)
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(
            std::fs::read_to_string(job_dir.join("state.json")).unwrap(),
            "first"
        );

        lua.load(r#"fs_overwrite("state.json", "second")"#)
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(
            std::fs::read_to_string(job_dir.join("state.json")).unwrap(),
            "second"
        );

        let err: mlua::Result<()> = lua
            .load(r#"fs_overwrite("/etc/evil", "nope")"#)
            .exec_async()
            .await;
        assert!(err.is_err(), "absolute paths should be rejected");

        Ok::<_, String>(())
    });

    let _ = std::fs::remove_dir_all(&test_dir);
    result.unwrap();
}

#[test]
fn test_log_binding() {
    let current_dir = std::env::current_dir().unwrap();
    let test_dir = current_dir.join("target").join("test_log_binding");
    let job_dir = test_dir.join("myjob");
    std::fs::create_dir_all(&job_dir).unwrap();

    let result = smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, Some(job_dir.clone()), "test_job").unwrap();

        lua.load(r#"log("hello from test")"#)
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;

        // log writes via the IO service which runs async — give it a moment
        smol::Timer::after(std::time::Duration::from_millis(200)).await;

        let log_path = job_dir.join("hooks.log");
        assert!(log_path.exists(), "hooks.log should be created");
        let content = std::fs::read_to_string(&log_path).map_err(|e| e.to_string())?;
        assert!(
            content.contains("hello from test"),
            "log file should contain the message, got: {}",
            content
        );

        Ok::<_, String>(())
    });

    let _ = std::fs::remove_dir_all(&test_dir);
    result.unwrap();
}

#[test]
fn test_fs_shared_read_write() {
    let current_dir = std::env::current_dir().unwrap();
    let shared_dir = current_dir.join("shared");
    let test_dir = current_dir.join("target").join("test_fs_shared");
    let job_dir = test_dir.join("myjob");
    std::fs::create_dir_all(&job_dir).unwrap();
    std::fs::create_dir_all(&shared_dir).unwrap();

    let shared_prefix = format!("{}/", super::system::SHARED_DIR);

    let result = smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, Some(job_dir.clone()), "test").unwrap();

        // Write to job_dir (default)
        lua.load(r#"fs_overwrite("local.txt", "local data")"#)
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;
        assert!(job_dir.join("local.txt").exists());

        // Write to shared/ explicitly
        let write_path = format!("{}shared.txt", shared_prefix);
        lua.load(format!(r#"fs_overwrite("{}", "shared data")"#, write_path))
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;
        assert!(shared_dir.join("shared.txt").exists());

        // Read finds local file first
        let content: Option<String> = lua
            .load(r#"return fs_read("local.txt")"#)
            .eval_async()
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(content.as_deref(), Some("local data"));

        // Read finds shared file
        let read_path = format!("{}shared.txt", shared_prefix);
        let content: Option<String> = lua
            .load(format!(r#"return fs_read("{}")"#, read_path))
            .eval_async()
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(content.as_deref(), Some("shared data"));

        // Append to shared/
        let append_path = format!("{}shared.txt", shared_prefix);
        lua.load(format!(r#"fs_append("{}", " more")"#, append_path))
            .exec_async()
            .await
            .map_err(|e| e.to_string())?;
        let content = std::fs::read_to_string(shared_dir.join("shared.txt")).unwrap();
        assert_eq!(content, "shared data more");

        Ok::<_, String>(())
    });

    let _ = std::fs::remove_dir_all(&test_dir);
    let _ = std::fs::remove_dir_all(&shared_dir);
    result.unwrap();
}

#[test]
fn test_fs_rejected_path() {
    let current_dir = std::env::current_dir().unwrap();
    let test_dir = current_dir.join("target").join("test_fs_rejected");
    let job_dir = test_dir.join("myjob");
    std::fs::create_dir_all(&job_dir).unwrap();

    let result = smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, Some(job_dir.clone()), "test").unwrap();

        // Traversal should be blocked
        let err: mlua::Result<()> = lua.load(r#"fs_read("../etc/passwd")"#).exec_async().await;
        assert!(err.is_err(), "traversal should be rejected");

        let err: mlua::Result<()> = lua
            .load(r#"fs_overwrite("../evil.txt", "nope")"#)
            .exec_async()
            .await;
        assert!(err.is_err(), "traversal write should be rejected");

        // Absolute paths should be blocked
        let err: mlua::Result<()> = lua.load(r#"fs_read("/etc/passwd")"#).exec_async().await;
        assert!(err.is_err(), "absolute path should be rejected");

        Ok::<_, String>(())
    });

    let _ = std::fs::remove_dir_all(&test_dir);
    result.unwrap();
}
