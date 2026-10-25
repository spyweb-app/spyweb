pub struct PlatformInfo;

impl PlatformInfo {
    pub fn os() -> &'static str {
        if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "unknown"
        }
    }

    pub fn arch() -> &'static str {
        if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else {
            "unknown"
        }
    }

    pub fn is_headless() -> bool {
        if cfg!(target_os = "macos") {
            return false;
        }
        if cfg!(target_os = "windows") {
            if let Ok(session) = std::env::var("SESSIONNAME") {
                return session == "Services";
            }
            return false;
        }

        std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err()
    }

    pub fn version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub fn lua_version() -> &'static str {
        if cfg!(feature = "luau") {
            "luau"
        } else {
            "lua54"
        }
    }

    pub fn storage() -> &'static str {
        if cfg!(feature = "sqlite") {
            "sqlite"
        } else {
            "redb"
        }
    }

    pub fn platform() -> String {
        let env = if cfg!(target_env = "musl") {
            "musl"
        } else if cfg!(target_env = "gnu") {
            "gnu"
        } else {
            ""
        };
        platform_for(Self::os(), Self::arch(), env)
    }
}

fn platform_for(os: &str, arch: &str, env: &str) -> String {
    if os == "unknown" || arch == "unknown" {
        return "unknown".to_string();
    }

    match os {
        "linux" => {
            let libc = match env {
                "gnu" => "glibc",
                "musl" => "musl",
                other => other,
            };
            if libc.is_empty() {
                "unknown".to_string()
            } else {
                format!("linux-{}-{}", libc, arch)
            }
        }
        "macos" | "windows" => format!("{}-{}", os, arch),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::platform_for;

    #[test]
    fn linux_gnu() {
        assert_eq!(platform_for("linux", "x86_64", "gnu"), "linux-glibc-x86_64");
        assert_eq!(
            platform_for("linux", "aarch64", "gnu"),
            "linux-glibc-aarch64"
        );
    }

    #[test]
    fn linux_musl() {
        assert_eq!(platform_for("linux", "x86_64", "musl"), "linux-musl-x86_64");
        assert_eq!(
            platform_for("linux", "aarch64", "musl"),
            "linux-musl-aarch64"
        );
    }

    #[test]
    fn apple_and_windows() {
        assert_eq!(platform_for("macos", "aarch64", ""), "macos-aarch64");
        assert_eq!(platform_for("macos", "x86_64", ""), "macos-x86_64");
        assert_eq!(platform_for("windows", "x86_64", "msvc"), "windows-x86_64");
    }

    #[test]
    fn unknown_components() {
        assert_eq!(platform_for("unknown", "x86_64", "gnu"), "unknown");
        assert_eq!(platform_for("linux", "unknown", "gnu"), "unknown");
        assert_eq!(platform_for("freebsd", "x86_64", ""), "unknown");
        assert_eq!(platform_for("linux", "x86_64", ""), "unknown");
    }

    #[test]
    fn other_libc_is_reported() {
        assert_eq!(
            platform_for("linux", "x86_64", "uclibc"),
            "linux-uclibc-x86_64"
        );
    }
}
