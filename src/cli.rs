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
                return Err(anyhow::anyhow!("The --port flag may only be specified once"));
            }
        } else if arg == "--port" {
            let Some(value) = args.get(i + 1) else {
                return Err(anyhow::anyhow!("Missing value for --port"));
            };
            let parsed = value
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("Invalid value for --port: {}", value))?;
            if port.replace(parsed).is_some() {
                return Err(anyhow::anyhow!("The --port flag may only be specified once"));
            }
            i += 1;
        }

        i += 1;
    }

    Ok(port)
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
                "Usage: {} <{}|{}|{} <job_name>|{}>",
                crate::color::c_bold("spyweb"),
                crate::color::c_info("check"),
                crate::color::c_info("start"),
                crate::color::c_info("debug"),
                crate::color::c_info("version")
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_start_port;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_port_after_start() {
        let parsed = parse_start_port(&args(&["spyweb", "start", "--port", "9999"])).unwrap();
        assert_eq!(parsed, Some(9999));
    }

    #[test]
    fn parses_port_equals_syntax() {
        let parsed = parse_start_port(&args(&["spyweb", "start", "--port=8888"])).unwrap();
        assert_eq!(parsed, Some(8888));
    }

    #[test]
    fn allows_start_without_port() {
        let parsed = parse_start_port(&args(&["spyweb", "start"])).unwrap();
        assert_eq!(parsed, None);
    }
}
