pub mod check;
pub mod profile;
pub mod update;

use clap::{Parser, Subcommand};

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
    about = "Tiny web monitoring/scraping engine with Lua scripting",
    disable_version_flag = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Check {
        #[command(subcommand)]
        command: Option<check::CheckSub>,
    },
    Start {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long = "no-server")]
        no_server: bool,
    },
    Debug {
        job_name: String,
    },
    Test {
        job_name: Option<String>,
        pattern: Option<String>,
    },
    Profile {
        #[command(subcommand)]
        command: profile::ProfileCommands,
    },
    #[command(aliases = ["v"])]
    Version,
    Types,
    Update {
        #[arg(short = 'c', long)]
        check: bool,
        #[arg(short = 'f', long)]
        force: bool,
        #[arg(short = 'k', long)]
        keep: Option<Option<String>>,
        #[arg(short = 'o', long)]
        overwrite: bool,
    },
}

pub fn listen_command() -> anyhow::Result<()> {
    spawn_terminal_if_needed();

    let cli = Cli::parse();

    match cli.command {
        Commands::Check { command } => check::run_check(command),
        Commands::Start { port, no_server } => {
            if let Err(e) = crate::entry::start_app_with_port(port, no_server) {
                crate::t_eprintln!("Application error: {}", e);
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::Debug { job_name } => {
            let res = smol::block_on(crate::scraper::runner::debug_job(&job_name));
            crate::services::utils::shutdown_system();
            res
        }
        Commands::Test { job_name, pattern } => {
            let res = crate::lua::test_runner::run_tests(job_name, pattern);
            crate::services::utils::shutdown_system();
            res
        }
        Commands::Profile { command } => profile::handle_profile_command(command),
        Commands::Version => {
            check::print_version();
            std::process::exit(0);
        }
        Commands::Types => write_types(),
        Commands::Update {
            check,
            force,
            keep,
            overwrite,
        } => update::run_update(check, force, keep, overwrite),
    }
}

fn write_types() -> anyhow::Result<()> {
    let dir = std::env::current_dir()?;
    std::fs::write(
        dir.join("spyweb-types.lua"),
        include_str!("../../spyweb-types.lua"),
    )?;
    std::fs::write(dir.join(".luarc.json"), include_str!("../../.luarc.json"))?;
    println!(
        "wrote {} and {} to {}",
        crate::color::c_info("spyweb-types.lua"),
        crate::color::c_info(".luarc.json"),
        crate::color::c_ok(&dir.display().to_string())
    );
    Ok(())
}
