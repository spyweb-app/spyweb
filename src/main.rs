fn main() {
    if let Err(e) = spyweb::cli::listen_command() {
        eprintln!("\x1b[1;31mError:\x1b[0m {}", e);
        std::process::exit(1);
    }
}
