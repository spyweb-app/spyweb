use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use smol::channel::{bounded, Receiver, Sender};

static IO_SENDER: OnceLock<Sender<IoTask>> = OnceLock::new();

pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB
pub const MAX_ROTATIONS: usize = 5;

#[derive(Debug)]
pub enum IoOp {
    Append,
    Overwrite,
}

#[derive(Debug)]
pub struct IoTask {
    pub path: PathBuf,
    pub content: Vec<u8>,
    pub op: IoOp,
    pub add_timestamp: bool,
}

pub fn init() {
    let (tx, rx) = bounded(1024);
    IO_SENDER.set(tx).expect("IO system already initialized");

    std::thread::spawn(move || {
        smol::block_on(io_worker(rx));
    });
}

pub async fn send_task(task: IoTask) -> anyhow::Result<()> {
    let sender = IO_SENDER.get().ok_or_else(|| anyhow::anyhow!("IO system not initialized"))?;
    sender.send(task).await.map_err(|_| anyhow::anyhow!("IO worker channel closed"))
}

pub fn shutdown() {
    if let Some(sender) = IO_SENDER.get() {
        sender.close();
        // The worker will finish draining and exit its loop
    }
}

async fn io_worker(rx: Receiver<IoTask>) {
    let mut files: HashMap<PathBuf, File> = HashMap::new();

    while let Ok(task) = rx.recv().await {
        if let Err(e) = handle_task(&mut files, task) {
            crate::t_eprintln!("IO Worker error: {}", e);
        }
    }

    // Graceful shutdown: flush and close files
    for (_, mut file) in files {
        let _ = file.flush();
    }
}

fn handle_task(files: &mut HashMap<PathBuf, File>, task: IoTask) -> anyhow::Result<()> {
    // 1. Path validation (Security)
    validate_path(&task.path)?;

    // 2. Ensure parent directory exists
    if let Some(parent) = task.path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    match task.op {
        IoOp::Append => {
            // Check if we need to rotate before appending
            if task.path.exists() {
                let metadata = fs::metadata(&task.path)?;
                if metadata.len() + task.content.len() as u64 > MAX_FILE_SIZE {
                    // Close existing handle if open
                    files.remove(&task.path);
                    rotate_file(&task.path)?;
                }
            }

            let file = if let Some(f) = files.get_mut(&task.path) {
                f
            } else {
                let f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&task.path)?;
                files.insert(task.path.clone(), f);
                files.get_mut(&task.path).unwrap()
            };

            if task.add_timestamp {
                let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                writeln!(file, "[{}]: {}", timestamp, String::from_utf8_lossy(&task.content))?;
            } else {
                file.write_all(&task.content)?;
            }
            file.flush()?;
        }
        IoOp::Overwrite => {
            // For overwrite, we don't rotate like logs, we just replace.
            // But we still close the handle if we had one for appending.
            files.remove(&task.path);
            let mut file = File::create(&task.path)?;
            file.write_all(&task.content)?;
            file.flush()?;
        }
    }

    Ok(())
}

fn validate_path(path: &Path) -> anyhow::Result<()> {
    // 1. Extension Allowlist
    let allowed_extensions = ["csv", "json", "jsonl", "txt", "log"];
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();

    if !allowed_extensions.contains(&ext.as_str()) {
        return Err(anyhow::anyhow!(
            "File extension '.{}' is not allowed. Supported: {:?}",
            ext,
            allowed_extensions
        ));
    }

    // 2. Traversal Prevention
    if path.to_string_lossy().contains("..") {
        return Err(anyhow::anyhow!("Directory traversal attempt blocked."));
    }

    // 3. Directory Scoping
    // In production, we ensure we are in a 'safe' folder.
    #[cfg(not(test))]
    {
        let path_str = path.to_string_lossy();
        // Allow writes to 'jobs' or 'data' or 'logs'
        if !path_str.contains("jobs") && !path_str.contains("data") && !path_str.contains("logs") {
            return Err(anyhow::anyhow!("Path must be within project scope: {:?}", path));
        }
    }

    Ok(())
}

fn rotate_file(path: &Path) -> anyhow::Result<()> {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    let parent = path.parent().unwrap_or(Path::new("."));

    // 1. Rename current file to timestamped version
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let rotated_name = if extension.is_empty() {
        format!("{}.{}", stem, timestamp)
    } else {
        format!("{}.{}.{}", stem, timestamp, extension)
    };
    let rotated_path = parent.join(rotated_name);
    
    fs::rename(path, rotated_path)?;

    // 2. Cleanup: Keep only MAX_ROTATIONS
    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(parent) {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Match files that start with "stem." and end with ".extension" (if any)
            if name.starts_with(&format!("{}.", stem)) && 
               (extension.is_empty() || name.ends_with(&format!(".{}", extension))) &&
               name != path.file_name().unwrap_or_default().to_string_lossy() 
            {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        entries.push((name, entry.path()));
                    }
                }
            }
        }
    }

    // Sort by name (which starts with stem and then timestamp)
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    // If we have more than MAX_ROTATIONS, delete the oldest
    if entries.len() > MAX_ROTATIONS {
        let to_delete = entries.len() - MAX_ROTATIONS;
        for i in 0..to_delete {
            let _ = fs::remove_file(&entries[i].1);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_validate_path() {
        assert!(validate_path(Path::new("data.csv")).is_ok());
        assert!(validate_path(Path::new("logs/hook.log")).is_ok());
        assert!(validate_path(Path::new("../secret.txt")).is_err());
        assert!(validate_path(Path::new("data/../../etc/passwd")).is_err());
    }

    #[test]
    fn test_io_rotation() {
        smol::block_on(async {
            let dir = TempDir::new().unwrap();
            let file_path = dir.path().join("test.log");
            
            let (tx, _rx) = bounded(1024);
            let _ = IO_SENDER.set(tx); 

            let mut files = HashMap::new();

            // 1. Write until limit
            let large_content = vec![b'a'; (MAX_FILE_SIZE - 5) as usize];
            handle_task(&mut files, IoTask {
                path: file_path.clone(),
                content: large_content,
                op: IoOp::Append,
                add_timestamp: false,
            }).unwrap();

            assert!(file_path.exists());
            assert_eq!(fs::metadata(&file_path).unwrap().len(), MAX_FILE_SIZE - 5);

            // 2. Trigger rotation
            handle_task(&mut files, IoTask {
                path: file_path.clone(),
                content: vec![b'b'; 10],
                op: IoOp::Append,
                add_timestamp: false,
            }).unwrap();

            assert!(file_path.exists());
            assert_eq!(fs::metadata(&file_path).unwrap().len(), 10);

            // Find the rotated file (it will have a timestamp)
            let mut rotated_found = false;
            if let Ok(read_dir) = fs::read_dir(dir.path()) {
                for entry in read_dir.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("test.2") && name.ends_with(".log") {
                        rotated_found = true;
                        assert_eq!(entry.metadata().unwrap().len(), MAX_FILE_SIZE - 5);
                    }
                }
            }
            assert!(rotated_found, "Timestamped rotated file not found");
            }
);
    }

    #[test]
    fn test_fs_overwrite() {
        smol::block_on(async {
            let dir = TempDir::new().unwrap();
            let file_path = dir.path().join("state.json");
            let mut files = HashMap::new();

            handle_task(&mut files, IoTask {
                path: file_path.clone(),
                content: b"first".to_vec(),
                op: IoOp::Overwrite,
                add_timestamp: false,
            }).unwrap();

            assert_eq!(fs::read_to_string(&file_path).unwrap(), "first");

            handle_task(&mut files, IoTask {
                path: file_path.clone(),
                content: b"second".to_vec(),
                op: IoOp::Overwrite,
                add_timestamp: false,
            }).unwrap();

            assert_eq!(fs::read_to_string(&file_path).unwrap(), "second");
        });
    }
}
