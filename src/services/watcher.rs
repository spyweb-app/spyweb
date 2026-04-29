use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use smol::channel::Sender;
use std::path::Path;

pub fn watch_configs(tx: Sender<()>) -> Result<()> {
    let (notify_tx, notify_rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(notify_tx)?;

    // watcher.watch(Path::new("jobs.toml"), RecursiveMode::NonRecursive)?;
    // watcher.watch(Path::new("jobs"), RecursiveMode::Recursive)?;

    for path in [Path::new("jobs.toml"), Path::new("jobs")] {
        if path.exists() {
            let recursive = if path.is_dir() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher.watch(path, recursive)?;
        }
    }

    for event in notify_rx {
        match event {
            Ok(event) => {
                let mut should_reload = false;
                for path in &event.paths {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

                    if ext == "toml" || ext == "lua" {
                        should_reload = true;
                        break;
                    }
                }

                if should_reload {
                    crate::t_println!("Config changed: {:?}, reloading...", event.paths);
                    let _ = tx.try_send(());
                }
            }
            Err(e) => crate::t_eprintln!("Watch error: {:?}", e),
        }
    }
    Ok(())
}
