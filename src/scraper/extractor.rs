use std::collections::HashMap;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use scraper::ElementRef;
use serde::Serialize;

use crate::config::types::{Field, JobConfig};

mod dom_parser;
mod raw_parser;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ExtractedItem {
    pub fields: HashMap<String, String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parser: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_parsers: Option<HashMap<String, String>>,

    pub parent_html: Option<String>,
    pub field_match_html: Option<HashMap<String, String>>,
    pub matches: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ExtractionResult {
    pub items: Vec<ExtractedItem>,
    pub selector_matches: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldSpec {
    name: String,
    selector: String,
    attr: String,
}

#[derive(Debug, Default)]
pub struct Extractor;

impl Extractor {
    pub fn new() -> Self {
        Self
    }

    pub fn extract(
        &self,
        config: &JobConfig,
        dir: Option<&Path>,
        html: &str,
    ) -> Result<ExtractionResult> {
        if config.debug {
            save_response_body(html, config, dir)?;
        }

        let field_specs = compile_fields(&config.fields)?;
        let result = extract_with_raw_fallback(config, html, &field_specs)?;

        if config.debug {
            save_extracted_items(&result.items, config, dir)?;
        }

        Ok(result)
    }
}

fn extract_with_raw_fallback(
    job: &JobConfig,
    html: &str,
    field_specs: &[FieldSpec],
) -> Result<ExtractionResult> {
    if let Some(res) = raw_parser::extract(job, html, field_specs)? {
        return Ok(res);
    }

    dom_parser::extract(job, html, field_specs)
}

fn compile_fields(fields: &[Field]) -> Result<Vec<FieldSpec>> {
    fields.iter().map(FieldSpec::try_from).collect()
}

fn extract_dom_value(element: &ElementRef<'_>, attr: &str, base_url: &str) -> String {
    if attr.eq_ignore_ascii_case("text") {
        let text = element.text().collect::<Vec<_>>().join(" ");
        return normalize_whitespace(&text);
    }

    element
        .value()
        .attr(attr)
        .map(normalize_whitespace)
        .map(|value| resolve_attr_value(base_url, attr, &value))
        .unwrap_or_default()
}

fn resolve_attr_value(base_url: &str, attr: &str, value: &str) -> String {
    if !attr.eq_ignore_ascii_case("href") {
        return value.to_owned();
    }

    resolve_url(base_url, value).unwrap_or_else(|| value.to_owned())
}

fn resolve_url(base_url: &str, value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("javascript:")
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with("tel:")
        || trimmed.starts_with("data:")
    {
        return Some(trimmed.to_owned());
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_owned());
    }

    if trimmed.starts_with("//") {
        let scheme = base_url.split_once("://")?.0;
        return Some(format!("{scheme}:{trimmed}"));
    }

    let (scheme, remainder) = base_url.split_once("://")?;
    let host_end = remainder.find('/').unwrap_or(remainder.len());
    let authority = &remainder[..host_end];
    let path = if host_end < remainder.len() {
        &remainder[host_end..]
    } else {
        "/"
    };

    let base_origin = format!("{scheme}://{authority}");

    if trimmed.starts_with('/') {
        return Some(format!("{base_origin}{trimmed}"));
    }

    let base_dir = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let joined = if base_dir.is_empty() {
        format!("/{trimmed}")
    } else {
        format!("{base_dir}/{trimmed}")
    };

    Some(format!("{base_origin}{}", normalize_relative_path(&joined)))
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_html_entities(value: &str) -> String {
    let mut decoded = value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
        .replace("&apos;", "'");

    while let Some(start) = decoded.find("&#") {
        let Some(end_rel) = decoded[start..].find(';') else {
            break;
        };
        let end = start + end_rel;
        let entity = &decoded[start + 2..end];
        let parsed = if let Some(hex) = entity
            .strip_prefix('x')
            .or_else(|| entity.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()
        } else {
            entity.parse::<u32>().ok()
        };

        let Some(codepoint) = parsed.and_then(char::from_u32) else {
            break;
        };

        decoded.replace_range(start..=end, &codepoint.to_string());
    }

    decoded
}

// fn matches_keywords(fields: &HashMap<String, String>, keywords: Option<&[String]>) -> bool {
//     let Some(keywords) = keywords else {
//         return true;
//     };

//     if keywords.is_empty() {
//         return true;
//     }

//     let haystack = fields
//         .values()
//         .map(|value| value.to_ascii_lowercase())
//         .collect::<Vec<_>>()
//         .join(" ");

//     keywords
//         .iter()
//         .map(|keyword| keyword.trim().to_ascii_lowercase())
//         .filter(|keyword| !keyword.is_empty())
//         .any(|keyword| haystack.contains(&keyword))
// }

pub fn matching_keywords(
    fields: &HashMap<String, String>,
    keywords: Option<&[String]>,
    search_fields: Option<&[String]>,
) -> Vec<String> {
    let Some(keywords) = keywords else {
        return vec![];
    };

    let haystack: String = if let Some(fields_to_search) = search_fields {
        fields_to_search
            .iter()
            .filter_map(|name| fields.get(name))
            .map(|value| value.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        fields
            .values()
            .map(|value| value.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    };

    keywords
        .iter()
        .map(|k| k.trim().to_ascii_lowercase())
        .filter(|k| !k.is_empty() && haystack.contains(k))
        .collect()
}

pub fn has_match(
    fields: &HashMap<String, String>,
    keywords: Option<&[String]>,
    search_fields: Option<&[String]>,
) -> bool {
    let Some(words) = keywords else {
        return true;
    };

    if words.is_empty() {
        return true;
    };

    let keywords_match = matching_keywords(fields, keywords, search_fields);

    !keywords_match.is_empty()
}

fn safe_element_html(element: ElementRef<'_>) -> String {
    catch_unwind(AssertUnwindSafe(|| element.html())).unwrap_or_else(|_| {
        format!(
            "[debug-html serialization panicked for <{}>]",
            element.value().name()
        )
    })
}

fn save_response_body(html: &str, config: &JobConfig, dir: Option<&Path>) -> Result<()> {
    let mut path = dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    path.push(format!("{}-response.html", config.id()));
    fs::write(&path, html).context(format!("failed to write {}", path.display()))
}

fn save_extracted_items(
    items: &[ExtractedItem],
    config: &JobConfig,
    dir: Option<&Path>,
) -> Result<()> {
    let mut path = dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    path.push(format!("{}-fields.json", config.id()));
    let body = serde_json::to_string_pretty(items)
        .context(format!("failed to serialize {}", path.display()))?;
    fs::write(&path, body).context(format!("failed to write {}", path.display()))
}

fn normalize_relative_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }

    format!("/{}", parts.join("/"))
}

impl TryFrom<&Field> for FieldSpec {
    type Error = anyhow::Error;

    fn try_from(value: &Field) -> Result<Self> {
        match value {
            Field::Full {
                name,
                selector,
                att,
            } => Ok(Self {
                name: name.clone(),
                selector: selector.clone(),
                attr: att.clone(),
            }),
            Field::Shorthand(raw) => parse_shorthand_field(raw),
        }
    }
}

fn parse_shorthand_field(raw: &str) -> Result<FieldSpec> {
    let (name, remainder) = raw
        .split_once(':')
        .context("shorthand field must use 'name:selector@attr' or 'name:selector' format")?;

    let (selector, attr) = match remainder.split_once('@') {
        Some((selector, attr)) => (selector.trim(), attr.trim()),
        None => (remainder.trim(), "text"),
    };

    let attr = if attr.is_empty() { "text" } else { attr };

    Ok(FieldSpec {
        name: name.trim().to_owned(),
        selector: selector.to_owned(),
        attr: attr.to_owned(),
    })
}

#[cfg(test)]
mod tests;
