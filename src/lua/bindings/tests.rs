use super::*;
use crate::services::db::Db;
use mlua::Lua;
use std::io::Read;
use std::{fs, sync::Arc};

struct TestDb {
    path: String,
    db: Option<Arc<Db>>,
}

impl TestDb {
    fn new(name: &str) -> Self {
        let path = format!("{}.redb", name);
        if fs::metadata(&path).is_ok() {
            fs::remove_file(&path).unwrap();
        }
        let db = Arc::new(Db::open(&path).unwrap());
        Self { path, db: Some(db) }
    }

    fn db(&self) -> Arc<Db> {
        Arc::clone(self.db.as_ref().unwrap())
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        self.db = None;
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn test_http_bindings() {
    let server = rouille::Server::new("127.0.0.1:0", |request| {
        if request.method() == "GET" {
            if request.header("X-Custom-Auth") == Some("secret-123") {
                rouille::Response::text("GET-OK-AUTH!")
            } else {
                rouille::Response::text("GET-OK!")
            }
        } else if request.method() == "POST" {
            let mut body = String::new();
            if let Some(mut reader) = request.data() {
                let _ = reader.read_to_string(&mut body);
            }
            assert_eq!(body, "my-body");
            if request.header("Content-Type") == Some("application/json") {
                rouille::Response::text("POST-OK-JSON!")
            } else {
                rouille::Response::text("POST-OK!")
            }
        } else {
            rouille::Response::empty_404()
        }
    })
    .unwrap();
    let port = server.server_addr().port();
    std::thread::spawn(move || server.run());

    smol::block_on(async {
        let lua = Lua::new();
        register_http_and_fs(&lua, None).unwrap();

        let code_get = format!(r#"http_get("http://127.0.0.1:{}/")"#, port);
        let body: String = lua.load(&code_get).eval_async().await.unwrap();
        assert_eq!(body, "GET-OK!");

        let code_get_headers = format!(
            r#"http_get("http://127.0.0.1:{}/", {{ ["X-Custom-Auth"] = "secret-123" }})"#,
            port
        );
        let body: String = lua.load(&code_get_headers).eval_async().await.unwrap();
        assert_eq!(body, "GET-OK-AUTH!");

        let code_post = format!(r#"http_post("http://127.0.0.1:{}/", "my-body")"#, port);
        let body: String = lua.load(&code_post).eval_async().await.unwrap();
        assert_eq!(body, "POST-OK!");

        let code_post_headers = format!(
            r#"http_post("http://127.0.0.1:{}/", "my-body", {{ ["Content-Type"] = "application/json" }})"#,
            port
        );
        let body: String = lua.load(&code_post_headers).eval_async().await.unwrap();
        assert_eq!(body, "POST-OK-JSON!");
    });
}

#[test]
fn test_lua_storage_bindings_are_exposed_and_work() {
    let tdb = TestDb::new("test_lua_storage_bindings");

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
        .eval()
        .unwrap();
    assert_eq!(missing_job_value, None);

    let missing_global_value: Option<String> = lua_job_one
        .load(r#"return global_store_get("shared_counter")"#)
        .eval()
        .unwrap();
    assert_eq!(missing_global_value, None);

    lua_job_one
        .load(
            r#"
            store_set("page", "3")
            global_store_set("shared_counter", "10")
        "#,
        )
        .exec()
        .unwrap();

    let job_one_page: Option<String> = lua_job_one
        .load(r#"return store_get("page")"#)
        .eval()
        .unwrap();
    assert_eq!(job_one_page.as_deref(), Some("3"));

    let job_two_page: Option<String> = lua_job_two
        .load(r#"return store_get("page")"#)
        .eval()
        .unwrap();
    assert_eq!(job_two_page, None);

    let shared_from_job_two: Option<String> = lua_job_two
        .load(r#"return global_store_get("shared_counter")"#)
        .eval()
        .unwrap();
    assert_eq!(shared_from_job_two.as_deref(), Some("10"));

    lua_job_one.load(r#"store_delete("page")"#).exec().unwrap();
    lua_job_two
        .load(r#"global_store_delete("shared_counter")"#)
        .exec()
        .unwrap();

    let deleted_job_value: Option<String> = lua_job_one
        .load(r#"return store_get("page")"#)
        .eval()
        .unwrap();
    assert_eq!(deleted_job_value, None);

    let deleted_global_value: Option<String> = lua_job_one
        .load(r#"return global_store_get("shared_counter")"#)
        .eval()
        .unwrap();
    assert_eq!(deleted_global_value, None);
}

#[test]
fn test_global_store_incr() {
    let tdb = TestDb::new("test_global_store_incr");

    let lua = Lua::new();
    register(&lua, tdb.db(), "test_job").unwrap();

    let initial: i64 = lua
        .load(r#"return global_store_incr("incr_key", 0, 1)"#)
        .eval()
        .unwrap();
    assert_eq!(initial, 1);

    let incremented: i64 = lua
        .load(r#"return global_store_incr("incr_key", 0, 5)"#)
        .eval()
        .unwrap();
    assert_eq!(incremented, 6);

    let decremented: i64 = lua
        .load(r#"return global_store_incr("incr_key", 0, -3)"#)
        .eval()
        .unwrap();
    assert_eq!(decremented, 3);

    let uses_default: i64 = lua
        .load(r#"return global_store_incr("nonexistent_key", 100, 10)"#)
        .eval()
        .unwrap();
    assert_eq!(uses_default, 110);
}

#[test]
fn test_env_get_binding() {
    let lua = Lua::new();
    register_http_and_fs(&lua, None).unwrap();

    unsafe {
        std::env::set_var("SPYWEB_TEST_SECRET", "12345");
        std::env::set_var("OTHER_SECRET", "hacker");
    }

    // Should work with explicit prefix
    let val1: String = lua
        .load(r#"return env_get("SPYWEB_TEST_SECRET")"#)
        .eval()
        .unwrap();
    assert_eq!(val1, "12345");

    // Should work with auto-prefixing
    let val2: String = lua.load(r#"return env_get("TEST_SECRET")"#).eval().unwrap();
    assert_eq!(val2, "12345");

    // Should NOT find variables without the prefix
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
    register_http_and_fs(&lua, None).unwrap();

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
    register_http_and_fs(&lua, None).unwrap();

    lua.load(
        r#"
        local original = {
            a = 1,
            b = { c = 2 },
            d = { 3, 4 }
        }

        -- Test Shallow Copy
        local shallow = copy(original)
        assert(shallow ~= original, "shallow should be a new table")
        assert(shallow.a == original.a, "shallow.a should match")
        assert(shallow.b == original.b, "shallow.b should be the same reference")

        -- Test Deep Copy
        local deep = deep_copy(original)
        assert(deep ~= original, "deep should be a new table")
        assert(deep.a == original.a, "deep.a should match")
        assert(deep.b ~= original.b, "deep.b should be a new table")
        assert(deep.b.c == original.b.c, "deep.b.c should match")
        assert(deep.d ~= original.d, "deep.d should be a new table")
        assert(deep.d[1] == original.d[1], "deep.d[1] should match")

        -- Test Deep Copy Cycles
        original.self = original
        local deep_cycle = deep_copy(original)
        assert(deep_cycle.self == deep_cycle, "deep_cycle should preserve self-reference")
        assert(deep_cycle.self ~= original, "deep_cycle.self should not point to original")
    "#,
    )
    .exec()
    .unwrap();
}
