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

    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe().unwrap();
        std::process::Command::new("cmd")
            .arg("/K")
            .arg(format!("\"{}\"", exe.to_str().unwrap()))
            .spawn()
            .expect("failed to spawn terminal");
        std::process::exit(0);
    }

    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().unwrap();
        std::process::Command::new("open")
            .args(["-a", "Terminal", exe.to_str().unwrap()])
            .spawn()
            .unwrap();
        std::process::exit(0);
    }

    #[cfg(target_os = "linux")]
    {
        let exe = std::env::current_exe().unwrap();
        for term in &["x-terminal-emulator", "gnome-terminal", "xterm", "konsole"] {
            if std::process::Command::new(term)
                .args(["-e", exe.to_str().unwrap()])
                .spawn()
                .is_ok()
            {
                std::process::exit(0);
            }
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
            if let Err(e) = crate::entry::start_app() {
                eprint!("Application error: {}", e);
                std::process::exit(1);
            }
            Ok(())
        }
        Some("debug") => match args.get(2) {
            Some(job_name) => smol::block_on(crate::scraper::runner::debug_job(job_name)),
            None => {
                eprintln!("Usage: spyweb debug <job_name>");
                std::process::exit(1);
            }
        },
        Some("v") | Some("-v") | Some("version") => {
            let engine = if cfg!(feature = "luau") {
                "Luau"
            } else {
                "Lua 5.4"
            };
            println!("SpyWeb v{} (Engine: {})", env!("CARGO_PKG_VERSION"), engine);
            std::process::exit(0);
        }
        _ => {
            eprintln!("Usage: spyweb <check|start|debug <job_name>|version>");
            std::process::exit(1);
        }
    }
}
