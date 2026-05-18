// use crate::config::types::JobConfig;
use crate::config::types::{Notification, Rotate};
pub fn enabled() -> bool {
    true
}
pub fn interval() -> u32 {
    600
}
pub fn rotate() -> Rotate {
    Rotate::RoundRobin
}
pub fn notification() -> Option<Notification> {
    Some(Notification {
        enabled: true,
        timeout: 0,
        title: None,
        body: None,
    })
}
pub fn text() -> String {
    String::from("text")
}