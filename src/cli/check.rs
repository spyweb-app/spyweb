use std::{fs, io::Read, path::Path};

use clap::Subcommand;

use crate::config::types::{JobConfig, JobFile};

const UPDATE_ENDPOINT: &str = "https://spyweb.app/latest.json";

#[derive(Subcommand)]
pub enum CheckSub {
    Config,
    Update,
}

pub fn print_version() {
    let engine = if cfg!(feature = "luau") {
        "Luau"
    } else {
        "Lua 5.4"
    };
    let db = if cfg!(feature = "sqlite") {
        "SQLite"
    } else {
        "redb"
    };
    println!(
        "{} {} (Engine: {}, DB: {})",
        crate::color::c_bold("SpyWeb"),
        crate::color::c_ok(&format!("v{}", env!("CARGO_PKG_VERSION"))),
        crate::color::c_info(engine),
        crate::color::c_info(db),
    );
}

pub fn run_check(sub: Option<CheckSub>) -> anyhow::Result<()> {
    match sub {
        None => {
            print_version();
            println!(
                "{}",
                crate::color::c_dim("────────────────────────────────────────────")
            );
            let _ = config_check();
            println!(
                "{}",
                crate::color::c_dim("────────────────────────────────────────────")
            );
            check_update()
        }
        Some(CheckSub::Config) => {
            if let Err(e) = config_check() {
                crate::t_eprintln!("{}", e);
                std::process::exit(1);
            }
            Ok(())
        }
        Some(CheckSub::Update) => {
            print_version();
            check_update()
        }
    }
}

fn fetch_json(url: &str, agent: &ureq::Agent) -> Option<serde_json::Value> {
    let mut response = agent.get(url).header("User-Agent", "spyweb").call().ok()?;
    let mut buf = Vec::new();
    response.body_mut().as_reader().read_to_end(&mut buf).ok()?;
    let body = String::from_utf8(buf).ok()?;
    serde_json::from_str(&body).ok()
}

fn check_update() -> anyhow::Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder().build().into();

    let json = match fetch_json(UPDATE_ENDPOINT, &agent) {
        Some(v) => v,
        None => {
            println!("  Could not check for updates: network error");
            return Ok(());
        }
    };

    let local = env!("CARGO_PKG_VERSION");
    let channel = if local.contains("-beta") {
        "beta"
    } else {
        "stable"
    };

    let channel_data = match json.get(channel) {
        Some(c) => c,
        None => {
            println!(
                "  Could not check for updates: no data for channel '{}'",
                channel
            );
            return Ok(());
        }
    };

    let latest = match channel_data.get("latest_version").and_then(|v| v.as_str()) {
        Some(v) => v,
        None => {
            println!("  Could not check for updates: invalid version data");
            return Ok(());
        }
    };

    if latest == local {
        println!(
            "{} - up to date",
            crate::color::c_ok(&format!("v{}", local)),
        );
    } else {
        let display = format!("v{}", latest);
        println!(
            "{} Update available: {} - run {} to update",
            crate::color::c_warn("[⚠]"),
            crate::color::c_info(&display),
            crate::color::c_warn("spyweb update")
        );
    }

    Ok(())
}

fn count_test_functions(source: &str) -> usize {
    source
        .lines()
        .filter(|line| {
            let t = line.trim();
            t.starts_with("function test_") || (t.starts_with("test_") && t.contains("= function"))
        })
        .count()
}

fn check_lua_file(
    lua: &mlua::Lua,
    dir: &Path,
    filename: &str,
    errors: &mut usize,
) -> Option<String> {
    let path = dir.join(filename);
    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            *errors += 1;
            crate::t_eprintln!("Failed to read {}: {}", path.display(), e);
            return None;
        }
    };
    match crate::config::validate::validate_lua_syntax(lua, &source, filename) {
        Ok(()) => Some(source),
        Err(e) => {
            *errors += 1;
            crate::t_eprintln!("Syntax error in {}: {}", path.display(), e);
            None
        }
    }
}

fn config_check() -> anyhow::Result<()> {
    let jobs_dir = Path::new("jobs");
    let lua = mlua::Lua::new();
    let mut entries: Vec<(JobConfig, Option<std::path::PathBuf>)> = Vec::new();
    let mut errors = 0usize;

    // Parse jobs.toml
    let file_path = Path::new("jobs.toml");
    if file_path.exists() {
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                crate::t_eprintln!("Failed to read jobs.toml: {}", e);
                return Ok(());
            }
        };
        let file: JobFile = match toml::from_str(&content) {
            Ok(f) => f,
            Err(e) => {
                crate::t_eprintln!("Config error in 'jobs.toml': {}", e);
                return Ok(());
            }
        };
        for config in file.jobs {
            entries.push((config, None));
        }
    }

    // Parse jobs/*/config.toml
    if jobs_dir.exists() {
        let dir_entries = match fs::read_dir(jobs_dir) {
            Ok(d) => d,
            Err(e) => {
                crate::t_eprintln!("Failed to read jobs directory: {}", e);
                return Ok(());
            }
        };
        for entry in dir_entries {
            let dir = match entry {
                Ok(d) => d.path(),
                Err(_) => continue,
            };
            if !dir.is_dir() {
                continue;
            }
            let config_path = dir.join("config.toml");
            if !config_path.exists() {
                continue;
            }
            let content = match fs::read_to_string(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    errors += 1;
                    crate::t_eprintln!("Failed to read {}: {}", config_path.display(), e);
                    continue;
                }
            };
            let config: JobConfig = match toml::from_str(&content) {
                Ok(c) => c,
                Err(e) => {
                    errors += 1;
                    crate::t_eprintln!("Config error in '{}': {}", config_path.display(), e);
                    continue;
                }
            };
            entries.push((config, Some(dir)));
        }
    }

    // Enabled first, then disabled; alphabetically within each group
    entries.sort_by(|a, b| b.0.enabled.cmp(&a.0.enabled).then(a.0.name.cmp(&b.0.name)));

    // Print header
    println!(
        "Found {} job(s):",
        crate::color::c_info(&entries.len().to_string())
    );

    // Validate each job
    for (config, dir) in &entries {
        let mut files: Vec<String> = Vec::new();

        if let Some(dir) = dir {
            // hooks.lua
            if check_lua_file(&lua, dir, "hooks.lua", &mut errors).is_some() {
                files.push(crate::color::c_info("hooks.lua").to_string());
            } else if dir.join("hooks.lua").exists() {
                files.push(crate::color::c_err("hooks.lua").to_string());
            }

            // defer.lua
            if check_lua_file(&lua, dir, "defer.lua", &mut errors).is_some() {
                files.push(crate::color::c_info("defer.lua").to_string());
            }

            // tests.lua
            if let Some(source) = check_lua_file(&lua, dir, "tests.lua", &mut errors) {
                let count = count_test_functions(&source);
                let label = if count > 0 {
                    format!(
                        "tests.lua ({} test{})",
                        count,
                        if count == 1 { "" } else { "s" }
                    )
                } else {
                    "tests.lua".to_string()
                };
                files.push(crate::color::c_info(&label).to_string());
            }
        }

        let status = if config.enabled {
            crate::color::c_ok("enabled")
        } else {
            crate::color::c_dim("disabled")
        };
        let files_str = if files.is_empty() {
            crate::color::c_dim("no hook").to_string()
        } else {
            files.join(", ")
        };
        println!(
            "  [{}] {} ({})",
            status,
            crate::color::c_job(&config.name),
            files_str,
        );
    }

    // server/init.lua
    if check_lua_file(&lua, Path::new("."), "server/init.lua", &mut errors).is_some() {
        println!("  {} server/init.lua", crate::color::c_ok("✓"));
    }

    if errors > 0 {
        crate::t_eprintln!("Config has errors (see above)");
        return Err(anyhow::anyhow!("config check failed"));
    } else {
        println!("{}", crate::color::c_ok("Config OK"));
    }

    Ok(())
}
