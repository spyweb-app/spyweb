use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use mlua::Function;
use smol::lock::Mutex;
use tempfile::NamedTempFile;

use crate::config::loader;
use crate::config::types::{Job, Jobs};
use crate::lua::server::ApiServer;
use crate::lua::{JobHooks, engine, hooks::source_uses_cdp};
use crate::services::db::Db;
use crate::services::profiles;
use crate::services::server::types;

pub fn run_tests(job_name_filter: Option<String>, pattern_filter: Option<String>) -> Result<()> {
    smol::block_on(run_tests_async(job_name_filter, pattern_filter))
}

async fn run_tests_async(
    job_name_filter: Option<String>,
    pattern_filter: Option<String>,
) -> Result<()> {
    let temp_db_file = NamedTempFile::new()?;
    let dummy_db = Arc::new(Db::open(&temp_db_file.path().to_string_lossy())?); // Needed for loader
    let jobs = Jobs {
        list: loader::load_dir_jobs("jobs", dummy_db.clone())?,
    };

    let filtered_jobs: Vec<&Job> = if let Some(ref name) = job_name_filter {
        match profiles::find_job(&jobs, name) {
            Some(job) => vec![job],
            None => vec![],
        }
    } else {
        jobs.list.iter().collect()
    };

    let server_dir = std::path::Path::new("server");
    let has_server_tests =
        server_dir.join("init.lua").exists() && server_dir.join("tests.lua").exists();

    if filtered_jobs.is_empty() && !has_server_tests {
        if let Some(name) = job_name_filter {
            println!("No jobs found matching '{}'", crate::color::c_err(&name));
        } else {
            println!("No jobs found to test.");
        }
        return Ok(());
    }

    let mut total_passed = 0;
    let mut total_failed = 0;
    let start_time = Instant::now();

    for job in filtered_jobs {
        let job_dir = match &job.dir {
            Some(dir) => dir,
            None => continue,
        };

        let tests = discover_tests(job_dir, &job.config.name)?;
        if tests.is_empty() {
            continue;
        }

        let filtered_tests = filter_tests_by_pattern(tests, pattern_filter.as_deref());

        if filtered_tests.is_empty() {
            continue;
        }

        print_test_header(&job.config.name, filtered_tests.len());

        for test_name in filtered_tests {
            print!("  test {:<32}", crate::color::c_info(&test_name));
            match run_single_test(job_dir, &job.config.name, &test_name).await {
                Ok(_) => {
                    println!(" ... {}", crate::color::c_ok("OK"));
                    total_passed += 1;
                }
                Err(e) => {
                    println!(" ... {}", crate::color::c_err("FAILED"));
                    print_failure_block(&test_name, &e);
                    total_failed += 1;
                }
            }
        }
    }

    if has_server_tests {
        let server_tests = match discover_tests(server_dir, "__server") {
            Ok(tests) => tests,
            Err(e) => {
                eprintln!("  {} server/tests.lua: {:#}", crate::color::c_err("✗"), e);
                Vec::new()
            }
        };
        let filtered_tests = filter_tests_by_pattern(server_tests, pattern_filter.as_deref());

        if !filtered_tests.is_empty() {
            let server_db_file = NamedTempFile::new()?;
            let server_db_path = server_db_file
                .path()
                .to_str()
                .context("Failed to convert temp db path to string")?
                .to_string();
            let server_db = Arc::new(Db::open(&server_db_path)?);
            let port = start_server_for_tests(server_db.clone())?;

            print_test_header("server", filtered_tests.len());

            for test_name in filtered_tests {
                print!("  test {:<32}", crate::color::c_info(&test_name));
                match run_single_server_test(server_dir, &test_name, port, server_db.clone()).await
                {
                    Ok(_) => {
                        println!(" ... {}", crate::color::c_ok("OK"));
                        total_passed += 1;
                    }
                    Err(e) => {
                        println!(" ... {}", crate::color::c_err("FAILED"));
                        print_failure_block(&test_name, &e);
                        total_failed += 1;
                    }
                }
            }
        }
    }

    let duration = start_time.elapsed();
    println!(
        "Test Result: {}. ({}) Passed; ({}) Failed; Finished in {}",
        if total_failed == 0 {
            crate::color::c_ok("PASSED")
        } else {
            crate::color::c_err("FAILED")
        },
        crate::color::c_ok(&total_passed.to_string()),
        crate::color::c_err(&total_failed.to_string()),
        crate::color::c_info(&crate::services::utils::format_duration(duration)),
    );

    if total_failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

fn print_test_header(job_name: &str, test_count: usize) {
    println!();
    println!(
        "{}",
        crate::color::c_bold(&format!(
            "Tests for {} ({})",
            crate::color::c_job(job_name),
            crate::color::c_info(&test_count.to_string())
        ))
    );
    println!(
        "{}",
        crate::color::c_dim("------------------------------------------------------------")
    );
}

fn print_failure_block(test_name: &str, err: &anyhow::Error) {
    println!();
    println!("failures:");
    println!();
    println!("---- {} stdout ----", test_name);
    println!("{:?}", err);
}

pub fn discover_tests(job_dir: &Path, job_name: &str) -> Result<Vec<String>> {
    let mut tests = Vec::new();

    // We need a temporary VM just to discover tests
    // Using a fake DB for discovery
    let temp_db_file = NamedTempFile::new()?;
    let db = Arc::new(Db::open(&temp_db_file.path().to_string_lossy())?);
    let uses_cdp = job_source_uses_cdp(job_dir)?;
    let lua = engine::create_engine(Some(job_dir.to_path_buf()), db, job_name, uses_cdp)?;

    load_test_sources(&lua, job_dir)?;

    let globals = lua.globals();
    for pair in globals.pairs::<String, mlua::Value>() {
        let (name, value) = pair?;
        if name.starts_with("test_") && matches!(value, mlua::Value::Function(_)) {
            tests.push(name);
        }
    }

    tests.sort_unstable();
    tests.dedup();

    Ok(tests)
}

pub async fn run_single_test(job_dir: &Path, job_name: &str, test_name: &str) -> Result<()> {
    let temp_db_file = NamedTempFile::new()?;
    let temp_db = temp_db_file
        .path()
        .to_str()
        .context("Failed to convert temp db path to string")?;
    let db = Arc::new(Db::open(temp_db)?);
    let uses_cdp = job_source_uses_cdp(job_dir)?;
    let lua = engine::create_engine(Some(job_dir.to_path_buf()), db, job_name, uses_cdp)?;

    load_test_sources(&lua, job_dir)?;

    let hooks = JobHooks {
        lua: Mutex::new(lua),
        job_name: job_name.to_string(),
        hook_path: job_dir.join("hooks.lua"),
        hook_mask: 0,
    };

    let ctx = hooks.new_cycle_context(0).await?;
    {
        let lua = hooks.lua.lock().await;
        lua.set_named_registry_value("active_ctx", ctx)?;
    }

    let test_fn: Function = {
        let lua = hooks.lua.lock().await;
        lua.globals().get(test_name)?
    };

    test_fn
        .call_async::<()>(())
        .await
        .with_context(|| format!("Test {} failed", crate::color::c_err(test_name)))?;

    {
        let lua = hooks.lua.lock().await;
        let _ = lua.set_named_registry_value("active_ctx", mlua::Value::Nil);
    }

    Ok(())
}

fn start_server_for_tests(db: Arc<Db>) -> Result<u16> {
    let init_source =
        std::fs::read_to_string("server/init.lua").context("Failed to read server/init.lua")?;
    let api_server = Arc::new(ApiServer::new(
        db,
        init_source,
        std::path::PathBuf::from("/dev/null"),
    ));

    let server = rouille::Server::new("127.0.0.1:0", move |rouille_req| {
        let req = types::from_rouille_request(rouille_req);
        if req.url.starts_with("/api/v/") {
            let path = req.url.strip_prefix("/api/v/").unwrap_or("");
            let mut segments: Vec<String> = path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            if !segments.is_empty() {
                let name = segments.remove(0);
                let resp = api_server.handle(req.method.as_str(), &name, segments, &req);
                return types::to_rouille_response(resp);
            }
        }
        rouille::Response::text("Not Found").with_status_code(404)
    })
    .map_err(|e| anyhow::anyhow!("Failed to start server for tests: {}", e))?;

    let port = server.server_addr().port();
    std::thread::spawn(move || server.run());
    Ok(port)
}

async fn run_single_server_test(dir: &Path, test_name: &str, port: u16, db: Arc<Db>) -> Result<()> {
    let lua = engine::create_engine(Some(dir.to_path_buf()), db, "__server", false)?;

    lua.globals().set("SERVER_PORT", port as i64)?;

    let init_path = dir.join("init.lua");
    if init_path.exists() {
        let globals = lua.globals();
        for m in &["get", "post", "put", "patch", "delete", "all"] {
            let _ = globals.set(*m, lua.create_table()?);
        }
        load_lua_file(&lua, &init_path)?;
    }

    load_lua_file(&lua, &dir.join("tests.lua"))?;

    let test_fn: Function = lua.globals().get(test_name)?;
    test_fn
        .call_async::<()>(())
        .await
        .with_context(|| format!("Test {} failed", crate::color::c_err(test_name)))?;

    Ok(())
}

fn load_test_sources(lua: &mlua::Lua, job_dir: &Path) -> Result<()> {
    for file in [
        job_dir.join("hooks.lua"),
        job_dir.join("defer.lua"),
        job_dir.join("tests.lua"),
    ] {
        if file.exists() {
            load_lua_file(lua, &file)?;
        }
    }

    Ok(())
}

fn job_source_uses_cdp(job_dir: &Path) -> Result<bool> {
    let mut source = String::new();
    for file in [
        job_dir.join("hooks.lua"),
        job_dir.join("defer.lua"),
        job_dir.join("tests.lua"),
    ] {
        if file.exists() {
            source.push_str(&std::fs::read_to_string(&file)?);
            source.push('\n');
        }
    }

    Ok(source_uses_cdp(&source))
}

fn load_lua_file(lua: &mlua::Lua, file: &Path) -> Result<()> {
    let source = std::fs::read_to_string(file)
        .with_context(|| format!("Failed to read {}", file.display()))?;
    lua.load(&source)
        .set_name(file.to_string_lossy().as_ref())
        .exec()
        .map_err(|e| anyhow::anyhow!("Failed to load {}: {}", file.display(), e))?;
    Ok(())
}

pub fn filter_tests_by_pattern(tests: Vec<String>, pattern: Option<&str>) -> Vec<String> {
    match pattern {
        Some(pattern) => tests
            .into_iter()
            .filter(|test_name| test_name.contains(pattern))
            .collect(),
        None => tests,
    }
}

#[cfg(test)]
mod tests {
    use super::{discover_tests, filter_tests_by_pattern, run_single_test};
    use crate::config::types::{Field, Job, JobConfig};
    use crate::services::profiles;
    use crate::services::server::types;
    use std::fs;
    use tempfile::tempdir;

    fn write_job_file(dir: &std::path::Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn discovers_tests_in_stable_order_across_files() {
        let tempdir = tempdir().unwrap();
        write_job_file(
            tempdir.path(),
            "hooks.lua",
            r#"
function test_beta()
end

test_marker = true
"#,
        );
        write_job_file(
            tempdir.path(),
            "tests.lua",
            r#"
function test_alpha()
end

helper = function()
end
"#,
        );

        let tests = discover_tests(tempdir.path(), "demo_job").unwrap();
        assert_eq!(
            tests,
            vec!["test_alpha".to_string(), "test_beta".to_string()]
        );
    }

    #[test]
    fn runs_async_lua_tests_with_production_bindings() {
        let server = types::MockServer::start(|request| {
            if request.method == "GET" {
                types::Response::text("runner-get-ok")
            } else if request.method == "POST" {
                let body = request.body_bytes().unwrap_or_default();
                let body_str = String::from_utf8_lossy(body);
                assert_eq!(body_str, "runner-post-body");
                types::Response::text("runner-post-ok")
            } else {
                types::Response::empty_404()
            }
        });
        let port = server.port();

        let tempdir = tempdir().unwrap();
        let hooks_source = format!(
            r#"
function test_async_bindings()
    spyweb.assert_eq(defer_helper(), "defer-ready")

    local get_response = http_get("http://127.0.0.1:{port}/")
    spyweb.assert_eq(get_response.status, 200)
    spyweb.assert_eq(get_response.body, "runner-get-ok")

    local post_response = http_post("http://127.0.0.1:{port}/", "runner-post-body")
    spyweb.assert_eq(post_response.status, 200)
    spyweb.assert_eq(post_response.body, "runner-post-ok")
end
"#
        );
        write_job_file(tempdir.path(), "hooks.lua", &hooks_source);
        write_job_file(
            tempdir.path(),
            "defer.lua",
            r#"
function defer_helper()
    return "defer-ready"
end
"#,
        );

        smol::block_on(async {
            run_single_test(tempdir.path(), "demo_job", "test_async_bindings")
                .await
                .unwrap();
        });
    }

    #[test]
    fn each_test_run_gets_a_fresh_lua_vm() {
        let tempdir = tempdir().unwrap();
        write_job_file(
            tempdir.path(),
            "hooks.lua",
            r#"
function test_mutates_global_state()
    leaked_state = "set during the first run"
    spyweb.assert_eq(leaked_state, "set during the first run")
end

function test_does_not_see_previous_run_state()
    spyweb.assert_eq(leaked_state, nil)
end
"#,
        );

        smol::block_on(async {
            run_single_test(tempdir.path(), "demo_job", "test_mutates_global_state")
                .await
                .unwrap();

            run_single_test(
                tempdir.path(),
                "demo_job",
                "test_does_not_see_previous_run_state",
            )
            .await
            .unwrap();
        });
    }

    #[test]
    fn filters_tests_by_pattern() {
        let tests = vec![
            "test_price_cleanup".to_string(),
            "test_name_cleanup".to_string(),
            "test_price_parse".to_string(),
        ];

        assert_eq!(
            filter_tests_by_pattern(tests.clone(), Some("price")),
            vec![
                "test_price_cleanup".to_string(),
                "test_price_parse".to_string(),
            ]
        );
        assert_eq!(
            filter_tests_by_pattern(tests, None),
            vec![
                "test_price_cleanup".to_string(),
                "test_name_cleanup".to_string(),
                "test_price_parse".to_string(),
            ]
        );
    }

    #[test]
    fn profiles_find_job_requires_exact_name_or_id_match() {
        let jobs = crate::config::types::Jobs {
            list: vec![Job {
                config: JobConfig {
                    name: "Price Sync".to_string(),
                    url: "https://example.com".to_string(),
                    selector: ".item".to_string(),
                    fields: vec![Field::Shorthand("title".to_string())],
                    keywords: None,
                    search_fields: None,
                    webhook: None,
                    debug: false,
                    enabled: true,
                    interval: 60,
                    proxy: None,
                    notification: None,
                    headers: None,
                    hash_fields: None,
                    workers: None,
                    urls: None,
                },
                hooks: None,
                has_hooks_file: false,
                dir: Some(std::path::PathBuf::from("jobs/price-sync")),
            }],
        };

        assert!(profiles::find_job(&jobs, "Price Sync").is_some());
        assert!(profiles::find_job(&jobs, "price_sync").is_some());
        assert!(profiles::find_job(&jobs, "price").is_none());
    }
}
