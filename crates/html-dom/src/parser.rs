use std::collections::BTreeMap;
use wm_proto::SurfaceMetadata;

#[derive(Clone, Debug, PartialEq)]
pub enum RawParsedNode {
    Element {
        tag: String,
        attributes: BTreeMap<String, String>,
        children: Vec<RawParsedNode>,
    },
    Text(String),
}

#[derive(Clone, Debug)]
pub struct ParsedHtmlOutput {
    pub metadata: SurfaceMetadata,
    pub embedded_styles: Vec<String>,
    pub root: RawParsedNode,
}

pub fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag,
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
            | "path"
            | "circle"
            | "rect"
            | "line"
            | "polyline"
            | "polygon"
            | "ellipse"
            | "stop"
            | "use"
    )
}

pub fn parse_inline_styles(style_str: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for decl in style_str.split(';') {
        let trimmed = decl.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((prop, val)) = trimmed.split_once(':') {
            let prop = prop.trim().to_string();
            let val = val.trim().to_string();
            if !prop.is_empty() && !val.is_empty() {
                map.insert(prop, val);
            }
        }
    }
    map
}

pub fn format_inline_styles(styles: &BTreeMap<String, String>) -> String {
    styles
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn parse_classes(class_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in class_str.split_whitespace() {
        if !part.is_empty() && !out.iter().any(|c| c == part) {
            out.push(part.to_string());
        }
    }
    out
}

pub fn decode_html_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

pub fn parse_html_document(html: &str) -> ParsedHtmlOutput {
    let mut metadata = SurfaceMetadata::default();
    let mut embedded_styles = Vec::new();

    let raw_nodes = tokenize_and_build_tree(html, &mut metadata, &mut embedded_styles);
    let root = select_document_root(raw_nodes, &mut metadata);

    if let RawParsedNode::Element { attributes, .. } = &root {
        apply_attrs_to_metadata(attributes, &mut metadata, false);
    }

    ParsedHtmlOutput {
        metadata,
        embedded_styles,
        root,
    }
}

fn select_document_root(
    nodes: Vec<RawParsedNode>,
    metadata: &mut SurfaceMetadata,
) -> RawParsedNode {
    let mut element_nodes: Vec<RawParsedNode> = nodes
        .into_iter()
        .filter(|n| matches!(n, RawParsedNode::Element { .. }))
        .collect();

    // If top-level has <html>, unwrap it and look for <body>
    if let Some(pos) = element_nodes.iter().position(|n| {
        matches!(n, RawParsedNode::Element { tag, .. } if tag.eq_ignore_ascii_case("html"))
    }) {
        if let RawParsedNode::Element {
            attributes,
            children,
            ..
        } = element_nodes.remove(pos)
        {
            apply_attrs_to_metadata(&attributes, metadata, false);
            return select_document_root(children, metadata);
        }
    }

    // Filter out non-visual head/meta/style/script/title/link elements
    element_nodes.retain(|n| match n {
        RawParsedNode::Element { tag, .. } => !matches!(
            tag.as_str(),
            "head" | "meta" | "title" | "link" | "style" | "script"
        ),
        RawParsedNode::Text(_) => true,
    });

    // If there is a <body> element, use it directly as the root node
    if let Some(pos) = element_nodes.iter().position(|n| {
        matches!(n, RawParsedNode::Element { tag, .. } if tag.eq_ignore_ascii_case("body"))
    }) {
        let body = element_nodes.remove(pos);
        if let RawParsedNode::Element { ref attributes, .. } = body {
            apply_attrs_to_metadata(attributes, metadata, false);
        }
        return body;
    }

    if element_nodes.len() == 1 {
        let single = element_nodes.remove(0);
        if let RawParsedNode::Element { ref attributes, .. } = single {
            apply_attrs_to_metadata(attributes, metadata, false);
        }
        return single;
    }

    RawParsedNode::Element {
        tag: "body".to_string(),
        attributes: BTreeMap::new(),
        children: element_nodes,
    }
}

fn tokenize_and_build_tree(
    html: &str,
    metadata: &mut SurfaceMetadata,
    embedded_styles: &mut Vec<String>,
) -> Vec<RawParsedNode> {
    let mut stack: Vec<(String, BTreeMap<String, String>, Vec<RawParsedNode>)> = Vec::new();
    let mut root_children: Vec<RawParsedNode> = Vec::new();

    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'<' {
            // Check for comment <!-- ... -->
            if html[i..].starts_with("<!--") {
                if let Some(end_offset) = html[i + 4..].find("-->") {
                    i += 4 + end_offset + 3;
                } else {
                    break;
                }
                continue;
            }

            // Check for <!DOCTYPE ...> or <![CDATA[...]]> or <!...>
            if html[i..].starts_with("<!") || html[i..].starts_with("<?") {
                if let Some(end_offset) = find_tag_end(&html[i + 2..]) {
                    i += 2 + end_offset + 1;
                } else {
                    break;
                }
                continue;
            }

            // Closing tag </tag>
            if i + 1 < len && bytes[i + 1] == b'/' {
                if let Some(end_offset) = find_tag_end(&html[i + 2..]) {
                    let close_tag = html[i + 2..i + 2 + end_offset]
                        .trim()
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    i += 2 + end_offset + 1;

                    if is_void_tag(&close_tag) {
                        continue;
                    }

                    if let Some(match_idx) =
                        stack.iter().rposition(|(open_tag, _, _)| open_tag == &close_tag)
                    {
                        while stack.len() > match_idx {
                            let (tag, attrs, children) = stack.pop().unwrap();
                            let node = RawParsedNode::Element {
                                tag,
                                attributes: attrs,
                                children,
                            };
                            push_node(&mut stack, &mut root_children, node);
                        }
                    }
                    continue;
                } else {
                    break;
                }
            }

            // Opening tag <tag ...> or <tag ... />
            if let Some(end_offset) = find_tag_end(&html[i + 1..]) {
                let inside = &html[i + 1..i + 1 + end_offset];
                i += 1 + end_offset + 1;

                let trimmed = inside.trim();
                let self_closing = trimmed.ends_with('/');
                let content = if self_closing {
                    trimmed[..trimmed.len() - 1].trim()
                } else {
                    trimmed
                };

                if content.is_empty() {
                    continue;
                }

                let (tag_raw, attrs_str) = split_tag_and_attrs(content);
                let tag = tag_raw.to_ascii_lowercase();
                let attributes = parse_attributes(attrs_str);

                if tag == "meta" {
                    handle_meta_tag(&attributes, metadata);
                    continue;
                }

                if tag == "style" && !self_closing {
                    // Consume raw text until </style>
                    if let Some(close_pos) = find_case_insensitive(&html[i..], "</style>") {
                        let css_text = html[i..i + close_pos].trim().to_string();
                        if !css_text.is_empty() {
                            embedded_styles.push(css_text);
                        }
                        i += close_pos + "</style>".len();
                    }
                    continue;
                }

                if tag == "script" && !self_closing {
                    if let Some(close_pos) = find_case_insensitive(&html[i..], "</script>") {
                        i += close_pos + "</script>".len();
                    }
                    continue;
                }

                if self_closing || is_void_tag(&tag) {
                    let node = RawParsedNode::Element {
                        tag,
                        attributes,
                        children: Vec::new(),
                    };
                    push_node(&mut stack, &mut root_children, node);
                } else {
                    stack.push((tag, attributes, Vec::new()));
                }
                continue;
            } else {
                // Malformed '<', treat as text
                i += 1;
                continue;
            }
        } else {
            // Text content up to next '<'
            let next_lt = html[i..].find('<').map(|off| i + off).unwrap_or(len);
            let raw_text = &html[i..next_lt];
            i = next_lt;

            if let Some(normalized) = normalize_text_segment(raw_text) {
                let decoded = decode_html_entities(&normalized);
                push_node(&mut stack, &mut root_children, RawParsedNode::Text(decoded));
            }
        }
    }

    // Close any remaining unclosed tags
    while let Some((tag, attrs, children)) = stack.pop() {
        let node = RawParsedNode::Element {
            tag,
            attributes: attrs,
            children,
        };
        push_node(&mut stack, &mut root_children, node);
    }

    root_children
}

fn push_node(
    stack: &mut Vec<(String, BTreeMap<String, String>, Vec<RawParsedNode>)>,
    root_children: &mut Vec<RawParsedNode>,
    node: RawParsedNode,
) {
    if let Some((_, _, parent_children)) = stack.last_mut() {
        parent_children.push(node);
    } else {
        root_children.push(node);
    }
}

fn normalize_text_segment(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    if raw.contains('\n') || raw.contains('\r') {
        let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            None
        } else {
            Some(collapsed)
        }
    } else {
        Some(raw.to_string())
    }
}

/// Finds the closing `>` of a tag while ignoring `>` inside single or double quotes.
fn find_tag_end(s: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in s.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '>' if !in_single && !in_double => return Some(idx),
            _ => {}
        }
    }
    None
}

fn find_case_insensitive(haystack: &str, needle_lower: &str) -> Option<usize> {
    let nlen = needle_lower.len();
    if haystack.len() < nlen {
        return None;
    }
    for (idx, _) in haystack.char_indices() {
        if idx + nlen <= haystack.len()
            && haystack.is_char_boundary(idx + nlen)
            && haystack[idx..idx + nlen].eq_ignore_ascii_case(needle_lower)
        {
            return Some(idx);
        }
    }
    None
}

fn split_tag_and_attrs(content: &str) -> (&str, &str) {
    for (idx, ch) in content.char_indices() {
        if ch.is_whitespace() {
            return (&content[..idx], content[idx..].trim());
        }
    }
    (content, "")
}

fn parse_attributes(s: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let n = chars.len();
    let mut p = 0;

    while p < n {
        // Skip whitespace and stray '/'
        while p < n && (chars[p].1.is_whitespace() || chars[p].1 == '/') {
            p += 1;
        }
        if p >= n {
            break;
        }

        // Read attribute name
        let name_start = chars[p].0;
        while p < n && !chars[p].1.is_whitespace() && chars[p].1 != '=' && chars[p].1 != '/' {
            p += 1;
        }
        let name_end = if p < n { chars[p].0 } else { s.len() };
        let name = s[name_start..name_end].trim().to_string();
        if name.is_empty() {
            p += 1;
            continue;
        }

        // Skip whitespace before '='
        while p < n && chars[p].1.is_whitespace() {
            p += 1;
        }

        if p < n && chars[p].1 == '=' {
            p += 1; // consume '='
            while p < n && chars[p].1.is_whitespace() {
                p += 1;
            }
            if p >= n {
                attrs.insert(name, String::new());
                break;
            }
            let quote = chars[p].1;
            if quote == '"' || quote == '\'' {
                p += 1; // consume opening quote
                let val_start = if p < n { chars[p].0 } else { s.len() };
                while p < n && chars[p].1 != quote {
                    p += 1;
                }
                let val_end = if p < n { chars[p].0 } else { s.len() };
                let val = decode_html_entities(&s[val_start..val_end]);
                attrs.insert(name, val);
                if p < n && chars[p].1 == quote {
                    p += 1; // consume closing quote
                }
            } else {
                let val_start = chars[p].0;
                while p < n && !chars[p].1.is_whitespace() {
                    p += 1;
                }
                let val_end = if p < n { chars[p].0 } else { s.len() };
                let val = decode_html_entities(&s[val_start..val_end]);
                attrs.insert(name, val);
            }
        } else {
            // Boolean attribute without '='
            attrs.insert(name, "true".to_string());
        }
    }

    attrs
}

fn handle_meta_tag(attrs: &BTreeMap<String, String>, metadata: &mut SurfaceMetadata) {
    let meta_name = attrs
        .get("name")
        .or_else(|| attrs.get("property"))
        .map(|s| s.trim().to_ascii_lowercase());

    match meta_name.as_deref() {
        Some("shell-surface") | Some("surface") | Some("lumina-surface") => {
            if let Some(content) = attrs.get("content") {
                let c = content.trim();
                if c.contains('=') {
                    for part in c.split(|ch| ch == ';' || ch == ',') {
                        if let Some((k, v)) = part.split_once('=') {
                            apply_single_metadata_kv(k.trim(), v.trim(), metadata);
                        }
                    }
                } else if !c.is_empty() {
                    metadata.name = c.to_string();
                }
            }
            apply_attrs_to_metadata(attrs, metadata, true);
        }
        Some(other_key) => {
            if let Some(content) = attrs.get("content") {
                apply_single_metadata_kv(other_key, content.trim(), metadata);
            }
            apply_attrs_to_metadata(attrs, metadata, false);
        }
        None => {
            apply_attrs_to_metadata(attrs, metadata, true);
        }
    }
}

fn apply_attrs_to_metadata(
    attrs: &BTreeMap<String, String>,
    metadata: &mut SurfaceMetadata,
    allow_unprefixed: bool,
) {
    for (k, v) in attrs {
        let key_lower = k.trim().to_ascii_lowercase();
        if let Some(stripped) = key_lower.strip_prefix("data-") {
            apply_single_metadata_kv(stripped, v.trim(), metadata);
        } else if allow_unprefixed && key_lower != "name" && key_lower != "content" {
            apply_single_metadata_kv(&key_lower, v.trim(), metadata);
        } else if matches!(
            key_lower.as_str(),
            "layer"
                | "anchor"
                | "exclusive-zone"
                | "exclusive_zone"
                | "keyboard-mode"
                | "keyboard_mode"
                | "visible-by-default"
        ) {
            apply_single_metadata_kv(&key_lower, v.trim(), metadata);
        }
    }
}

fn apply_single_metadata_kv(key: &str, val: &str, metadata: &mut SurfaceMetadata) {
    match key {
        "name" | "surface" | "surface-name" => {
            if !val.is_empty() {
                metadata.name = val.to_string();
            }
        }
        "layer" => {
            if !val.is_empty() {
                metadata.layer = val.to_ascii_lowercase();
            }
        }
        "anchor" | "anchors" => {
            if val.eq_ignore_ascii_case("none") || val.is_empty() {
                metadata.anchor.clear();
            } else {
                metadata.anchor = val
                    .split(|c: char| c == ',' || c == '|' || c.is_whitespace())
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
        "width" => {
            let num_str = val.trim_end_matches("px").trim();
            if num_str.eq_ignore_ascii_case("auto") || num_str.eq_ignore_ascii_case("none") {
                metadata.width = None;
            } else if let Ok(w) = num_str.parse::<u32>() {
                metadata.width = Some(w);
            }
        }
        "height" => {
            let num_str = val.trim_end_matches("px").trim();
            if num_str.eq_ignore_ascii_case("auto") || num_str.eq_ignore_ascii_case("none") {
                metadata.height = None;
            } else if let Ok(h) = num_str.parse::<u32>() {
                metadata.height = Some(h);
            }
        }
        "exclusive-zone" | "exclusive_zone" => {
            let num_str = val.trim_end_matches("px").trim();
            if let Ok(ez) = num_str.parse::<i32>() {
                metadata.exclusive_zone = ez;
            }
        }
        "keyboard-mode" | "keyboard_mode" | "keyboard" => {
            if !val.is_empty() {
                metadata.keyboard_mode = val.to_ascii_lowercase();
            }
        }
        "visible" | "visible-by-default" | "visible_by_default" => {
            metadata.visible_by_default = !matches!(
                val.to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            );
        }
        _ => {}
    }
}
