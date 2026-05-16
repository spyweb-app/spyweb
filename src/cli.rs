use std::sync::Arc;

use clap::{Parser, Subcommand};

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

#[derive(Parser)]
#[command(
    name = "spyweb",
    version,
    about = "Tiny web monitoring/scraper with Lua scripting",
    disable_version_flag = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Check,
    Start {
        #[arg(long)]
        port: Option<u16>,
    },
    Debug {
        job_name: String,
    },
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },
    #[command(aliases = ["v"])]
    Version,
}

#[derive(Subcommand)]
enum ProfileCommands {
    Check { job: Option<String> },
    List { job: Option<String> },
    Clear { target: String },
    Delete { target: String },
}

fn load_jobs_for_profiles() -> anyhow::Result<Jobs> {
    let db = Arc::new(Db::open("data")?);
    loader::load_all_jobs("jobs.toml", "jobs", db)
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

fn handle_profile_command(cmd: ProfileCommands) -> anyhow::Result<()> {
    let jobs = load_jobs_for_profiles()?;

    match cmd {
        ProfileCommands::Check { job } | ProfileCommands::List { job } => {
            match job.as_deref() {
                None | Some("all") | Some("--all") => print_profile_check_all(&jobs)?,
                Some(target) => print_profile_check(resolve_job(&jobs, target)?)?,
            }
            Ok(())
        }
        ProfileCommands::Clear { target } => {
            if target == "all" || target == "--all" {
                for job in &jobs.list {
                    if job.dir.is_none() {
                        continue;
                    }
                    clear_job_profile(job)?;
                }
            } else {
                clear_job_profile(resolve_job(&jobs, &target)?)?;
            }
            Ok(())
        }
        ProfileCommands::Delete { target } => {
            if target == "all" || target == "--all" {
                for job in &jobs.list {
                    if job.dir.is_none() {
                        continue;
                    }
                    delete_job_profile(job)?;
                }
            } else {
                delete_job_profile(resolve_job(&jobs, &target)?)?;
            }
            Ok(())
        }
    }
}

pub fn listen_command() -> anyhow::Result<()> {
    spawn_terminal_if_needed();

    let cli = Cli::parse();

    match cli.command {
        Commands::Check => crate::config::config_check(),
        Commands::Start { port } => {
            if let Err(e) = crate::entry::start_app_with_port(port) {
                crate::t_eprintln!("Application error: {}", e);
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::Debug { job_name } => {
            smol::block_on(crate::scraper::runner::debug_job(&job_name))
        }
        Commands::Profile { command } => handle_profile_command(command),
        Commands::Version => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_job;
    use crate::config::types::{Field, Job, JobConfig, Jobs};

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
            dir: dir.map(std::path::PathBuf::from),
        }
    }

    #[test]
    fn resolves_job_by_name_or_id() {
        let jobs = Jobs {
            list: vec![mock_job(Some("jobs/spyweb"), "Spyweb")],
        };
        assert!(resolve_job(&jobs, "SpyWeb").is_ok());
        assert!(resolve_job(&jobs, "spyweb").is_ok());
    }
}
