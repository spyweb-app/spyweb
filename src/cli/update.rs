use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const LATEST_ENDPOINT: &str = "https://spyweb.app/latest.json";
const BETA_BASE: &str = "https://beta.spyweb.app";
const STABLE_BASE: &str = "https://dl.spyweb.app";

pub fn run_update(
    check: bool,
    force: bool,
    keep: Option<Option<String>>,
    overwrite: bool,
) -> Result<()> {
    if overwrite && keep.is_some() {
        bail!("Cannot use both --overwrite and --keep");
    }

    let local = env!("CARGO_PKG_VERSION");
    let channel = if local.contains("-beta") {
        "beta"
    } else {
        "stable"
    };

    println!("Checking for updates...");

    let json =
        fetch_json(LATEST_ENDPOINT).context("Failed to fetch update info from spyweb.app")?;
    let channel_data = json
        .get(channel)
        .context(format!("No update data for '{}' channel", channel))?;
    let latest = channel_data
        .get("latest_version")
        .and_then(|v| v.as_str())
        .context("Invalid version in update data")?;

    if !force {
        if latest == local {
            println!(
                "{}",
                crate::color::c_ok(&format!("v{} - up to date", local))
            );
            return Ok(());
        }
        println!(
            "{}  v{} available (current: v{})",
            crate::color::c_warn("New version:"),
            crate::color::c_info(latest),
            local
        );
    }

    let base = if channel == "beta" {
        BETA_BASE
    } else {
        STABLE_BASE
    };
    let variant = platform_variant();
    let url = format!("{}/{}", base, variant);

    if check {
        println!("  Channel: {}", channel);
        println!("  Download: {}", url);
        return Ok(());
    }

    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let archive_path = temp_dir.path().join("spyweb.tar.gz");

    println!("  Downloading from {}...", url);
    download_file(&url, &archive_path).context("Failed to download update archive")?;

    println!("  Extracting...");
    extract_archive(&archive_path, temp_dir.path()).context("Failed to extract archive")?;

    let extracted = temp_dir.path().join("spyweb");
    let new_binary = find_binary(&extracted, "spyweb")?;
    let new_tray = find_binary_opt(&extracted, "spyweb-tray");

    println!("  Verifying binary...");
    self_test(&new_binary)?;

    let install_dir = std::env::current_exe()?
        .parent()
        .context("Cannot determine install directory")?
        .to_path_buf();

    let current_binary = install_dir.join(binary_name("spyweb"));

    let retain = !overwrite;
    let suffix = keep.and_then(|k| k);

    swap_binary(
        &current_binary,
        &new_binary,
        retain,
        suffix.as_deref(),
        local,
    )
    .context("Failed to replace spyweb binary")?;

    if let Some(new_tray) = new_tray.as_ref() {
        let current_tray = install_dir.join(binary_name("spyweb-tray"));
        if current_tray.exists() {
            swap_binary(&current_tray, new_tray, retain, suffix.as_deref(), local)
                .context("Failed to replace spyweb-tray binary")?;
        }
    }

    println!("{}", crate::color::c_ok(&format!("Updated to v{}", latest)));

    Ok(())
}

fn fetch_json(url: &str) -> Result<serde_json::Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();

    let mut response = agent
        .get(url)
        .header("User-Agent", "spyweb")
        .call()
        .context(format!("Failed to fetch {}", url))?;

    let mut buf = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut buf)
        .context("Failed to read response body")?;
    let body = String::from_utf8(buf).context("Invalid UTF-8 in response")?;
    Ok(serde_json::from_str(&body)?)
}

fn download_file(url: &str, dest: &Path) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();

    let mut response = agent
        .get(url)
        .header("User-Agent", "spyweb")
        .call()
        .context(format!("Failed to download {}", url))?;

    let mut file = fs::File::create(dest)?;
    let mut reader = response.body_mut().as_reader();
    std::io::copy(&mut reader, &mut file)?;

    Ok(())
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .context("Failed to run tar")?;

    if !status.success() {
        bail!("tar extraction failed (exit code: {:?})", status.code());
    }

    Ok(())
}

fn self_test(binary: &Path) -> Result<()> {
    let output = Command::new(binary)
        .arg("v")
        .output()
        .context("Failed to execute new binary for self-test")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("New binary failed self-test: {}", stderr.trim());
    }

    Ok(())
}

fn swap_binary(
    current: &Path,
    new: &Path,
    retain: bool,
    suffix: Option<&str>,
    version: &str,
) -> Result<()> {
    if retain {
        let backup = backup_path(current, suffix, version);
        println!(
            "  {} {} → {}",
            crate::color::c_dim("Backing up"),
            current.file_name().unwrap_or_default().to_string_lossy(),
            backup.file_name().unwrap_or_default().to_string_lossy(),
        );
        fs::rename(current, &backup).context("Failed to back up current binary")?;
        fs::copy(new, current).context("Failed to install new binary")?;
    } else {
        let temp_backup = current.with_extension("old");
        println!(
            "  {} {}",
            crate::color::c_dim("Replacing"),
            current.file_name().unwrap_or_default().to_string_lossy(),
        );
        fs::rename(current, &temp_backup)
            .context("Failed to back up current binary for replacement")?;
        if let Err(e) = fs::copy(new, current) {
            let _ = fs::rename(&temp_backup, current);
            return Err(e).context("Failed to copy new binary, restored original");
        }
        fs::remove_file(&temp_backup)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(current, fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

fn backup_path(current: &Path, suffix: Option<&str>, version: &str) -> PathBuf {
    let stem = current.file_stem().unwrap_or_default().to_string_lossy();
    let ext = current
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    match suffix {
        Some(s) => current.with_file_name(format!("{}-{}{}", stem, s, ext)),
        None => current.with_file_name(format!("{}-v{}{}", stem, version, ext)),
    }
}

fn platform_variant() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "mac-arm"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "mac-intel"
    } else {
        "unknown"
    };

    let sql = if cfg!(feature = "sqlite") { "-sql" } else { "" };
    format!("{}{}", os, sql)
}

fn binary_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{}.exe", name)
    } else {
        name.to_string()
    }
}

fn find_binary(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(binary_name(name));
    if path.exists() {
        return Ok(path);
    }
    bail!(
        "Binary '{}' not found in extracted archive at {}",
        binary_name(name),
        dir.display()
    );
}

fn find_binary_opt(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(binary_name(name));
    if path.exists() { Some(path) } else { None }
}
