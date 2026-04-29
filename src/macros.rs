#[macro_export]
macro_rules! t_println {
    ($($arg:tt)*) => {
        println!("\x1b[38;2;107;114;128m[{}]\x1b[0m: {}", chrono::Local::now().format("%Y-%m-%d %I:%M %p"), format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! t_eprintln {
    ($($arg:tt)*) => {
        eprintln!("\x1b[38;2;107;114;128m[{}]\x1b[0m: \x1b[1;31m{}\x1b[0m", chrono::Local::now().format("%Y-%m-%d %I:%M %p"), format_args!($($arg)*))
    };
}

// Color Helpers (Raw ANSI TrueColor)
// Job: #a29bfe -> 162, 155, 254
// OK:  #55efc4 -> 85, 239, 196
// Web: #fab1a0 -> 250, 177, 160
// Info:#74b9ff -> 116, 185, 255
