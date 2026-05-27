use std::time::Duration;

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

pub fn nanos_to_zulu(time: Option<u64>) -> String {
    use chrono::{DateTime, Utc};

    let dt = match time {
        Some(nanos) => DateTime::from_timestamp_nanos(nanos as i64),
        None => Utc::now(),
    };

    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn now_zulu() -> String {
    nanos_to_zulu(None)
}

pub fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{:.2}ms", duration.as_secs_f64() * 1000.0)
    }
}

pub fn shutdown_system() {
    crate::cdp::browser::shutdown_all();
    crate::services::io::shutdown();
    std::thread::sleep(std::time::Duration::from_millis(500));
}
