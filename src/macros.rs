#[macro_export]
macro_rules! t_println {
    ($($arg:tt)*) => {
        println!("{}: {}", $crate::color::c_dim(&format!("[{}]", chrono::Local::now().format("%Y-%m-%d %I:%M %p"))), format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! t_eprintln {
    ($($arg:tt)*) => {
        eprintln!("{}: {}", $crate::color::c_dim_err(&format!("[{}]", chrono::Local::now().format("%Y-%m-%d %I:%M %p"))), $crate::color::c_err(&format!($($arg)*)))
    };
}

/// Warning-level output: timestamped, yellow text on stderr
#[macro_export]
macro_rules! t_warnln {
    ($($arg:tt)*) => {
        eprintln!("{}: {}", $crate::color::c_dim_err(&format!("[{}]", chrono::Local::now().format("%Y-%m-%d %I:%M %p"))), $crate::color::c_warn(&format!($($arg)*)))
    };
}
