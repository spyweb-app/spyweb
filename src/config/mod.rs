pub mod defaults;
pub mod loader;
pub mod types;
pub mod validate;

#[cfg(test)]
pub mod tests;

// pub const BASE_URL: &str = "127.0.0.1:8001";

pub fn get_port() -> u16 {
    get_port_with_override(None)
}

pub fn get_port_with_override(override_port: Option<u16>) -> u16 {
    override_port
        .or_else(|| {
            std::env::var("SPYWEB_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
        })
        .unwrap_or(7979)
}

pub fn get_base_url_with_override(override_port: Option<u16>) -> String {
    format!("127.0.0.1:{}", get_port_with_override(override_port))
}

pub fn get_base_url() -> String {
    get_base_url_with_override(None)
}
