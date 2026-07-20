use std::sync::atomic::AtomicU8;

pub const LOG_LEVEL_INFO: u8 = 0;
pub const LOG_LEVEL_WARN: u8 = 1;
pub const LOG_LEVEL_ERROR: u8 = 2;

pub static LOG_LEVEL: AtomicU8 = AtomicU8::new(LOG_LEVEL_INFO);

pub fn set_log_level(level: u8) {
    LOG_LEVEL.store(level, std::sync::atomic::Ordering::Relaxed);
}

pub fn init_log_level_from_env() {
    if let Ok(val) = std::env::var("SPYWEB_LOG") {
        match val.to_lowercase().as_str() {
            "error" | "err" => set_log_level(LOG_LEVEL_ERROR),
            "warn" | "warning" => set_log_level(LOG_LEVEL_WARN),
            _ => {}
        }
    }
}

#[macro_export]
macro_rules! t_println {
    ($($arg:tt)*) => {
        if $crate::macros::LOG_LEVEL.load(std::sync::atomic::Ordering::Relaxed) <= $crate::macros::LOG_LEVEL_INFO {
            println!("{}: {}", $crate::color::c_dim(&format!("[{}]", chrono::Local::now().format("%Y-%m-%d %I:%M %p"))), format_args!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! t_eprintln {
    ($($arg:tt)*) => {
        if $crate::macros::LOG_LEVEL.load(std::sync::atomic::Ordering::Relaxed) <= $crate::macros::LOG_LEVEL_ERROR {
            eprintln!("{}: {}", $crate::color::c_dim_err(&format!("[{}]", chrono::Local::now().format("%Y-%m-%d %I:%M %p"))), $crate::color::c_err(&format!($($arg)*)))
        }
    };
}

/// Warning-level output: timestamped, yellow text on stderr
#[macro_export]
macro_rules! t_warnln {
    ($($arg:tt)*) => {
        if $crate::macros::LOG_LEVEL.load(std::sync::atomic::Ordering::Relaxed) <= $crate::macros::LOG_LEVEL_WARN {
            eprintln!("{}: {}", $crate::color::c_dim_err(&format!("[{}]", chrono::Local::now().format("%Y-%m-%d %I:%M %p"))), $crate::color::c_warn(&format!($($arg)*)))
        }
    };
}
