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
    disable_version_flag = true,
    long_about = "Tiny web monitoring/scraping engine with Lua scripting.\n\nRuns scheduled scraping/monitoring jobs defined in jobs.toml or jobs/*/config.toml, and serves a dashboard + JSON API at http://127.0.0.1:7979 (change with `spyweb start --port` or SPYWEB_PORT).",
    after_help = "\nExamples:\n  spyweb start                       Run the engine and web server\n  spyweb start -nqq --port 9000      Scrape only, errors only, port 9000\n  spyweb check config                Validate config and Lua syntax\n  spyweb test                        Run all Lua tests\n  spyweb debug \"My Job\"              One-shot debug run of a job\n  spyweb update --check              Check for a new version\n  spyweb profile list                Show browser profile status"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Health check: prints version, validates configuration, checks for updates.
    ///
    /// With no subcommand this runs version + config validation + update check and
    /// always exits 0 (purely informational). Use `spyweb check config` to fail
    /// with a non-zero exit code on config errors.
    Check {
        #[command(subcommand)]
        command: Option<check::CheckSub>,
    },
    /// Start the monitoring engine and the dashboard/API web server.
    #[command(
        after_help = "Examples:\n  spyweb start\n  spyweb start --port 9000\n  spyweb start -nqq --port 9000\n\nEnvironment:\n  SPYWEB_PORT          Override the dashboard/API port (default 7979)\n  SPYWEB_DISABLE_SERVER  Same as --no-server\n  SPYWEB_LOG           Same as -q/-qq (values: info|warn|error)"
    )]
    Start {
        /// Port for the dashboard and JSON API server (default 7979)
        #[arg(long)]
        port: Option<u16>,
        /// Run the engine without the HTTP server (scraping/monitoring only)
        #[arg(short = 'n', long = "no-server")]
        no_server: bool,
        /// Reduce log output (`-q` warnings & errors only, `-qq` errors only; additional `-q`s act like `-qq`)
        #[arg(short = 'q', long, action = clap::ArgAction::Count)]
        quiet: u8,
    },
    /// Run a single job once with debug output.
    ///
    /// Runs the job even if `enabled = false`, prints extracted items and pipeline
    /// stages to the terminal, and saves response HTML + JSON artifacts in the job
    /// directory. Skips store, notification, and webhook phases.
    #[command(after_help = "Example:\n  spyweb debug \"My Job\"")]
    Debug {
        /// Job name or id to debug
        job_name: String,
    },
    /// Run Lua tests for jobs or the API server.
    ///
    /// Each test_* function runs in an isolated Lua VM with a temporary database.
    #[command(
        after_help = "Examples:\n  spyweb test               Run all tests across all jobs\n  spyweb test \"My Job\"        Run a specific job's tests\n  spyweb test \"My Job\" price  Only tests whose name contains 'price'\n  spyweb test server          Run the programmable API server test suite (auto-starts server)"
    )]
    Test {
        /// Job name to test, or `server` for the API server suite; all jobs when omitted
        job_name: Option<String>,
        /// Only run tests whose function name contains this substring
        pattern: Option<String>,
    },
    /// Manage Chrome user data directories (profiles) for CDP jobs.
    ///
    /// Inspect, wipe, or delete per-job browser profile directories used by CDP
    /// automation.
    Profile {
        #[command(subcommand)]
        command: profile::ProfileCommands,
    },
    /// Print the version and active engine.
    #[command(aliases = ["v"])]
    Version,
    /// Write Lua LSP type definitions (spyweb-types.lua + .luarc.json) to the current directory.
    Types,
    /// Self-update the spyweb binary.
    ///
    /// Downloads the latest release and swaps it in. By default the old binary is
    /// kept as a backup.
    #[command(
        after_help = "Examples:\n  spyweb update             Check, download, and replace (old kept as spyweb-v{version})\n  spyweb update -c          Check for updates without downloading\n  spyweb update -k backup   Keep the old binary as spyweb-backup\n  spyweb update -o          Discard the old binary"
    )]
    Update {
        /// Only check for updates; print the download URL without installing
        #[arg(short = 'c', long)]
        check: bool,
        /// Skip the up-to-date check and always download and replace
        #[arg(short = 'f', long)]
        force: bool,
        /// Keep the old binary as spyweb-{SUFFIX}, or spyweb-v{version} when no suffix is given
        #[arg(short = 'k', long, value_name = "SUFFIX")]
        keep: Option<Option<String>>,
        /// Discard the old binary instead of keeping a backup (cannot be combined with --keep)
        #[arg(short = 'o', long)]
        overwrite: bool,
    },
}

pub fn listen_command() -> anyhow::Result<()> {
    spawn_terminal_if_needed();
    crate::macros::init_log_level_from_env();

    let cli = Cli::parse();

    match cli.command {
        Commands::Check { command } => check::run_check(command),
        Commands::Start {
            port,
            no_server,
            quiet,
        } => {
            match quiet {
                1 => crate::macros::set_log_level(crate::macros::LOG_LEVEL_WARN),
                _ if quiet >= 2 => crate::macros::set_log_level(crate::macros::LOG_LEVEL_ERROR),
                _ => {}
            }
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
