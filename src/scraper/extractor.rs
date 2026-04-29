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
    ) -> Result<Vec<ExtractedItem>> {
        if config.debug {
            save_response_body(html, config, dir)?;
        }

        let field_specs = compile_fields(&config.fields)?;
        let items = extract_with_raw_fallback(config, html, &field_specs)?;

        if config.debug {
            save_extracted_items(&items, config, dir)?;
        }

        Ok(items)
    }
}

fn extract_with_raw_fallback(
    job: &JobConfig,
    html: &str,
    field_specs: &[FieldSpec],
) -> Result<Vec<ExtractedItem>> {
    if let Some(items) = raw_parser::extract(job, html, field_specs)? {
        return Ok(items);
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

fn matching_keywords(
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
mod tests {
    use super::*;
    use crate::config::types::Proxy;

    #[test]
    fn test_parse_shorthand_empty_selector() {
        let field = parse_shorthand_field("link:@href").unwrap();
        assert_eq!(field.name, "link");
        assert_eq!(field.selector, "");
        assert_eq!(field.attr, "href");

        let field_text = parse_shorthand_field("title:").unwrap();
        assert_eq!(field_text.name, "title");
        assert_eq!(field_text.selector, "");
        assert_eq!(field_text.attr, "text");
    }

    fn job(selector: &str, fields: Vec<Field>, keywords: Option<Vec<&str>>) -> JobConfig {
        JobConfig {
            name: "test".into(),
            url: "https://example.com".into(),
            debug: false,
            selector: selector.into(),
            fields,
            keywords: keywords
                .map(|keywords| keywords.into_iter().map(str::to_string).collect::<Vec<_>>()),
            search_fields: None,
            webhook: None,
            enabled: true,
            interval: 60,
            proxy: Some(Proxy {
                enabled: false,
                rotate: crate::config::types::Rotate::RoundRobin,
                urls: vec![],
            }),
            notification: None,
            headers: None,
            hash_fields: None,
        }
    }

    const HTML: &str = r#"
        <div class="job">
            <h2>Rust Developer</h2>
            <a href="/jobs/rust">Apply</a>
            <p class="description">Build scraping tools in Rust</p>
        </div>
        <div class="job">
            <h2>PHP Developer</h2>
            <a href="/jobs/php">Apply</a>
            <p class="description">Legacy app maintenance</p>
        </div>
    "#;

    const UGLY_HTML: &str = r#"
        <a href="/job/Project-Manager-1625213"></a>
        <div class="jobpost-cat-box">
          <a href="/job/Project-Manager-1625213"></a>
          <div class="d-flex align-items-start">
            <a href="/job/Project-Manager-1625213">
              <div class="d-block mr-3">
                <img class="jobpost-cat-box-logo" alt="Williams Drafting Co">
              </div>
            </a>
            <div class="flex-1">
              <a href="/job/Project-Manager-1625213">
                <dl class="row fs-14 d-flex justify-content-between">
                  <dt class="col">
                    <h4 class="fs-16 fw-700">Project Manager <span class="badge any mt-md-0">Any</span></h4>
                  </dt>
                </dl>
                <p class="fs-13 mb-0" data-temp="2026-04-16 14:47:49">
                  <em>Posted on 2026-04-16 14:47:49</em>
                </p>
                <dl class="row fs-14 no-gutters align-items-top mt-2 mb-2 mt-sm-0 mb-sm-0">
                  <dt class="col-auto text-center">
                    <i class="icon icon-round-dollar fs-22 mr-2 d-sm-none d-block left"></i>
                  </dt>
                  <dd class="col">43600</dd>
                </dl>
              </a>
              <div class="desc fs-14 d-none d-sm-block">
                <a href="/job/Project-Manager-1625213"> We are an Australian based Drafting company <br> PROJECT MANAGER <br> Skills <br> - Project… </a>
                <a href="/job/1625213" target="_blank">See More</a>
              </div>
              <div class="job-tag">
                <a href="/search/c/project-management" class="badge">Project Management</a>
              </div>
            </div>
          </div>
        </div>
    "#;

    #[test]
    fn extracts_fields_from_each_matching_container() {
        let extractor = Extractor::new();
        let job = job(
            ".job",
            vec![
                Field::Shorthand("title:h2".into()),
                Field::Full {
                    name: "link".into(),
                    selector: "a".into(),
                    att: "href".into(),
                },
                Field::Full {
                    name: "description".into(),
                    selector: ".description".into(),
                    att: "text".into(),
                },
            ],
            None,
        );

        let items = extractor.extract(&job, None, HTML).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].fields.get("title").map(String::as_str),
            Some("Rust Developer")
        );
        assert_eq!(
            items[0].fields.get("link").map(String::as_str),
            Some("https://example.com/jobs/rust")
        );
        assert_eq!(items[0].parent_html, None);
        assert_eq!(items[0].field_match_html, None);
    }

    #[test]
    fn keyword_filter_is_case_insensitive() -> Result<()> {
        let extractor = Extractor::new();
        let job = job(
            ".job",
            vec![
                Field::Shorthand("title:h2".into()),
                Field::Full {
                    name: "description".into(),
                    selector: ".description".into(),
                    att: "text".into(),
                },
            ],
            Some(vec!["RUST"]),
        );

        let items = extractor.extract(&job, None, HTML).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].fields.get("title").map(String::as_str),
            Some("Rust Developer")
        );
        Ok(())
    }

    #[test]
    fn shorthand_defaults_to_text_attribute() {
        let spec = parse_shorthand_field("title:h2").unwrap();

        assert_eq!(spec.name, "title");
        assert_eq!(spec.selector, "h2");
        assert_eq!(spec.attr, "text");
    }

    #[test]
    fn includes_empty_fields_for_missing_matches() {
        let extractor = Extractor::new();
        let job = job(
            ".job",
            vec![
                Field::Shorthand("title:h2".into()),
                Field::Full {
                    name: "company".into(),
                    selector: ".company".into(),
                    att: "text".into(),
                },
                Field::Full {
                    name: "link".into(),
                    selector: "a".into(),
                    att: "href".into(),
                },
            ],
            None,
        );

        let html = r#"
            <div class="job">
                <h2>Rust Developer</h2>
            </div>
        "#;

        let items = extractor.extract(&job, None, html).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].fields.get("title").map(String::as_str),
            Some("Rust Developer")
        );
        assert_eq!(items[0].fields.get("company").map(String::as_str), Some(""));
        assert_eq!(items[0].fields.get("link").map(String::as_str), Some(""));
        assert_eq!(items[0].field_match_html, None);
    }

    #[test]
    fn resolves_relative_href_to_absolute_url_in_dom_mode() {
        let extractor = Extractor::new();
        let job = job(".job", vec![Field::Shorthand("link:a@href".into())], None);

        let items = extractor.extract(&job, None, HTML).unwrap();

        assert_eq!(
            items[0].fields.get("link").map(String::as_str),
            Some("https://example.com/jobs/rust")
        );
    }

    #[test]
    fn preserves_descendants_in_badly_formatted_html() {
        let extractor = Extractor::new();
        let job = job(
            ".jobpost-cat-box",
            vec![
                Field::Shorthand("title:h4".into()),
                Field::Shorthand("desc:.desc".into()),
                Field::Shorthand("link:a@href".into()),
                Field::Shorthand("date:p.fs-13@data-temp".into()),
                Field::Shorthand("rate:dl>dd".into()),
            ],
            None,
        );

        let items = extractor.extract(&job, None, UGLY_HTML).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].fields.get("title").map(String::as_str),
            Some("Project Manager Any")
        );
        assert_eq!(
            items[0].fields.get("link").map(String::as_str),
            Some("https://example.com/job/Project-Manager-1625213")
        );
        assert_eq!(
            items[0].fields.get("date").map(String::as_str),
            Some("2026-04-16 14:47:49")
        );
        assert_eq!(
            items[0].fields.get("rate").map(String::as_str),
            Some("43600")
        );
        assert!(
            items[0]
                .fields
                .get("desc")
                .unwrap()
                .contains("We are an Australian based Drafting company")
        );
        assert!(items[0].fields.get("desc").unwrap().contains("See More"));
        assert_eq!(items[0].parent_html, None);
        assert_eq!(items[0].field_match_html, None);
    }

    #[test]
    fn includes_debug_html_only_when_debug_is_enabled() {
        let extractor = Extractor::new();
        let mut job = job(
            ".jobpost-cat-box",
            vec![
                Field::Shorthand("title:h4".into()),
                Field::Shorthand("desc:.desc".into()),
                Field::Shorthand("link:a@href".into()),
            ],
            None,
        );

        job.debug = true;

        let items = extractor.extract(&job, None, UGLY_HTML).unwrap();

        assert_eq!(items.len(), 1);
        assert!(items[0].parent_html.as_deref().unwrap().contains("job-tag"));
        assert!(
            items[0]
                .field_match_html
                .as_ref()
                .unwrap()
                .get("desc")
                .unwrap()
                .contains(r#"<div class="desc fs-14 d-none d-sm-block">"#)
        );

        let mut html_path = PathBuf::from(".");
        html_path.push(format!("{}-response.html", job.id()));
        let mut json_path = PathBuf::from(".");
        json_path.push(format!("{}-fields.json", job.id()));
        let _ = fs::remove_file(html_path);
        let _ = fs::remove_file(json_path);
    }

    #[test]
    fn resolves_relative_href_to_absolute_url_in_raw_mode() {
        let extractor = Extractor::new();
        let job = job(
            ".jobpost-cat-box",
            vec![Field::Shorthand("link:a@href".into())],
            None,
        );

        let items = extractor.extract(&job, None, UGLY_HTML).unwrap();

        assert_eq!(
            items[0].fields.get("link").map(String::as_str),
            Some("https://example.com/job/Project-Manager-1625213")
        );
    }

    #[test]
    fn complex_field_selector_can_fallback_to_dom_even_when_parent_uses_raw_parser() {
        let extractor = Extractor::new();
        let job = job(
            ".jobpost-cat-box",
            vec![Field::Full {
                name: "see_more".into(),
                selector: r#"a[target="_blank"]"#.into(),
                att: "href".into(),
            }],
            None,
        );

        let items = extractor.extract(&job, None, UGLY_HTML).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].fields.get("see_more").map(String::as_str),
            Some("https://example.com/job/1625213")
        );
    }
}
