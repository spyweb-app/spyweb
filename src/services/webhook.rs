use crate::scraper::extractor::ExtractedItem;
use anyhow::Result;
use std::collections::HashMap;

pub async fn trigger_webhook(
    job: &crate::config::types::JobConfig,
    payload: serde_json::Value,
) -> Result<bool> {
    let Some(ref webhook_config) = job.webhook else {
        return Ok(false);
    };

    if !webhook_config.enabled {
        return Ok(false);
    }

    let url = webhook_config.url.clone();
    let headers = webhook_config.headers.clone();
    let job_name = job.name.clone();

    let _ = smol::unblock(move || {
        if let Err(e) = send_webhook(&url, headers.as_ref(), payload) {
            crate::t_eprintln!("Webhook error for {}: {}", job_name, e);
        }
    })
    .await;

    Ok(true)
}

/// Build the default webhook payload from job name + items.
/// This is the payload that gets sent if no before_webhook hook exists.
pub fn build_default_payload(job_name: &str, items: &[ExtractedItem]) -> serde_json::Value {
    let items_json: Vec<serde_json::Value> = items
        .iter()
        .take(50)
        .map(|item| {
            let mut obj = serde_json::Map::new();
            for (key, value) in &item.fields {
                obj.insert(key.clone(), serde_json::Value::String(value.clone()));
            }
            if !item.matches.is_empty() {
                obj.insert(
                    "_keywords".to_string(),
                    serde_json::Value::String(item.matches.join(", ")),
                );
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    serde_json::json!({
        "job_name": job_name,
        "item_count": items.len(),
        "items": items_json,
    })
}

fn send_webhook(
    url: &str,
    headers: Option<&HashMap<String, String>>,
    payload: serde_json::Value,
) -> Result<()> {
    let mut request = ureq::post(url);

    if let Some(header_map) = headers {
        for (key, value) in header_map {
            request = request.header(key, value);
        }
    }

    request
        .send_json(&payload)
        .map_err(|e| anyhow::anyhow!("webhook failed: {}", e))?;
    Ok(())
}
