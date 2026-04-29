// use crate::config::types::JobConfig;
use crate::config::types::{Notification, Rotate};
pub fn enabled() -> bool {
    true
}
pub fn interval() -> u32 {
    30
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
// impl Default for JobConfig {
// fn default() -> Self {
//     Self {
//         enabled: true,
//         interval: 60,
//         push_notification: true,
//     }
//     // pub struct JobConfig {
//     // pub name: String,
//     // pub url: String,
//     // pub selector: String,
//     // pub fields: Vec<Field>,
//     // pub keywords: Vec<String>,
//     // pub webhook: Option<String>,
//     // pub enabled: bool,
//     // pub interval: u32,
//     // pub proxy: Option<Proxy>,
//     // pub push_notification: bool,
//     // pub headers: Option<HashMap<String, String>>,
// }
// pub fn apply_defaults(&mut self) {
//     if self.enabled.is_none() {
//         self.enabled = Some(true);
//     }

//     if self.interval.is_none() {
//         self.interval = Some(30); // default 30 seconds
//     }

//     if self.headers.is_none() {
//         self.headers = Some(HashMap::new());
//     }

//     if self.fields.is_none() {
//
//         self.fields = Some(vec![
//             Field {
//                 name: "title".to_string(),
//                 selector: "h1,h2,h3".to_string(),
//                 attr: "text".to_string(),
//             },
//             Field {
//                 name: "link".to_string(),
//                 selector: "a".to_string(),
//                 attr: "href".to_string(),
//             },
//         ]);
//     }

//     if self.keywords.is_none() {
//         self.keywords = Some(vec![]); // empty = no filtering, show all
//     }
// }
// }
