use anyhow::Result;
use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind};
use notify::{RecursiveMode, Watcher};
use smol::channel::Sender;
use std::path::Path;

fn is_write_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Access(AccessKind::Close(AccessMode::Write))
            | EventKind::Create(_)
            | EventKind::Remove(_)
    )
}

pub fn watch_configs(tx: Sender<()>) -> Result<()> {
    let (notify_tx, notify_rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(notify_tx)?;

    /* for path in [Path::new("jobs.toml"), Path::new("jobs")] {
        if path.exists() {
            let recursive = if path.is_dir() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher.watch(path, recursive)?;
        }
    } */

    watcher.watch(".", RecursiveMode::Recursive)?;

    for event in notify_rx {
        match event {
            Ok(event) => {
                let mut should_reload = false;
                for path in &event.paths {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

                    if (ext == "toml" || ext == "lua") && is_write_event(&event.kind) {
                        should_reload = true;
                        break;
                    }
                }

                if should_reload {
                    crate::t_println!("File changed: {:?}, reloading...", event.paths);
                    /* DEBUG: uncomment
                    crate::t_println!("[watcher] >>> triggering reload (matched .toml/.lua in {:?})", event.paths);
                    for path in &event.paths {
                        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                        if ext == "toml" || ext == "lua" {
                            crate::t_println!("[watcher]   scanning /proc for holders of: {}", path.display());
                            log_holders(path);
                        }
                    }
                    */
                    let _ = tx.try_send(());
                }
            }
            Err(e) => crate::t_eprintln!("Watch error: {:?}", e),
        }
    }
    Ok(())
}

/* DEBUG: uncomment
fn log_holders(path: &Path) {
    let canon = match std::fs::canonicalize(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let proc_dir = match std::fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return,
    };

    for entry in proc_dir.flatten() {
        let pid_str = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }

        let fd_dir = entry.path().join("fd");
        let fds = match std::fs::read_dir(&fd_dir) {
            Ok(d) => d,
            Err(_) => continue,
        };

        for fd_entry in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd_entry.path())
                && target == canon
            {
                let comm = std::fs::read_to_string(entry.path().join("comm"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                crate::t_println!("[watcher]   held by PID {} ({})", pid_str, comm);
                break;
            }
        }
    }
}
*/
