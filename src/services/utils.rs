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
