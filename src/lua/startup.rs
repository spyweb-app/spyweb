use crate::lua::engine;
use crate::services::db::Db;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn run(db: Arc<Db>) -> Result<()> {
    run_file(Path::new("startup.lua"), db)
}

pub(crate) fn run_file(path: &Path, db: Arc<Db>) -> Result<()> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "failed to read startup file '{}': {e}",
                path.display()
            ));
        }
    };

    let root = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };

    let lua = engine::create_startup_engine(Some(root), db)?;
    smol::block_on(async { lua.load(&source).set_name("startup.lua").exec_async().await })
        .map_err(|e| anyhow::anyhow!("startup.lua failed: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::loader;
    use std::fs;
    use tempfile::TempDir;

    fn temp_db() -> (TempDir, Arc<Db>) {
        let dir = tempfile::TempDir::with_prefix("startup_db").unwrap();
        let path = dir.path().join("db.sqlite");
        let db = Arc::new(Db::open(path.to_str().unwrap()).unwrap());
        (dir, db)
    }

    fn write_startup(root: &Path, script: &str) -> PathBuf {
        let path = root.join("startup.lua");
        fs::write(&path, script).unwrap();
        path
    }

    fn run_script(root: &Path, db: Arc<Db>, script: &str) -> Result<()> {
        run_file(&write_startup(root, script), db)
    }

    fn target_tempdir(prefix: &str) -> TempDir {
        fs::create_dir_all("target").unwrap();
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in("target")
            .unwrap()
    }

    #[test]
    fn test_missing_file_is_noop() {
        let (dir, db) = temp_db();
        let path = dir.path().join("does-not-exist.lua");

        assert!(run_file(&path, db).is_ok());
    }

    #[test]
    fn test_startup_executes_with_available_bindings() {
        let (dir, db) = temp_db();
        let script = r#"
            global_store_set("startup_key", "ready")
            assert(engine.version ~= nil)
            assert(env_get("SPYWEB_STARTUP_MISSING_XYZ") == nil)
            local encoded = json_encode({ ok = true })
            assert(encoded:find('"ok":true') ~= nil)
        "#;

        run_script(dir.path(), Arc::clone(&db), script).unwrap();

        assert_eq!(db.lua_get("startup_key").unwrap().as_deref(), Some("ready"));
    }

    #[test]
    fn test_startup_error_aborts() {
        let (dir, db) = temp_db();
        let err = run_script(dir.path(), db, r#"error("boom")"#)
            .unwrap_err()
            .to_string();

        assert!(err.contains("startup.lua"), "{err}");
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn test_forbidden_bindings_are_startup_specific_stubs() {
        let (dir, db) = temp_db();
        let cases = [
            (r#"store_set("key", "value")"#, "job-scoped"),
            (r#"store_get("key")"#, "job-scoped"),
            (r#"store_delete("key")"#, "job-scoped"),
            (r#"defer(function() end)"#, "cycle hooks"),
            (r#"queue_push("https://example.com")"#, "on_start"),
            (r#"cdp.launch()"#, "not available in startup.lua"),
        ];

        for (script, needle) in cases {
            let err = run_script(dir.path(), Arc::clone(&db), script)
                .unwrap_err()
                .to_string();
            assert!(err.contains(needle), "{script}: {err}");
        }
    }

    #[test]
    fn test_notify_is_registered_as_real_function() {
        let (dir, db) = temp_db();
        let lua = engine::create_startup_engine(Some(dir.path().to_path_buf()), db).unwrap();

        lua.globals()
            .get::<mlua::Function>("notify")
            .expect("notify should be a real function");
    }

    #[test]
    fn test_spyweb_assertions_work() {
        let (dir, db) = temp_db();
        let ok_script = r#"
            spyweb.assert_eq(1, 1)
            spyweb.assert_ne(1, 2)
        "#;
        run_script(dir.path(), Arc::clone(&db), ok_script).unwrap();

        let err = run_script(dir.path(), db, "spyweb.assert_eq(1, 2)")
            .unwrap_err()
            .to_string();
        assert!(err.contains("assertion failed"), "{err}");
    }

    #[test]
    fn test_filesystem_is_root_relative() {
        let project = target_tempdir("startup_fs");
        let (_db_dir, db) = temp_db();
        let script = r#"
            fs_overwrite("state.txt", "hello")
            assert(fs_read("state.txt") == "hello")
            fs_append("state.txt", " world")
            assert(fs_read("state.txt") == "hello world")
        "#;

        run_script(project.path(), db, script).unwrap();

        assert_eq!(
            fs::read_to_string(project.path().join("state.txt")).unwrap(),
            "hello world"
        );
    }

    #[test]
    fn test_log_writes_to_startup_log() {
        let project = target_tempdir("startup_log");
        let (_db_dir, db) = temp_db();

        run_script(project.path(), db, r#"log("hello from startup")"#).unwrap();

        let log = fs::read_to_string(project.path().join("startup.log")).unwrap();
        assert!(log.contains("hello from startup"), "{log}");
        assert!(!project.path().join("hooks.log").exists());
    }

    #[cfg(feature = "luau")]
    #[test]
    fn test_require_from_project_root() {
        let project = target_tempdir("startup_require");
        let (_db_dir, db) = temp_db();
        fs::write(project.path().join("helper.lua"), "return { value = 42 }").unwrap();
        let script = r#"
            local helper = require("helper")
            assert(helper.value == 42)
        "#;

        run_script(project.path(), db, script).unwrap();
    }

    #[test]
    fn test_startup_runs_before_job_hooks() {
        let project = tempfile::TempDir::with_prefix("startup_before_hooks").unwrap();
        let job_dir = project.path().join("jobs").join("demo");
        fs::create_dir_all(&job_dir).unwrap();
        fs::write(
            job_dir.join("config.toml"),
            r#"
name = "demo"
url = "http://example.com"
selector = "div.item"
fields = ["title: h1"]
"#,
        )
        .unwrap();
        fs::write(
            job_dir.join("hooks.lua"),
            r#"
local value = global_store_get("startup_seed")
assert(value == "ready", "hook did not see startup value")
global_store_set("hook_seen", "yes")
"#,
        )
        .unwrap();
        let startup_path = write_startup(
            project.path(),
            r#"global_store_set("startup_seed", "ready")"#,
        );
        let (_db_dir, db) = temp_db();

        run_file(&startup_path, Arc::clone(&db)).unwrap();
        let jobs = loader::load_all_jobs(
            project.path().join("missing.toml").to_str().unwrap(),
            project.path().join("jobs").to_str().unwrap(),
            Arc::clone(&db),
        )
        .unwrap();

        assert_eq!(jobs.list.len(), 1);
        assert!(jobs.list[0].hooks.is_some());
        assert_eq!(db.lua_get("hook_seen").unwrap().as_deref(), Some("yes"));
    }
}
