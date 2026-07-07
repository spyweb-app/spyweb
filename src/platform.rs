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
}
