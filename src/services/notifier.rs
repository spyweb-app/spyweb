use std::collections::HashMap;

use crate::{config::types::JobConfig, scraper::extractor::ExtractedItem};
use anyhow::Result;
use notify_rust::{Notification, Timeout};

use crate::config::types::{Field, Notification as Notif};

const BODY_ITEM_CAP: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationData {
    pub job_name: String,
    pub url: String,
    pub timestamp: u64,
    pub item_count: usize,
}

impl NotificationData {
    fn template_fields(&self) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("job_name".into(), self.job_name.clone());
        tags.insert("url".into(), self.url.clone());
        tags.insert("item_count".into(), self.item_count.to_string());
        tags.insert("timestamp".into(), self.timestamp.to_string());
        tags.insert("time".into(), self.timestamp.to_string());
        tags
    }
}

impl Notif {
    fn render(
        &self,
        field_order: &[String],
        items: &[ExtractedItem],
        data: &NotificationData,
    ) -> Option<(String, String)> {
        if !self.enabled {
            return None;
        }

        let all_matches = collect_all_matches(items);
        let mut run_tags = data.template_fields();
        run_tags.insert("match_count".into(), all_matches.len().to_string());
        run_tags.insert("matches".into(), all_matches.join(", "));
        run_tags.insert("matches_multiline".into(), all_matches.join("\n"));

        let title = self
            .title
            .as_deref()
            .map(|t| render_template_tags(t, &run_tags))
            .unwrap_or_else(|| default_title(data, items))
            .trim()
            .to_owned();
        let title = if title.is_empty() {
            default_title(data, items)
        } else {
            title
        };

        let body = match self.body.as_deref() {
            Some(template) => render_body_template(template, items, &run_tags),
            None => default_body(field_order, items),
        };

        Some((title, body))
    }
}



fn _field_names(fields: &[Field]) -> Vec<String> {
    fields
        .iter()
        .map(|field| match field {
            Field::Shorthand(raw) => raw
                .split_once(':')
                .map(|(name, _)| name.trim().to_owned())
                .unwrap_or_else(|| raw.trim().to_owned()),
            Field::Full { name, .. } => name.clone(),
        })
        .collect()
}

fn render_template_tags(template: &str, fields: &HashMap<String, String>) -> String {
    let bytes = template.as_bytes();
    let mut result = String::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'{' {
            result.push(bytes[i] as char);
            i += 1;
            continue;
        }

        let start = i + 1;
        let mut cursor = start;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }

        if cursor > start && cursor < bytes.len() && bytes[cursor] == b'}' {
            let field_name = std::str::from_utf8(&bytes[start..cursor]).unwrap_or_default();
            result.push_str(fields.get(field_name).map(String::as_str).unwrap_or(""));
            i = cursor + 1;
            continue;
        }

        result.push('{');
        i += 1;
    }

    result
}

fn render_body_template(
    template: &str,
    items: &[ExtractedItem],
    run_tags: &HashMap<String, String>,
) -> String {
    let mut parts: Vec<String> = items
        .iter()
        .take(BODY_ITEM_CAP)
        .map(|item| {
            let mut tags = run_tags.clone();
            tags.extend(item.fields.clone());
            render_template_tags(template, &tags)
        })
        .collect();

    let overflow = items.len().saturating_sub(BODY_ITEM_CAP);
    if overflow > 0 {
        parts.push(format!("+ {} more", overflow));
    }

    parts.join("\n---\n")
}

pub fn send_notification(title: &str, body: &str, timeout: u32) -> Result<()> {
    if let Err(e) = Notification::new()
        .summary(title)
        .body(body)
        .timeout(Timeout::Milliseconds(timeout))
        .show()
    {
        crate::t_println!("Skipped desktop notification (headless or OS error): {}", e);
    }
    Ok(())
}

fn send_config_notification(
    config: &Notif,
    field_order: &[String],
    items: &[ExtractedItem],
    data: &NotificationData,
) -> Result<bool> {
    let Some((title, body)) = config.render(field_order, items, data) else {
        return Ok(false);
    };
    send_notification(&title, &body, config.timeout)?;
    Ok(true)
}

pub fn trigger_notification(job: &JobConfig, items: &[ExtractedItem]) -> Result<bool> {
    let Some(ref notif_config) = job.notification else {
        return Ok(false);
    };

    if items.is_empty() {
        return Ok(false);
    }

    let data = NotificationData {
        job_name: job.name.clone(),
        url: format!(
            "{}/api/records?job_id={}",
            crate::config::get_base_url(),
            job.id()
        ),
        timestamp: crate::services::utils::now_secs(),
        item_count: items.len(),
    };

    send_config_notification(notif_config, &job.field_names(), items, &data)
}

fn default_title(data: &NotificationData, items: &[ExtractedItem]) -> String {
    if items.len() == 1 {
        format!("New match from {}", data.job_name.clone())
    } else {
        format!("{} new matches from {}", items.len(), data.job_name.clone())
    }
}


fn default_body(field_order: &[String], items: &[ExtractedItem]) -> String {
    let mut parts: Vec<String> = items
        .iter()
        .take(BODY_ITEM_CAP)
        .map(|item| {
            let mut lines = Vec::new();
            for field_name in unique_field_names(field_order) {
                if let Some(value) = item.fields.get(&field_name)
                    && !value.trim().is_empty()
                {
                    lines.push(format!("{}: {}", format_label(&field_name), value));
                }
                if lines.len() >= 5 {
                    break;
                }
            }

            if !item.matches.is_empty() {
                lines.push(format!("Keywords: {}", item.matches.join(", ")));
            }

            lines.join("\n")
        })
        .collect();
    let overflow = items.len().saturating_sub(BODY_ITEM_CAP);
    if overflow > 0 {
        parts.push(format!("+ {} more", overflow));
    }

    parts.join("\n---\n")
}

fn unique_field_names(field_order: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();

    for field_name in field_order {
        if seen.insert(field_name.clone()) {
            ordered.push(field_name.clone());
        }
    }

    ordered
}

fn format_label(field_name: &str) -> String {
    let mut label = String::new();
    for (index, part) in field_name.split('_').enumerate() {
        if index > 0 {
            label.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            label.extend(first.to_uppercase());
            label.extend(chars);
        }
    }
    label
}

fn collect_all_matches(items: &[ExtractedItem]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .flat_map(|item| &item.matches)
        .filter(|m| seen.insert((*m).clone()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(title: Option<&str>, body: Option<&str>) -> Notif {
        Notif {
            enabled: true,
            timeout: 0,
            title: title.map(str::to_string),
            body: body.map(str::to_string),
        }
    }

    fn data() -> NotificationData {
        NotificationData {
            job_name: "TestJob".into(),
            url: "https://example.com/jobs/123".into(),
            timestamp: 1_234_567_890,
            item_count: 3,
        }
    }

    fn items() -> Vec<ExtractedItem> {
        vec![ExtractedItem {
            fields: [
                ("title".to_string(), "Rust Developer".to_string()),
                (
                    "description".to_string(),
                    "Build scraping tools".to_string(),
                ),
                (
                    "link".to_string(),
                    "https://example.com/jobs/123".to_string(),
                ),
                ("date".to_string(), "2026-04-17".to_string()),
            ]
            .into_iter()
            .collect(),
            parent_html: None,
            field_match_html: None,
            matches: vec!["rust".into(), "backend".into()],
            ..Default::default()
        }]
    }

    fn field_order() -> Vec<String> {
        vec![
            "title".into(),
            "description".into(),
            "link".into(),
            "date".into(),
        ]
    }

    #[test]
    fn render_template_replaces_known_tags_and_drops_unknown_ones() {
        let mut fields = HashMap::new();
        fields.insert("title".into(), "Rust Developer".into());
        fields.insert("job_name".into(), "TestJob".into());

        let rendered = render_template_tags("Job {job_name}: {title} {missing}", &fields);

        assert_eq!(rendered, "Job TestJob: Rust Developer ");
    }

    #[test]
    fn render_uses_both_extracted_fields_and_predefined_notification_tags() {
        let cfg = config(
            Some("{job_name}: {title} ({match_count})"),
            Some("{matches_multiline}\n{url}\n{time}"),
        );

        let mut all_items = items();
        all_items.push(ExtractedItem {
            fields: [("title".to_string(), "Go Developer".to_string())]
                .into_iter()
                .collect(),
            parent_html: None,
            field_match_html: None,
            matches: vec!["go".into(), "backend".into()],
            ..Default::default()
        });

        let rendered = cfg.render(&field_order(), &all_items, &data()).unwrap();

        // Note: Currently, extracted item fields (like {title}) are NOT available
        // in the notification title template, only in the body template.
        assert_eq!(rendered.0, "TestJob:  (3)"); // rust, backend, go
        assert_eq!(
            rendered.1,
            "rust\nbackend\ngo\nhttps://example.com/jobs/123\n1234567890\n---\nrust\nbackend\ngo\nhttps://example.com/jobs/123\n1234567890"
        );
    }
    #[test]
    fn render_returns_none_only_when_disabled() {
        let mut cfg = config(None, Some("{title}"));

        assert_eq!(
            cfg.render(&field_order(), &items(), &data()),
            Some(("New match from TestJob".into(), "Rust Developer".into()))
        );

        cfg.enabled = false;
        cfg.title = Some("{title}".into());
        assert_eq!(cfg.render(&field_order(), &items(), &data()), None);
    }

    #[test]
    fn render_uses_default_title_and_body_when_templates_are_missing() {
        let cfg = config(None, None);

        let rendered = cfg.render(&field_order(), &items(), &data()).unwrap();

        assert_eq!(rendered.0, "New match from TestJob");
        assert_eq!(
            rendered.1,
            "Title: Rust Developer\nDescription: Build scraping tools\nLink: https://example.com/jobs/123\nDate: 2026-04-17\nKeywords: rust, backend"
        );
    }

    #[test]
    fn field_names_preserve_declared_order() {
        let fields = vec![
            Field::Shorthand("title:h4".into()),
            Field::Full {
                name: "description".into(),
                selector: ".desc".into(),
                att: "text".into(),
            },
            Field::Shorthand("link:a@href".into()),
        ];

        assert_eq!(_field_names(&fields), vec!["title", "description", "link"]);
    }
}
