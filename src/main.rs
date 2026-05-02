fn main() {
    if let Err(e) = spyweb::cli::listen_command() {
        spyweb::t_eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
