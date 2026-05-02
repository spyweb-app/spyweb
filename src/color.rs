use std::io::IsTerminal;
use std::sync::OnceLock;

static STDOUT_TTY: OnceLock<bool> = OnceLock::new();
static STDERR_TTY: OnceLock<bool> = OnceLock::new();

pub fn stdout_is_tty() -> bool {
    *STDOUT_TTY.get_or_init(|| std::io::stdout().is_terminal())
}

pub fn stderr_is_tty() -> bool {
    *STDERR_TTY.get_or_init(|| std::io::stderr().is_terminal())
}

// Color Palette (TrueColor)
// Job:  #a29bfe -> 162, 155, 254
// OK:   #55efc4 -> 85, 239, 196
// Warn: #fdcb6e -> 253, 203, 110
// Info: #74b9ff -> 116, 185, 255
// Dim:  #6b7280 -> 107, 114, 128

/// Purple bold — job names
pub fn c_job(text: &str) -> String {
    if stdout_is_tty() {
        format!("\x1b[38;2;162;155;254;1m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Green bold — success, item counts
pub fn c_ok(text: &str) -> String {
    if stdout_is_tty() {
        format!("\x1b[38;2;85;239;196;1m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Blue — URLs, durations, numeric info
pub fn c_info(text: &str) -> String {
    if stdout_is_tty() {
        format!("\x1b[38;2;116;185;255m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Red bold — errors, failures
pub fn c_err(text: &str) -> String {
    if stderr_is_tty() {
        format!("\x1b[1;31m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Yellow bold — warnings, non-fatal issues
pub fn c_warn(text: &str) -> String {
    if stderr_is_tty() {
        format!("\x1b[38;2;253;203;110;1m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Bold only — section headers, key labels
pub fn c_bold(text: &str) -> String {
    if stdout_is_tty() {
        format!("\x1b[1m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Gray dim — timestamps, secondary info
pub fn c_dim(text: &str) -> String {
    if stdout_is_tty() {
        format!("\x1b[38;2;107;114;128m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Gray dim for stderr — used by t_eprintln timestamp
pub fn c_dim_err(text: &str) -> String {
    if stderr_is_tty() {
        format!("\x1b[38;2;107;114;128m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}
