use anyhow::{Result, anyhow};
use scraper::{Html, Selector};

use crate::{config::types::JobConfig, scraper::extractor::matching_keywords};

use super::{ExtractedItem, FieldSpec, extract_dom_value, has_match, safe_element_html};

pub(super) fn extract(
    job: &JobConfig,
    html: &str,
    field_specs: &[FieldSpec],
) -> Result<Vec<ExtractedItem>> {
    let document = Html::parse_fragment(html);
    let item_selector = Selector::parse(&job.selector)
        .map_err(|err| anyhow!("invalid selector '{}': {err}", job.selector))?;

    let mut items = Vec::new();
    for element in document.select(&item_selector) {
        let mut fields = std::collections::HashMap::new();
        let mut field_match_html = job.debug.then(std::collections::HashMap::new);
        let mut field_parsers = job.debug.then(std::collections::HashMap::new);

        for spec in field_specs {
            let matched = if spec.selector.is_empty() {
                Some(element)
            } else {
                let selector = Selector::parse(&spec.selector)
                    .map_err(|err| anyhow!("invalid field selector '{}': {err}", spec.selector))?;

                if selector.matches(&element) {
                    Some(element)
                } else {
                    element.select(&selector).next()
                }
            };

            let value = matched
                .as_ref()
                .map(|target| extract_dom_value(target, &spec.attr, &job.url))
                .unwrap_or_default();
            fields.insert(spec.name.clone(), value);

            if let Some(debug_matches) = field_match_html.as_mut() {
                let matched_html = matched.map(safe_element_html).unwrap_or_default();
                debug_matches.insert(spec.name.clone(), matched_html);
            }
            if let Some(parsers) = field_parsers.as_mut() {
                parsers.insert(spec.name.clone(), "dom".to_string());
            }
        }

        if !has_match(
            &fields,
            job.keywords.as_deref(),
            job.search_fields.as_deref(),
        ) {
            continue;
        }

        let matches = matching_keywords(
            &fields,
            job.keywords.as_deref(),
            job.search_fields.as_deref(),
        );
        items.push(ExtractedItem {
            fields,
            parser: job.debug.then(|| "dom".to_string()),
            field_parsers,
            parent_html: job.debug.then(|| safe_element_html(element)),
            field_match_html,
            matches,
        });
    }

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dom_self_selection() {
        let html = r#"<div class="job-link" data-url="https://example.com">Job Title</div>"#;
        let job = JobConfig {
            name: "test".into(),
            url: "https://example.com".into(),
            selector: ".job-link".into(),
            fields: vec![],
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 300,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
        };
        
        let field_specs = vec![
            FieldSpec {
                name: "link".into(),
                selector: ".job-link".into(),
                attr: "data-url".into(),
            },
            FieldSpec {
                name: "title".into(),
                selector: "".into(), // Empty selector test
                attr: "text".into(),
            }
        ];

        let results = extract(&job, html, &field_specs).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fields["link"], "https://example.com");
        assert_eq!(results[0].fields["title"], "Job Title");
    }

    #[test]
    fn test_dom_descendant_selection() {
        let html = r#"<div class="card"><h4 class="title">Software Engineer</h4></div>"#;
        let job = JobConfig {
            name: "test".into(),
            url: "https://example.com".into(),
            selector: ".card".into(),
            fields: vec![],
            keywords: None,
            search_fields: None,
            webhook: None,
            debug: false,
            enabled: true,
            interval: 300,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
        };
        
        let field_specs = vec![
            FieldSpec {
                name: "title".into(),
                selector: ".title".into(),
                attr: "text".into(),
            }
        ];

        let results = extract(&job, html, &field_specs).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fields["title"], "Software Engineer");
    }
}
