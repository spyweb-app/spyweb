// use smallvec::SmallVec;
use std::collections::HashMap;

use anyhow::{Result, anyhow};
use scraper::{ElementRef, Html, Selector};

use crate::{config::types::JobConfig, scraper::extractor::matching_keywords};

use super::{
    ExtractedItem, FieldSpec, decode_html_entities, extract_dom_value, has_match,
    normalize_whitespace, resolve_attr_value, safe_element_html,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Combinator {
    Descendant,
    Child,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SimpleSelector {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectorPath {
    steps: Vec<(Combinator, SimpleSelector)>,
}

#[derive(Debug, Clone)]
struct RawNode {
    tag_name: String,
    attrs: HashMap<String, String>,
    // tag_name: std::ops::Range<usize>,
    // attrs: SmallVec<[AttrRange; 4]>,
    start: usize,
    open_end: usize,
    close_start: usize,
    end: usize,
    children: Vec<usize>,
    // children: SmallVec<[usize; 4]>,
}

#[derive(Debug, Clone)]
struct RawDocument<'a> {
    html: &'a str,
    nodes: Vec<RawNode>,
    roots: Vec<usize>,
}

// type AttrRange = (std::ops::Range<usize>, std::ops::Range<usize>);

pub(super) fn extract(
    job: &JobConfig,
    html: &str,
    field_specs: &[FieldSpec],
) -> Result<Option<Vec<ExtractedItem>>> {
    let Some(item_selector) = SelectorPath::parse(&job.selector) else {
        return Ok(None);
    };

    let raw_document = RawDocument::parse(html);
    let raw_items = raw_document.select_all(&item_selector);
    if raw_items.is_empty() {
        return Ok(None);
    }

    let mut items = Vec::new();
    for &item_index in &raw_items {
        let item_html = raw_document.node_html(item_index).to_owned();
        let fragment = Html::parse_fragment(&item_html);
        let fragment_root = fragment.root_element();
        let mut fields = HashMap::new();
        let mut field_match_html = job.debug.then(HashMap::new);
        let mut field_parsers = job.debug.then(HashMap::new);

        for spec in field_specs {
            let mut parser_type = "raw";
            let (value, matched_html) = if let Some(selector) = SelectorPath::parse(&spec.selector)
            {
                let matched = raw_document.select_first_within(item_index, &selector);
                let value = matched
                    .map(|index| extract_raw_value(&raw_document, index, &spec.attr, &job.url))
                    .unwrap_or_default();
                let matched_html = matched
                    .map(|index| raw_document.node_html(index).to_owned())
                    .unwrap_or_default();
                (value, matched_html)
            } else {
                parser_type = "dom-fallback";
                extract_field_with_dom_fallback(&fragment_root, spec, &job.url)?
            };

            fields.insert(spec.name.clone(), value);

            if let Some(debug_matches) = field_match_html.as_mut() {
                debug_matches.insert(spec.name.clone(), matched_html);
            }
            if let Some(parsers) = field_parsers.as_mut() {
                parsers.insert(spec.name.clone(), parser_type.to_string());
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
            parser: job.debug.then(|| "raw".to_string()),
            field_parsers,
            parent_html: job
                .debug
                .then(|| raw_document.node_html(item_index).to_owned()),
            field_match_html,
            matches,
        });
    }

    Ok(Some(items))
}

fn extract_field_with_dom_fallback(
    root: &ElementRef<'_>,
    spec: &FieldSpec,
    base_url: &str,
) -> Result<(String, String)> {
    let matched = if spec.selector.is_empty() {
        Some(*root)
    } else {
        let selector = Selector::parse(&spec.selector)
            .map_err(|err| anyhow!("invalid field selector '{}': {err}", spec.selector))?;
        if selector.matches(root) {
            Some(*root)
        } else {
            root.select(&selector).next()
        }
    };

    let value = matched
        .as_ref()
        .map(|target| extract_dom_value(target, &spec.attr, base_url))
        .unwrap_or_default();
    let matched_html = matched.map(safe_element_html).unwrap_or_default();

    Ok((value, matched_html))
}

fn extract_raw_value(
    document: &RawDocument<'_>,
    index: usize,
    attr: &str,
    base_url: &str,
) -> String {
    let node = &document.nodes[index];

    if attr.eq_ignore_ascii_case("text") {
        return normalize_whitespace(&decode_html_entities(&collect_raw_text(document, index)));
    }

    node.attrs
        .get(&attr.to_ascii_lowercase())
        .map(|value| normalize_whitespace(&decode_html_entities(value)))
        .map(|value| resolve_attr_value(base_url, attr, &value))
        .unwrap_or_default()
}

fn collect_raw_text(document: &RawDocument<'_>, index: usize) -> String {
    let node = &document.nodes[index];
    let mut text = String::new();
    let mut cursor = node.open_end;

    for &child_index in &node.children {
        let child = &document.nodes[child_index];
        if cursor < child.start {
            text.push_str(&document.html[cursor..child.start]);
            text.push(' ');
        }
        text.push_str(&collect_raw_text(document, child_index));
        text.push(' ');
        cursor = child.end;
    }

    if cursor < node.close_start {
        text.push_str(&document.html[cursor..node.close_start]);
    }

    text
}

impl SelectorPath {
    fn parse(input: &str) -> Option<Self> {
        let normalized = input.replace('>', " > ");
        let mut combinator = Combinator::Descendant;
        let mut steps = Vec::new();

        for part in normalized.split_whitespace() {
            if part == ">" {
                combinator = Combinator::Child;
                continue;
            }

            steps.push((combinator, SimpleSelector::parse(part)?));
            combinator = Combinator::Descendant;
        }

        if steps.is_empty() {
            return None;
        }

        Some(Self { steps })
    }
}

impl SimpleSelector {
    fn parse(input: &str) -> Option<Self> {
        let token = input.trim();
        if token.is_empty() || token.contains('[') || token.contains(':') {
            return None;
        }

        let mut tag = None;
        let mut id = None;
        let mut classes = Vec::new();

        let mut current = String::new();
        let mut mode = 't'; // t=tag, i=id, c=class

        for c in token.chars() {
            if c == '#' || c == '.' {
                match mode {
                    't' => {
                        if !current.is_empty() {
                            tag = Some(current.to_ascii_lowercase())
                        }
                    }
                    'i' => {
                        if !current.is_empty() {
                            id = Some(current)
                        }
                    }
                    'c' => {
                        if !current.is_empty() {
                            classes.push(current)
                        }
                    }
                    _ => {}
                }
                current = String::new();
                mode = if c == '#' { 'i' } else { 'c' };
            } else {
                current.push(c);
            }
        }

        match mode {
            't' => {
                if !current.is_empty() {
                    tag = Some(current.to_ascii_lowercase())
                }
            }
            'i' => {
                if !current.is_empty() {
                    id = Some(current)
                }
            }
            'c' => {
                if !current.is_empty() {
                    classes.push(current)
                }
            }
            _ => {}
        }

        if tag.is_none() && id.is_none() && classes.is_empty() {
            return None;
        }

        Some(Self { tag, id, classes })
    }

    fn matches(&self, node: &RawNode) -> bool {
        if let Some(tag) = &self.tag
            && &node.tag_name != tag
        {
            return false;
        }

        if let Some(id) = &self.id {
            let node_id = node.attrs.get("id").map(String::as_str).unwrap_or_default();
            if node_id != id {
                return false;
            }
        }

        if self.classes.is_empty() {
            return true;
        }

        let Some(class_attr) = node.attrs.get("class") else {
            return false;
        };
        let class_names = class_attr.split_whitespace().collect::<Vec<_>>();

        self.classes
            .iter()
            .all(|class_name| class_names.iter().any(|name| name == class_name))
    }
}

impl<'a> RawDocument<'a> {
    fn parse(html: &'a str) -> Self {
        let mut document = Self {
            html,
            nodes: Vec::new(),
            roots: Vec::new(),
        };
        let mut stack: Vec<usize> = Vec::new();
        let bytes = html.as_bytes();
        let mut cursor = 0;

        while cursor < bytes.len() {
            let Some(open_rel) = html[cursor..].find('<') else {
                break;
            };
            let tag_start = cursor + open_rel;
            let Some(tag_end) = find_tag_end(html, tag_start) else {
                break;
            };

            let inner = &html[tag_start + 1..tag_end - 1];
            let trimmed = inner.trim();
            if trimmed.is_empty() || trimmed.starts_with('!') || trimmed.starts_with('?') {
                cursor = tag_end;
                continue;
            }

            if let Some(stripped) = trimmed.strip_prefix('/') {
                let tag_name = stripped
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                close_until_match(
                    &mut document.nodes,
                    &mut stack,
                    &tag_name,
                    tag_start,
                    tag_end,
                );
                cursor = tag_end;
                continue;
            }

            let Some((tag_name, attrs, self_closing)) = parse_start_tag(trimmed) else {
                cursor = tag_end;
                continue;
            };

            let parent = stack.last().copied();
            let index = document.nodes.len();
            document.nodes.push(RawNode {
                tag_name: tag_name.clone(),
                attrs,
                start: tag_start,
                open_end: tag_end,
                close_start: tag_end,
                end: tag_end,
                children: Vec::new(),
            });

            if let Some(parent_index) = parent {
                document.nodes[parent_index].children.push(index);
            } else {
                document.roots.push(index);
            }

            if !self_closing && !is_void_tag(&tag_name) {
                stack.push(index);
            }

            cursor = tag_end;
        }

        for index in stack {
            let len = html.len();
            document.nodes[index].close_start = len;
            document.nodes[index].end = len;
        }

        document
    }

    fn select_all(&self, selector: &SelectorPath) -> Vec<usize> {
        let mut matches = Vec::new();
        for &root in &self.roots {
            self.collect_matches(root, &selector.steps[0].1, true, &mut matches);
        }

        for (combinator, step) in selector.steps.iter().skip(1) {
            let mut next = Vec::new();
            for index in matches {
                match combinator {
                    Combinator::Descendant => {
                        for &child in &self.nodes[index].children {
                            self.collect_matches(child, step, true, &mut next);
                        }
                    }
                    Combinator::Child => {
                        for &child in &self.nodes[index].children {
                            if step.matches(&self.nodes[child]) {
                                next.push(child);
                            }
                        }
                    }
                }
            }
            matches = next;
        }

        matches
    }

    fn select_first_within(&self, root: usize, selector: &SelectorPath) -> Option<usize> {
        let mut current = Vec::new();

        let (first_combinator, first_step) = &selector.steps[0];

        if matches!(first_combinator, Combinator::Descendant)
            && first_step.matches(&self.nodes[root])
        {
            current.push(root);
        }

        for &child in &self.nodes[root].children {
            self.collect_matches(child, &selector.steps[0].1, true, &mut current);
        }

        for (combinator, step) in selector.steps.iter().skip(1) {
            let mut next = Vec::new();
            for index in current {
                match combinator {
                    Combinator::Descendant => {
                        for &child in &self.nodes[index].children {
                            self.collect_matches(child, step, true, &mut next);
                        }
                    }
                    Combinator::Child => {
                        for &child in &self.nodes[index].children {
                            if step.matches(&self.nodes[child]) {
                                next.push(child);
                            }
                        }
                    }
                }
            }
            current = next;
        }

        current.into_iter().next()
    }

    fn collect_matches(
        &self,
        index: usize,
        selector: &SimpleSelector,
        recurse: bool,
        matches: &mut Vec<usize>,
    ) {
        if selector.matches(&self.nodes[index]) {
            matches.push(index);
        }

        if recurse {
            for &child in &self.nodes[index].children {
                self.collect_matches(child, selector, true, matches);
            }
        }
    }

    fn node_html(&self, index: usize) -> &'a str {
        let node = &self.nodes[index];
        &self.html[node.start..node.end]
    }
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut cursor = start + 1;
    let mut quote: Option<u8> = None;

    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(current_quote) = quote {
            if byte == current_quote {
                quote = None;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some(cursor + 1);
        }

        cursor += 1;
    }

    None
}

fn parse_start_tag(input: &str) -> Option<(String, HashMap<String, String>, bool)> {
    let self_closing = input.trim_end().ends_with('/');
    let body = input.trim_end_matches('/').trim();
    let mut chars = body.char_indices().peekable();
    let mut end = body.len();

    while let Some((index, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            end = index;
            break;
        }
        chars.next();
    }

    let tag_name = body[..end].trim().to_ascii_lowercase();
    if tag_name.is_empty() {
        return None;
    }

    let attrs = parse_attributes(&body[end..]);
    Some((tag_name, attrs, self_closing))
}

fn parse_attributes(input: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let bytes = input.as_bytes();
    let mut cursor = 0;

    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }

        let name_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }

        if name_start == cursor {
            cursor += 1;
            continue;
        }

        let name = input[name_start..cursor].trim().to_ascii_lowercase();

        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }

        let mut value = String::new();
        if cursor < bytes.len() && bytes[cursor] == b'=' {
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }

            if cursor < bytes.len() && (bytes[cursor] == b'"' || bytes[cursor] == b'\'') {
                let quote = bytes[cursor];
                cursor += 1;
                let value_start = cursor;
                while cursor < bytes.len() && bytes[cursor] != quote {
                    cursor += 1;
                }
                value = input[value_start..cursor].to_owned();
                if cursor < bytes.len() {
                    cursor += 1;
                }
            } else {
                let value_start = cursor;
                while cursor < bytes.len()
                    && !bytes[cursor].is_ascii_whitespace()
                    && bytes[cursor] != b'/'
                {
                    cursor += 1;
                }
                value = input[value_start..cursor].to_owned();
            }
        }

        attrs.insert(name, value);
    }

    attrs
}

fn close_until_match(
    nodes: &mut [RawNode],
    stack: &mut Vec<usize>,
    tag_name: &str,
    close_start: usize,
    close_end: usize,
) {
    let Some(position) = stack
        .iter()
        .rposition(|&index| nodes[index].tag_name == tag_name)
    else {
        return;
    };

    while stack.len() > position {
        let index = stack.pop().unwrap();
        nodes[index].close_start = close_start;
        nodes[index].end = close_end;
    }
}

fn is_void_tag(tag_name: &str) -> bool {
    matches!(
        tag_name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

#[cfg(test)]
#[test]
fn test_select_root_element() {
    let html = r#"
        <div class="listing-box" data-link="parent-link">
            <div class="link-box" data-link="child-link"></div>
        </div>
    "#;

    let doc = RawDocument::parse(html);

    let item_selector = SelectorPath::parse(".listing-box").unwrap();
    let items = doc.select_all(&item_selector);

    assert_eq!(items.len(), 1);
    let item_index = items[0];

    let field_selector = SelectorPath::parse(".listing-box").unwrap();

    let matched = doc.select_first_within(item_index, &field_selector);

    assert!(matched.is_some(), "Should match the root element itself");
    let value = extract_raw_value(&doc, matched.unwrap(), "data-link", "");
    assert_eq!(value, "parent-link");
}

#[test]
fn test_id_selector() {
    let html = r#"
        <div id="target">Found</div>
        <div id="wrong">Miss</div>
    "#;
    let doc = RawDocument::parse(html);
    let sel = SelectorPath::parse("#target").unwrap();
    let matches = doc.select_all(&sel);
    assert_eq!(matches.len(), 1);
    assert_eq!(
        doc.node_html(matches[0]).trim(),
        "<div id=\"target\">Found</div>"
    );
}

#[test]
fn test_tag_id_class_combined() {
    let html = r#"
        <div id="main" class="container active">Match</div>
        <span id="main" class="container">Wrong Tag</span>
        <div id="other" class="container active">Wrong ID</div>
        <div id="main" class="container">Missing Class</div>
    "#;
    let doc = RawDocument::parse(html);

    // Tag + ID + Multiple Classes
    let sel = SelectorPath::parse("div#main.container.active").unwrap();
    let matches = doc.select_all(&sel);
    assert_eq!(matches.len(), 1);
    assert_eq!(
        doc.node_html(matches[0]).trim(),
        "<div id=\"main\" class=\"container active\">Match</div>"
    );
}

#[test]
fn test_multiple_identical_ids() {
    let html = r#"
        <li id="item">One</li>
        <li id="item">Two</li>
    "#;
    let doc = RawDocument::parse(html);
    let sel = SelectorPath::parse("#item").unwrap();
    let matches = doc.select_all(&sel);
    // Even if IDs should be unique, scrapers must handle broken HTML where they aren't
    assert_eq!(matches.len(), 2);
}
