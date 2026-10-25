use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::platform::PlatformInfo;

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
    let sql = PlatformInfo::storage() == "sqlite";
    let url = update_url(base, latest, &PlatformInfo::platform(), sql)?;

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

    let current_tray = install_dir.join(binary_name("spyweb-tray"));
    match tray_action(new_tray.as_deref(), &current_tray) {
        TrayAction::Replace => {
            if let Some(new_tray) = new_tray.as_deref() {
                if let Err(e) =
                    swap_binary(&current_tray, new_tray, retain, suffix.as_deref(), local)
                {
                    crate::t_warnln!("Warning: failed to update spyweb-tray: {}", e);
                }
            }
        }
        TrayAction::SkipMissing => {}
        TrayAction::LeaveStale => {
            crate::t_warnln!(
                "Warning: spyweb-tray was not included in this package; existing tray was left unchanged."
            );
        }
        TrayAction::Nothing => {}
    }

    println!("{}", crate::color::c_ok(&format!("Updated to v{}", latest)));
    println!(
        "  {}",
        crate::color::c_dim(
            "Restart any running spyweb or spyweb-tray process to use the new version."
        )
    );

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
    let dir = current
        .parent()
        .context("Cannot determine binary directory")?;
    let file_name = current
        .file_name()
        .context("Invalid binary path")?
        .to_string_lossy()
        .to_string();

    let staged = tempfile::Builder::new()
        .prefix(&format!(".{}.", file_name))
        .suffix(".new")
        .tempfile_in(dir)
        .context("Failed to create staging file")?;
    fs::copy(new, staged.path()).context("Failed to stage new binary")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o755))?;
    }

    let aside = if retain {
        backup_path(current, suffix, version)
    } else {
        current.with_file_name(format!("{}.old", file_name))
    };

    if let Err(e) = fs::remove_file(&aside) {
        if e.kind() != std::io::ErrorKind::NotFound {
            bail!(
                "Cannot replace {}: {} is still in use ({}). Stop the other spyweb process running the old version and retry.",
                file_name,
                aside.display(),
                e
            );
        }
    }

    if retain {
        println!(
            "  {} {} → {}",
            crate::color::c_dim("Backing up"),
            file_name,
            aside.file_name().unwrap_or_default().to_string_lossy(),
        );
    } else {
        println!("  {} {}", crate::color::c_dim("Replacing"), file_name);
    }

    fs::rename(current, &aside).context("Failed to back up current binary")?;

    if let Err(e) = staged.persist(current) {
        let install_err = e.error;
        drop(e.file);
        return match fs::rename(&aside, current) {
            Ok(()) => Err(install_err).context("Failed to install new binary, restored original"),
            Err(restore_err) => bail!(
                "Failed to install new binary: {}; failed to restore original: {}; backup remains at {}",
                install_err,
                restore_err,
                aside.display()
            ),
        };
    }

    if !retain {
        if let Err(e) = fs::remove_file(&aside) {
            if e.kind() != std::io::ErrorKind::NotFound {
                crate::t_warnln!(
                    "Old binary is still in use; cleanup will be retried on the next start."
                );
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
pub fn clean_old_exe() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    clean_old_exe_in(dir);
}

#[cfg(any(windows, test))]
fn clean_old_exe_in(dir: &Path) {
    for name in ["spyweb", "spyweb-tray"] {
        let old = dir.join(format!("{}.old", binary_name(name)));
        match fs::remove_file(&old) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                crate::t_warnln!(
                    "Warning: {} is still in use; another spyweb process may still be running the old version.",
                    old.display()
                );
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum TrayAction {
    Replace,
    SkipMissing,
    LeaveStale,
    Nothing,
}

fn tray_action(new_tray: Option<&Path>, current_tray: &Path) -> TrayAction {
    match (new_tray, current_tray.exists()) {
        (Some(_), true) => TrayAction::Replace,
        (Some(_), false) => TrayAction::SkipMissing,
        (None, true) => TrayAction::LeaveStale,
        (None, false) => TrayAction::Nothing,
    }
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

fn update_url(base: &str, latest: &str, platform: &str, sql: bool) -> Result<String> {
    let suffix = if sql { "-sql" } else { "" };
    let version = latest.trim_start_matches('v');
    let deploy = "https://github.com/spyweb-app/deploy/releases/download";

    let url = match platform {
        "linux-glibc-x86_64" => format!("{}/linux{}", base, suffix),
        "linux-musl-x86_64" => format!(
            "{}/spyweb-{}/spyweb-linux-musl-x86_64{}.tar.gz",
            deploy, version, suffix
        ),
        "linux-musl-aarch64" | "linux-glibc-aarch64" => format!(
            "{}/spyweb-{}/spyweb-linux-musl-aarch64{}.tar.gz",
            deploy, version, suffix
        ),
        "macos-aarch64" => format!("{}/mac-arm{}", base, suffix),
        "macos-x86_64" => format!("{}/mac-intel{}", base, suffix),
        "windows-x86_64" => format!("{}/windows{}", base, suffix),
        other => bail!("unsupported platform: {}", other),
    };

    Ok(url)
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{
        BETA_BASE, STABLE_BASE, TrayAction, binary_name, clean_old_exe_in, swap_binary,
        tray_action, update_url,
    };

    fn write_file(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    #[test]
    fn dl_platforms() {
        assert_eq!(
            update_url(STABLE_BASE, "1.5.6", "linux-glibc-x86_64", false).unwrap(),
            "https://dl.spyweb.app/linux"
        );
        assert_eq!(
            update_url(STABLE_BASE, "1.5.6", "linux-glibc-x86_64", true).unwrap(),
            "https://dl.spyweb.app/linux-sql"
        );
        assert_eq!(
            update_url(BETA_BASE, "1.6.0-beta", "linux-glibc-x86_64", false).unwrap(),
            "https://beta.spyweb.app/linux"
        );
        assert_eq!(
            update_url(STABLE_BASE, "1.5.6", "macos-aarch64", true).unwrap(),
            "https://dl.spyweb.app/mac-arm-sql"
        );
        assert_eq!(
            update_url(STABLE_BASE, "1.5.6", "macos-x86_64", false).unwrap(),
            "https://dl.spyweb.app/mac-intel"
        );
        assert_eq!(
            update_url(STABLE_BASE, "1.5.6", "windows-x86_64", true).unwrap(),
            "https://dl.spyweb.app/windows-sql"
        );
    }

    #[test]
    fn musl_platforms() {
        assert_eq!(
            update_url(STABLE_BASE, "1.5.6", "linux-musl-x86_64", false).unwrap(),
            "https://github.com/spyweb-app/deploy/releases/download/spyweb-1.5.6/spyweb-linux-musl-x86_64.tar.gz"
        );
        assert_eq!(
            update_url(BETA_BASE, "v1.6.0-beta", "linux-musl-aarch64", true).unwrap(),
            "https://github.com/spyweb-app/deploy/releases/download/spyweb-1.6.0-beta/spyweb-linux-musl-aarch64-sql.tar.gz"
        );
        assert_eq!(
            update_url(STABLE_BASE, "1.5.6", "linux-glibc-aarch64", true).unwrap(),
            "https://github.com/spyweb-app/deploy/releases/download/spyweb-1.5.6/spyweb-linux-musl-aarch64-sql.tar.gz"
        );
    }

    #[test]
    fn unsupported_platform_errors() {
        assert!(update_url(STABLE_BASE, "1.5.6", "unknown", false).is_err());
        assert!(update_url(STABLE_BASE, "1.5.6", "freebsd-x86_64", false).is_err());
    }

    #[test]
    fn swap_retains_backup() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("spyweb");
        let new = dir.path().join("new-spyweb");
        write_file(&current, "old");
        write_file(&new, "new");

        swap_binary(&current, &new, true, None, "1.5.6").unwrap();

        assert_eq!(fs::read_to_string(&current).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(dir.path().join("spyweb-v1.5.6")).unwrap(),
            "old"
        );
    }

    #[test]
    fn swap_overwrites_without_backup() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("spyweb");
        let new = dir.path().join("new-spyweb");
        write_file(&current, "old");
        write_file(&new, "new");

        swap_binary(&current, &new, false, None, "1.5.6").unwrap();

        assert_eq!(fs::read_to_string(&current).unwrap(), "new");
        assert!(!dir.path().join("spyweb.old").exists());
    }

    #[test]
    fn swap_clears_stale_tombstone() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("spyweb");
        let new = dir.path().join("new-spyweb");
        write_file(&current, "old");
        write_file(&new, "new");
        write_file(&dir.path().join("spyweb.old"), "stale");

        swap_binary(&current, &new, false, None, "1.5.6").unwrap();

        assert_eq!(fs::read_to_string(&current).unwrap(), "new");
        assert!(!dir.path().join("spyweb.old").exists());
    }

    #[test]
    fn swap_replaces_existing_backup() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("spyweb");
        let new = dir.path().join("new-spyweb");
        write_file(&current, "old");
        write_file(&new, "new");
        write_file(&dir.path().join("spyweb-v1.5.6"), "stale");

        swap_binary(&current, &new, true, None, "1.5.6").unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("spyweb-v1.5.6")).unwrap(),
            "old"
        );
    }

    #[test]
    fn swap_failure_leaves_current_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("spyweb");
        let missing = dir.path().join("missing-spyweb");
        write_file(&current, "old");

        assert!(swap_binary(&current, &missing, true, None, "1.5.6").is_err());
        assert_eq!(fs::read_to_string(&current).unwrap(), "old");
        assert!(!dir.path().join("spyweb-v1.5.6").exists());
    }

    #[test]
    fn cleanup_removes_unlocked_tombstones() {
        let dir = tempfile::tempdir().unwrap();
        let spyweb_old = dir.path().join(format!("{}.old", binary_name("spyweb")));
        let tray_old = dir
            .path()
            .join(format!("{}.old", binary_name("spyweb-tray")));
        write_file(&spyweb_old, "old");
        write_file(&tray_old, "old");

        clean_old_exe_in(dir.path());

        assert!(!spyweb_old.exists());
        assert!(!tray_old.exists());
    }

    #[test]
    fn tray_actions_match_installation_state() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("spyweb-tray");
        let new = dir.path().join("new-spyweb-tray");
        write_file(&new, "new");

        assert_eq!(tray_action(Some(&new), &current), TrayAction::SkipMissing);

        write_file(&current, "old");
        assert_eq!(tray_action(Some(&new), &current), TrayAction::Replace);
        assert_eq!(tray_action(None, &current), TrayAction::LeaveStale);

        fs::remove_file(&current).unwrap();
        assert_eq!(tray_action(None, &current), TrayAction::Nothing);
    }
}
