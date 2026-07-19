use anyhow::Result;
use indexmap::IndexMap;
use smol::channel::{Receiver, Sender, bounded};
use std::fs::{self, File, OpenOptions};
use std::io::Write;

pub(crate) const ALLOWED_WRITE_EXT: &[&str] = &["csv", "json", "jsonl", "txt", "log"];
pub(crate) const ALLOWED_READ_EXT: &[&str] = &["csv", "json", "jsonl", "txt", "log"];
pub(crate) const ALLOWED_BINARY_EXT: &[&str] = &[
    "csv", "json", "jsonl", "txt", "log", "png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "ico",
    "woff", "woff2", "ttf", "otf", "pdf", "zip",
];

pub(crate) fn check_extension(path: &Path, allowed: &[&str], context: &str) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if !allowed.contains(&ext.as_str()) {
        return Err(anyhow::anyhow!(
            "{}: file extension '.{}' not allowed. Supported: {:?}",
            context,
            ext,
            allowed
        ));
    }
    Ok(())
}
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
pub const MAX_ROTATIONS: usize = 5;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
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
    pub reply: Option<Sender<Result<()>>>,
}

impl Clone for IoTask {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            content: self.content.clone(),
            op: self.op.clone(),
            add_timestamp: self.add_timestamp,
            reply: None,
        }
    }
}

static IO_SERVICE: OnceLock<RwLock<IoService>> = OnceLock::new();

struct IoService {
    sender: Option<Sender<IoTask>>,
    worker: Option<JoinHandle<()>>,
}

impl IoService {
    fn new() -> Self {
        Self {
            sender: None,
            worker: None,
        }
    }

    fn valid_sender(&self) -> Option<Sender<IoTask>> {
        self.sender.as_ref().and_then(|s| {
            if !s.is_closed() {
                Some(s.clone())
            } else {
                None
            }
        })
    }

    fn start(&mut self) -> Sender<IoTask> {
        let (tx, rx) = bounded(128);
        let handle = std::thread::spawn(|| worker_main(rx));
        self.sender = Some(tx.clone());
        self.worker = Some(handle);
        tx
    }

    fn shutdown(&mut self) {
        if let Some(ref sender) = self.sender {
            sender.close();
        }
        self.sender = None;
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn service() -> &'static RwLock<IoService> {
    IO_SERVICE.get_or_init(|| RwLock::new(IoService::new()))
}

fn ensure_sender() -> Sender<IoTask> {
    if let Some(sender) = service()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .valid_sender()
    {
        return sender;
    }
    let mut svc = service().write().unwrap_or_else(|e| e.into_inner());
    svc.valid_sender().unwrap_or_else(|| svc.start())
}

pub async fn send_task(mut task: IoTask) -> Result<()> {
    let (tx, rx) = bounded(1);
    task.reply = Some(tx);

    let sender = ensure_sender();
    if let Err(e) = sender.send(task).await {
        let sender = {
            let mut svc = service().write().unwrap_or_else(|e| e.into_inner());
            svc.start()
        };
        sender
            .send(e.0)
            .await
            .map_err(|_| anyhow::anyhow!("IO worker channel closed"))?;
    }

    rx.recv()
        .await
        .map_err(|_| anyhow::anyhow!("IO worker dropped reply channel"))?
}

pub fn shutdown() {
    if let Some(svc) = IO_SERVICE.get() {
        svc.write().unwrap_or_else(|e| e.into_inner()).shutdown();
    }
}

fn get_or_create_file<'a>(
    files: &'a mut IndexMap<PathBuf, File>,
    path: &Path,
) -> Result<&'a mut File> {
    if let Some(idx) = files.get_index_of(path) {
        files.move_index(idx, files.len() - 1);
    } else {
        if files.len() >= 64 {
            files.shift_remove_index(0);
        }
        files.insert(
            path.to_path_buf(),
            OpenOptions::new().create(true).append(true).open(path)?,
        );
    }
    files
        .get_mut(path)
        .ok_or_else(|| anyhow::anyhow!("io mapping corrupted for {}", path.display()))
}

fn worker_main(rx: Receiver<IoTask>) {
    let mut files = IndexMap::new();
    loop {
        let task = smol::block_on(async {
            futures_lite::future::or(async { rx.recv().await.ok() }, async {
                smol::Timer::after(IDLE_TIMEOUT).await;
                None
            })
            .await
        });

        let Some(task) = task else {
            break;
        };

        let reply = task.reply.clone();
        let res = handle_task(&mut files, task);
        if let Some(tx) = reply {
            let _ = smol::block_on(tx.send(res));
        }

        while let Ok(task) = rx.try_recv() {
            let reply = task.reply.clone();
            let res = handle_task(&mut files, task);
            if let Some(tx) = reply {
                let _ = smol::block_on(tx.send(res));
            }
        }
    }
}

fn handle_task(files: &mut IndexMap<PathBuf, File>, task: IoTask) -> Result<()> {
    validate_path(&task.path)?;
    check_extension(&task.path, ALLOWED_WRITE_EXT, "write")?;

    match task.op {
        IoOp::Append => append_to_file(files, &task.path, &task.content, task.add_timestamp),
        IoOp::Overwrite => overwrite_file(files, &task.path, &task.content),
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn append_to_file(
    files: &mut IndexMap<PathBuf, File>,
    path: &Path,
    content: &[u8],
    add_timestamp: bool,
) -> Result<()> {
    ensure_parent_dir(path)?;

    if should_rotate(path, content.len() as u64)? {
        files.shift_remove(path);
        rotate_file(path)?;
    }

    let file = get_or_create_file(files, path)?;

    if add_timestamp {
        let timestamp = chrono::Local::now()
            .format("[%Y-%m-%d %H:%M:%S]: ")
            .to_string();
        file.write_all(timestamp.as_bytes())?;
        file.write_all(content)?;
        file.write_all(b"\n")?;
    } else {
        file.write_all(content)?;
    }

    Ok(())
}

fn overwrite_file(files: &mut IndexMap<PathBuf, File>, path: &Path, content: &[u8]) -> Result<()> {
    ensure_parent_dir(path)?;
    let mut file = File::create(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    files.shift_remove(path);
    Ok(())
}

fn should_rotate(path: &Path, incoming_size: u64) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let metadata = fs::metadata(path)?;
    Ok(metadata.len() + incoming_size > MAX_FILE_SIZE)
}

fn rotate_file(path: &Path) -> Result<()> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let new_name = if ext.is_empty() {
        format!("{}.{}", stem, timestamp)
    } else {
        format!("{}.{}.{}", stem, timestamp, ext)
    };

    fs::rename(path, parent.join(new_name))?;
    prune_rotations(parent, stem, ext)?;
    Ok(())
}

fn prune_rotations(parent: &Path, stem: &str, extension: &str) -> Result<()> {
    let mut entries = Vec::new();
    let prefix = format!("{}.", stem);
    let suffix = if extension.is_empty() {
        String::new()
    } else {
        format!(".{}", extension)
    };

    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if !entry.metadata()?.is_file() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && name.starts_with(&prefix)
            && (suffix.is_empty() || name.ends_with(&suffix))
        {
            entries.push((name.to_string(), entry.path()));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    if entries.len() > MAX_ROTATIONS {
        for (_, path) in entries.iter().take(entries.len() - MAX_ROTATIONS) {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

pub(crate) fn validate_path(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(anyhow::anyhow!("Directory traversal attempt blocked."));
    }

    let current_dir = std::env::current_dir()?;
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    };

    let mut check_path = abs_path.as_path();
    while !check_path.exists() {
        match check_path.parent() {
            Some(parent) => check_path = parent,
            None => break,
        }
    }

    if check_path.exists() {
        let canonical = check_path.canonicalize()?;
        let canonical_cwd = current_dir.canonicalize()?;
        if !canonical.starts_with(&canonical_cwd) {
            return Err(anyhow::anyhow!("Path escapes sandbox via symlink."));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_path() {
        assert!(validate_path(Path::new("data.csv")).is_ok());
        assert!(validate_path(Path::new("logs/hook.log")).is_ok());
        assert!(validate_path(Path::new("jobs/myjob/output.json")).is_ok());
        assert!(validate_path(Path::new("random.ext")).is_ok());

        assert!(validate_path(Path::new("../secret.txt")).is_err());
        assert!(validate_path(Path::new("data/../../etc/passwd")).is_err());
        assert!(validate_path(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn test_io_rotation() {
        let current_dir = std::env::current_dir().unwrap();
        let test_dir = current_dir.join("target").join("test_grok_io_rotation");
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        let file_path = test_dir.join("test.log");
        let mut files = IndexMap::new();

        let large_content = vec![b'a'; (MAX_FILE_SIZE - 5) as usize];
        handle_task(
            &mut files,
            IoTask {
                path: file_path.clone(),
                content: large_content,
                op: IoOp::Append,
                add_timestamp: false,
                reply: None,
            },
        )
        .unwrap();

        assert!(file_path.exists());
        assert_eq!(fs::metadata(&file_path).unwrap().len(), MAX_FILE_SIZE - 5);

        handle_task(
            &mut files,
            IoTask {
                path: file_path.clone(),
                content: vec![b'b'; 10],
                op: IoOp::Append,
                add_timestamp: false,
                reply: None,
            },
        )
        .unwrap();

        assert!(file_path.exists());
        assert_eq!(fs::metadata(&file_path).unwrap().len(), 10);

        let mut rotated_found = false;
        if let Ok(read_dir) = fs::read_dir(&test_dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if path != file_path && name.starts_with("test.") && name.ends_with(".log") {
                    rotated_found = true;
                    assert_eq!(entry.metadata().unwrap().len(), MAX_FILE_SIZE - 5);
                }
            }
        }
        assert!(rotated_found, "Timestamped rotated file not found");
        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_fs_overwrite() {
        let current_dir = std::env::current_dir().unwrap();
        let test_dir = current_dir.join("target").join("test_grok_io_overwrite");
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        let file_path = test_dir.join("state.json");
        let mut files = IndexMap::new();

        handle_task(
            &mut files,
            IoTask {
                path: file_path.clone(),
                content: b"first".to_vec(),
                op: IoOp::Overwrite,
                add_timestamp: false,
                reply: None,
            },
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "first");

        handle_task(
            &mut files,
            IoTask {
                path: file_path.clone(),
                content: b"second".to_vec(),
                op: IoOp::Overwrite,
                add_timestamp: false,
                reply: None,
            },
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "second");
        let _ = fs::remove_dir_all(&test_dir);
    }
}
