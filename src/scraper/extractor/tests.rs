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
        workers: None,
        urls: None,
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
    <a href="/listing/manager-777"></a>
    <div class="result-card">
      <a href="/listing/manager-777"></a>
      <div class="layout-flex">
        <a href="/listing/manager-777">
          <div class="icon-wrapper">
            <img class="vendor-logo" alt="Acme Global">
          </div>
        </a>
        <div class="main-content">
          <a href="/listing/manager-777">
            <dl class="meta-row">
              <dt>
                <h4 class="title">Product Lead <span class="tag">Remote</span></h4>
              </dt>
            </dl>
            <p class="timestamp" data-meta="2026-05-01 10:00:00">
              <em>Published on 2026-05-01</em>
            </p>
            <dl class="salary-box">
              <dt>
                <i class="currency-icon"></i>
              </dt>
              <dd class="value">95000</dd>
            </dl>
          </a>
          <div class="summary-text hidden-mobile">
            <a href="/listing/manager-777"> Leading a team of developers in a high-growth startup <br> ROLE OVERVIEW <br> Requirements <br> - Leadership… </a>
            <a href="/listing/777" target="_blank">Full Details</a>
          </div>
          <div class="categories">
            <a href="/tags/management" class="pill">Management</a>
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

    let extraction = extractor.extract(&job, None, HTML).unwrap();
    let items = extraction.items;

    assert_eq!(items.len(), 2);
    assert_eq!(extraction.selector_matches, 2);
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

    let extraction = extractor.extract(&job, None, HTML).unwrap();
    let items = extraction.items;

    assert_eq!(items.len(), 1);
    assert_eq!(extraction.selector_matches, 2);
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

    let extraction = extractor.extract(&job, None, html).unwrap();
    let items = extraction.items;

    assert_eq!(items.len(), 1);
    assert_eq!(extraction.selector_matches, 1);

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

    let extraction = extractor.extract(&job, None, HTML).unwrap();
    let items = extraction.items;

    assert_eq!(extraction.selector_matches, 2);
    assert_eq!(
        items[0].fields.get("link").map(String::as_str),
        Some("https://example.com/jobs/rust")
    );
}

#[test]
fn preserves_descendants_in_badly_formatted_html() {
    let extractor = Extractor::new();
    let job = job(
        ".result-card",
        vec![
            Field::Shorthand("title:h4".into()),
            Field::Shorthand("desc:.summary-text".into()),
            Field::Shorthand("link:a@href".into()),
            Field::Shorthand("date:p.timestamp@data-meta".into()),
            Field::Shorthand("rate:dl>dd".into()),
        ],
        None,
    );

    let extraction = extractor.extract(&job, None, UGLY_HTML).unwrap();
    let items = extraction.items;

    assert_eq!(items.len(), 1);
    assert_eq!(extraction.selector_matches, 1);
    assert_eq!(
        items[0].fields.get("title").map(String::as_str),
        Some("Product Lead Remote")
    );
    assert_eq!(
        items[0].fields.get("link").map(String::as_str),
        Some("https://example.com/listing/manager-777")
    );
    assert_eq!(
        items[0].fields.get("date").map(String::as_str),
        Some("2026-05-01 10:00:00")
    );
    assert_eq!(
        items[0].fields.get("rate").map(String::as_str),
        Some("95000")
    );
    assert!(
        items[0]
            .fields
            .get("desc")
            .unwrap()
            .contains("Leading a team of developers")
    );
    assert!(
        items[0]
            .fields
            .get("desc")
            .unwrap()
            .contains("Full Details")
    );
    assert_eq!(items[0].parent_html, None);
    assert_eq!(items[0].field_match_html, None);
}

#[test]
fn includes_debug_html_only_when_debug_is_enabled() {
    let extractor = Extractor::new();
    let mut job = job(
        ".result-card",
        vec![
            Field::Shorthand("title:h4".into()),
            Field::Shorthand("desc:.summary-text".into()),
            Field::Shorthand("link:a@href".into()),
        ],
        None,
    );

    job.debug = true;

    let extraction = extractor.extract(&job, None, UGLY_HTML).unwrap();
    let items = extraction.items;

    assert_eq!(items.len(), 1);
    assert_eq!(extraction.selector_matches, 1);
    assert!(
        items[0]
            .parent_html
            .as_deref()
            .unwrap()
            .contains("categories")
    );
    assert!(
        items[0]
            .field_match_html
            .as_ref()
            .unwrap()
            .get("desc")
            .unwrap()
            .contains(r#"<div class="summary-text hidden-mobile">"#)
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
        ".result-card",
        vec![Field::Shorthand("link:a@href".into())],
        None,
    );

    let extraction = extractor.extract(&job, None, UGLY_HTML).unwrap();
    let items = extraction.items;

    assert_eq!(
        items[0].fields.get("link").map(String::as_str),
        Some("https://example.com/listing/manager-777")
    );
}

#[test]
fn complex_field_selector_can_fallback_to_dom_even_when_parent_uses_raw_parser() {
    let extractor = Extractor::new();
    let job = job(
        ".result-card",
        vec![Field::Full {
            name: "see_more".into(),
            selector: r#"a[target="_blank"]"#.into(),
            att: "href".into(),
        }],
        None,
    );

    let extraction = extractor.extract(&job, None, UGLY_HTML).unwrap();
    let items = extraction.items;

    assert_eq!(items.len(), 1);
    assert_eq!(extraction.selector_matches, 1);
    assert_eq!(
        items[0].fields.get("see_more").map(String::as_str),
        Some("https://example.com/listing/777")
    );
}
