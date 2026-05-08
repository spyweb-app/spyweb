use std::sync::Arc;

use crate::config::loader;
use crate::config::types::{Job, Jobs};
use crate::services::{db::Db, profiles};

#[cfg(target_os = "windows")]
fn is_persistent_terminal() -> bool {
    unsafe {
        unsafe extern "system" {
            fn GetConsoleProcessList(lpdwprocesslist: *mut u32, dwprocesscount: u32) -> u32;
        }
        let mut pids = [0u32; 2];
        let count = GetConsoleProcessList(pids.as_mut_ptr(), 2);
        count >= 2
    }
}

// this thing is sooo unrealiable
// #[cfg(target_os = "windows")]
// fn is_persistent_terminal() -> bool {
//     std::env::var("PROMPT").is_ok()      // cmd.exe sets this
//     || std::env::var("PSModulePath").is_ok() // PowerShell sets this
// }

#[cfg(not(target_os = "windows"))]
fn is_persistent_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

fn spawn_terminal_if_needed() {
    if std::env::args().len() > 1 {
        return;
    }

    if is_persistent_terminal() {
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            crate::t_eprintln!("Failed to determine executable path: {}", err);
            return;
        }
    };

    #[cfg(target_os = "windows")]
    {
        if let Err(err) = std::process::Command::new("cmd")
            .arg("/K")
            .arg(format!("\"{}\"", exe.display()))
            .spawn()
        {
            crate::t_eprintln!("Failed to spawn terminal: {}", err);
            return;
        }
        std::process::exit(0);
    }

    #[cfg(target_os = "macos")]
    {
        if let Err(err) = std::process::Command::new("open")
            .args(["-a", "Terminal"])
            .arg(&exe)
            .spawn()
        {
            crate::t_eprintln!("Failed to spawn Terminal.app: {}", err);
            return;
        }
        std::process::exit(0);
    }

    #[cfg(target_os = "linux")]
    {
        for term in &["x-terminal-emulator", "gnome-terminal", "xterm", "konsole"] {
            if std::process::Command::new(term)
                .arg("-e")
                .arg(&exe)
                .spawn()
                .is_ok()
            {
                std::process::exit(0);
            }
        }

        crate::t_eprintln!("Failed to spawn a terminal with known Linux terminal emulators.");
    }
}

fn parse_start_port(args: &[String]) -> anyhow::Result<Option<u16>> {
    let mut port = None;
    let mut i = 2;

    while i < args.len() {
        let arg = &args[i];

        if let Some(value) = arg.strip_prefix("--port=") {
            let parsed = value
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("Invalid value for --port: {}", value))?;
            if port.replace(parsed).is_some() {
                return Err(anyhow::anyhow!(
                    "The --port flag may only be specified once"
                ));
            }
        } else if arg == "--port" {
            let Some(value) = args.get(i + 1) else {
                return Err(anyhow::anyhow!("Missing value for --port"));
            };
            let parsed = value
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("Invalid value for --port: {}", value))?;
            if port.replace(parsed).is_some() {
                return Err(anyhow::anyhow!(
                    "The --port flag may only be specified once"
                ));
            }
            i += 1;
        }

        i += 1;
    }

    Ok(port)
}

fn load_jobs_for_profiles() -> anyhow::Result<Jobs> {
    let db = Arc::new(Db::open("data")?);
    loader::load_all_jobs("jobs.toml", "jobs", db)
}

fn profile_usage() {
    eprintln!(
        "Usage: {} {} <{}|{}|{} <job|all>|{} <job|all>>",
        crate::color::c_bold("spyweb"),
        crate::color::c_info("profile"),
        crate::color::c_info("check"),
        crate::color::c_info("list"),
        crate::color::c_info("clear"),
        crate::color::c_info("delete")
    );
}

fn print_profile_check(job: &Job) -> anyhow::Result<()> {
    let summary = profiles::profile_summary(job)?;
    println!("{}", summary);
    Ok(())
}

fn print_profile_check_all(jobs: &Jobs) -> anyhow::Result<()> {
    let mut shown = 0usize;
    for job in &jobs.list {
        if job.dir.is_none() {
            continue;
        }
        print_profile_check(job)?;
        shown += 1;
    }

    if shown == 0 {
        println!("{}", crate::color::c_dim("No job profiles found."));
    }

    Ok(())
}

fn resolve_job<'a>(jobs: &'a Jobs, target: &str) -> anyhow::Result<&'a Job> {
    profiles::find_job(jobs, target).ok_or_else(|| {
        anyhow::anyhow!(
            "Job '{}' not found. Use `spyweb check` to list jobs.",
            target
        )
    })
}

fn clear_job_profile(job: &Job) -> anyhow::Result<()> {
    let Some(path) = profiles::profile_dir_for_job(job) else {
        println!(
            "{} has no profile directory",
            crate::color::c_job(&job.config.name)
        );
        return Ok(());
    };

    profiles::clear_profile_dir(&path)?;
    println!(
        "Cleared profile for {} at {}",
        crate::color::c_job(&job.config.name),
        path.display()
    );
    Ok(())
}

fn delete_job_profile(job: &Job) -> anyhow::Result<()> {
    let Some(path) = profiles::profile_dir_for_job(job) else {
        println!(
            "{} has no profile directory",
            crate::color::c_job(&job.config.name)
        );
        return Ok(());
    };

    profiles::delete_profile_dir(&path)?;
    println!(
        "Deleted profile for {} at {}",
        crate::color::c_job(&job.config.name),
        path.display()
    );
    Ok(())
}

fn handle_profile_command(args: &[String]) -> anyhow::Result<()> {
    let Some(cmd) = args.get(2).map(String::as_str) else {
        profile_usage();
        std::process::exit(1);
    };

    let jobs = load_jobs_for_profiles()?;

    match cmd {
        "check" | "list" => {
            match args.get(3).map(String::as_str) {
                None | Some("all") | Some("--all") => print_profile_check_all(&jobs)?,
                Some(target) => print_profile_check(resolve_job(&jobs, target)?)?,
            }
            Ok(())
        }
        "clear" => {
            match args.get(3).map(String::as_str) {
                Some("all") | Some("--all") => {
                    for job in &jobs.list {
                        if job.dir.is_none() {
                            continue;
                        }
                        clear_job_profile(job)?;
                    }
                }
                Some(target) => clear_job_profile(resolve_job(&jobs, target)?)?,
                None => {
                    profile_usage();
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        "delete" => {
            match args.get(3).map(String::as_str) {
                Some("all") | Some("--all") => {
                    for job in &jobs.list {
                        if job.dir.is_none() {
                            continue;
                        }
                        delete_job_profile(job)?;
                    }
                }
                Some(target) => delete_job_profile(resolve_job(&jobs, target)?)?,
                None => {
                    profile_usage();
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        _ => {
            profile_usage();
            std::process::exit(1);
        }
    }
}

pub fn listen_command() -> anyhow::Result<()> {
    // lets spawn a terminal on dboule click if needed
    // too lazy to cd
    spawn_terminal_if_needed();

    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("check") => crate::config::config_check(),
        Some("start") => {
            let port = parse_start_port(&args)?;
            if let Err(e) = crate::entry::start_app_with_port(port) {
                crate::t_eprintln!("Application error: {}", e);
                std::process::exit(1);
            }
            Ok(())
        }
        Some("debug") => match args.get(2) {
            Some(job_name) => smol::block_on(crate::scraper::runner::debug_job(job_name)),
            None => {
                eprintln!(
                    "Usage: {} {} <job_name>",
                    crate::color::c_bold("spyweb"),
                    crate::color::c_info("debug")
                );
                std::process::exit(1);
            }
        },
        Some("profile") | Some("profiles") => handle_profile_command(&args),
        Some("v") | Some("-v") | Some("version") => {
            let engine = if cfg!(feature = "luau") {
                "Luau"
            } else {
                "Lua 5.4"
            };
            println!(
                "{} {} (Engine: {})",
                crate::color::c_bold("SpyWeb"),
                crate::color::c_ok(&format!("v{}", env!("CARGO_PKG_VERSION"))),
                crate::color::c_info(engine)
            );
            std::process::exit(0);
        }
        _ => {
            eprintln!(
                "Usage: {} <{}|{}|{} <job_name>|{}|{} <job|all>>",
                crate::color::c_bold("spyweb"),
                crate::color::c_info("check"),
                crate::color::c_info("start"),
                crate::color::c_info("debug"),
                crate::color::c_info("profile"),
                crate::color::c_info("version")
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_start_port, resolve_job};
    use crate::config::types::{Field, Job, JobConfig, Jobs};

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_port_after_start() {
        let Ok(parsed) = parse_start_port(&args(&["spyweb", "start", "--port", "9999"])) else {
            panic!("valid --port syntax should parse");
        };
        assert_eq!(parsed, Some(9999));
    }

    #[test]
    fn parses_port_equals_syntax() {
        let Ok(parsed) = parse_start_port(&args(&["spyweb", "start", "--port=8888"])) else {
            panic!("valid --port= syntax should parse");
        };
        assert_eq!(parsed, Some(8888));
    }

    #[test]
    fn allows_start_without_port() {
        let Ok(parsed) = parse_start_port(&args(&["spyweb", "start"])) else {
            panic!("missing optional --port should still parse");
        };
        assert_eq!(parsed, None);
    }

    fn mock_job(dir: Option<&str>, name: &str) -> Job {
        Job {
            config: JobConfig {
                name: name.to_string(),
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
            },
            hooks: None,
            dir: dir.map(|d| std::path::PathBuf::from(d)),
        }
    }

    #[test]
    fn resolves_job_by_name_or_id() {
        let jobs = Jobs {
            list: vec![mock_job(Some("jobs/jumia"), "Jumia")],
        };
        assert!(resolve_job(&jobs, "Jumia").is_ok());
        assert!(resolve_job(&jobs, "jumia").is_ok());
    }
}
