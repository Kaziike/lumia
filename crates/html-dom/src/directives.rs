use crate::parser::{format_inline_styles, parse_classes, parse_inline_styles, RawParsedNode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wm_proto::{ActionCommand, DirtyFlags, DomNodeKind, NodeId, ReactiveStore, StateValue};

/// UI event kinds dispatched to DOM nodes and bubbled up the ancestor chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomEventKind {
    Click,
    RightClick,
    MiddleClick,
    DoubleClick,
    ScrollUp,
    ScrollDown,
    Input { value: String },
    KeyDown { key: String },
    PointerDown,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct NodeDirectives {
    pub data_bind: Option<String>,
    pub data_if: Option<String>,
    pub data_show: Option<String>,
    pub class_bindings: Vec<(String, String)>,
    pub style_bindings: Vec<(String, String)>,
    pub attr_bindings: Vec<(String, String)>,
    pub interpolated_attrs: Vec<(String, String)>,
}

impl NodeDirectives {
    pub fn from_attributes(attrs: &BTreeMap<String, String>) -> Self {
        let mut dir = Self::default();
        for (k, v) in attrs {
            if k == "data-bind" {
                dir.data_bind = Some(v.clone());
            } else if k == "data-if" {
                dir.data_if = Some(v.clone());
            } else if k == "data-show" {
                dir.data_show = Some(v.clone());
            } else if let Some(cls) = k.strip_prefix(":class.") {
                if !cls.is_empty() {
                    dir.class_bindings.push((cls.to_string(), v.clone()));
                }
            } else if let Some(cls) = k.strip_prefix("data-class-") {
                if !cls.is_empty() {
                    dir.class_bindings.push((cls.to_string(), v.clone()));
                }
            } else if let Some(prop) = k.strip_prefix(":style.") {
                if !prop.is_empty() {
                    dir.style_bindings.push((prop.to_string(), v.clone()));
                }
            } else if let Some(prop) = k.strip_prefix("data-bind-style-") {
                if !prop.is_empty() {
                    dir.style_bindings.push((prop.to_string(), v.clone()));
                }
            } else if let Some(attr_name) = k.strip_prefix("data-bind-attr-") {
                if !attr_name.is_empty() {
                    dir.attr_bindings.push((attr_name.to_string(), v.clone()));
                }
            } else if let Some(attr_name) = k.strip_prefix(':') {
                if !attr_name.is_empty()
                    && !attr_name.starts_with("class.")
                    && !attr_name.starts_with("style.")
                {
                    dir.attr_bindings.push((attr_name.to_string(), v.clone()));
                }
            } else if v.contains("{{") && v.contains("}}") {
                dir.interpolated_attrs.push((k.clone(), v.clone()));
            }
        }
        dir
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ForDirective {
    pub item_var: String,
    pub index_var: Option<String>,
    pub list_expr: String,
    pub key_expr: Option<String>,
    pub template_nodes: Vec<RawParsedNode>,
    pub instances: Vec<ForInstance>,
}

#[derive(Clone, Debug)]
pub(crate) struct ForInstance {
    pub key: String,
    pub root_nodes: Vec<NodeId>,
}

pub(crate) fn parse_for_expression(raw: &str) -> Option<(String, Option<String>, String)> {
    let trimmed = raw.trim();
    let (lhs, rhs) = trimmed
        .split_once(" in ")
        .or_else(|| trimmed.split_once(" of "))?;
    let list_expr = rhs.trim().to_string();
    if list_expr.is_empty() {
        return None;
    }
    let lhs_clean = lhs.trim().trim_start_matches('(').trim_end_matches(')').trim();
    if let Some((item_part, idx_part)) = lhs_clean.split_once(',') {
        let item_var = item_part.trim().to_string();
        let idx_var = idx_part.trim().to_string();
        if item_var.is_empty() {
            return None;
        }
        Some((
            item_var,
            if idx_var.is_empty() {
                None
            } else {
                Some(idx_var)
            },
            list_expr,
        ))
    } else {
        let item_var = lhs_clean.to_string();
        if item_var.is_empty() {
            return None;
        }
        Some((item_var, None, list_expr))
    }
}

/// Checks whether a `data-for` element should repeat itself inside its parent
/// (Vue `v-for` style) rather than acting as a container repeating its children.
pub(crate) fn is_self_repeating_for_node(
    attributes: &BTreeMap<String, String>,
    children: &[RawParsedNode],
    item_var: &str,
) -> bool {
    let has_element_children = children
        .iter()
        .any(|c| matches!(c, RawParsedNode::Element { .. }));
    if !has_element_children {
        return true;
    }

    let var_dot = format!("{item_var}.");
    for (k, v) in attributes {
        if k == "data-for" || k == "data-key" {
            continue;
        }
        if v.contains(&var_dot) || v.trim() == item_var {
            return true;
        }
    }
    false
}

pub(crate) fn compute_item_key(
    store: &dyn ReactiveStore,
    key_expr: Option<&str>,
    item_var: &str,
    item: &StateValue,
    idx: usize,
    scope: &BTreeMap<String, StateValue>,
) -> String {
    if let Some(k_expr) = key_expr {
        let k_trimmed = k_expr.trim();
        if !k_trimmed.is_empty() {
            if let Ok(val) = evaluate_with_fallback(store, k_trimmed, Some(scope)) {
                let s = val.to_display_string();
                if !s.is_empty() {
                    return s;
                }
            }
            if let Some(sub) = k_trimmed.strip_prefix(&format!("{item_var}.")) {
                if let Some(val) = item.get_path(sub) {
                    let s = val.to_display_string();
                    if !s.is_empty() {
                        return s;
                    }
                }
            }
            if let Some(val) = item.get_path(k_trimmed) {
                let s = val.to_display_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    format!("__idx_{idx}")
}

pub(crate) fn evaluate_with_fallback(
    store: &dyn ReactiveStore,
    expr: &str,
    local_scope: Option<&BTreeMap<String, StateValue>>,
) -> Result<StateValue, String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(StateValue::Null);
    }

    match store.evaluate_expr(trimmed, local_scope) {
        Ok(val) => {
            if !matches!(val, StateValue::Null) {
                return Ok(val);
            }
            // If store returned Null, check if local_scope has it directly in case a simple mock store ignored local_scope
            if let Some(fb) = fallback_eval_expr(store, trimmed, local_scope) {
                if !matches!(fb, StateValue::Null) {
                    return Ok(fb);
                }
            }
            Ok(StateValue::Null)
        }
        Err(e) => {
            if let Some(fb) = fallback_eval_expr(store, trimmed, local_scope) {
                Ok(fb)
            } else {
                Err(e)
            }
        }
    }
}

pub(crate) fn interpolate_with_fallback(
    store: &dyn ReactiveStore,
    template: &str,
    local_scope: Option<&BTreeMap<String, StateValue>>,
) -> Result<String, String> {
    match store.interpolate_string(template, local_scope) {
        Ok(res) => {
            // If the store left unresolved `{{` or returned empty for a local_scope variable in a mock store, check fallback
            if res.contains("{{") {
                Ok(fallback_interpolate(store, template, local_scope))
            } else if let Some(scope) = local_scope {
                if !scope.is_empty() && template.contains("{{") {
                    let fb = fallback_interpolate(store, template, local_scope);
                    if res != fb && !fb.is_empty() && res == strip_template_tags(template) {
                        return Ok(fb);
                    }
                }
                Ok(res)
            } else {
                Ok(res)
            }
        }
        Err(_) => Ok(fallback_interpolate(store, template, local_scope)),
    }
}

fn strip_template_tags(template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start + 2..].find("}}") {
            rest = &rest[start + 2 + end + 2..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

fn fallback_interpolate(
    store: &dyn ReactiveStore,
    template: &str,
    local_scope: Option<&BTreeMap<String, StateValue>>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let expr = after_open[..end].trim();
            if let Ok(val) = evaluate_with_fallback(store, expr, local_scope) {
                out.push_str(&val.to_display_string());
            }
            rest = &after_open[end + 2..];
        } else {
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn fallback_eval_expr(
    store: &dyn ReactiveStore,
    expr: &str,
    local_scope: Option<&BTreeMap<String, StateValue>>,
) -> Option<StateValue> {
    let s = expr.trim();
    if s.is_empty() {
        return Some(StateValue::Null);
    }

    // Logical OR `||`
    if let Some((l, r)) = split_binary_op(s, "||") {
        let lv = fallback_eval_expr(store, l, local_scope)?.as_bool();
        if lv {
            return Some(StateValue::Bool(true));
        }
        let rv = fallback_eval_expr(store, r, local_scope)?.as_bool();
        return Some(StateValue::Bool(rv));
    }

    // Logical AND `&&`
    if let Some((l, r)) = split_binary_op(s, "&&") {
        let lv = fallback_eval_expr(store, l, local_scope)?.as_bool();
        if !lv {
            return Some(StateValue::Bool(false));
        }
        let rv = fallback_eval_expr(store, r, local_scope)?.as_bool();
        return Some(StateValue::Bool(rv));
    }

    // Comparisons
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        if let Some((l, r)) = split_binary_op(s, op) {
            let lv = fallback_eval_expr(store, l, local_scope).unwrap_or(StateValue::Null);
            let rv = fallback_eval_expr(store, r, local_scope).unwrap_or(StateValue::Null);
            let res = match op {
                "==" => values_equal(&lv, &rv),
                "!=" => !values_equal(&lv, &rv),
                ">=" => compare_values(&lv, &rv).map_or(false, |o| o.is_ge()),
                "<=" => compare_values(&lv, &rv).map_or(false, |o| o.is_le()),
                ">" => compare_values(&lv, &rv).map_or(false, |o| o.is_gt()),
                "<" => compare_values(&lv, &rv).map_or(false, |o| o.is_lt()),
                _ => false,
            };
            return Some(StateValue::Bool(res));
        }
    }

    // Unary NOT `!`
    if let Some(inner) = s.strip_prefix('!') {
        let v = fallback_eval_expr(store, inner, local_scope).unwrap_or(StateValue::Null);
        return Some(StateValue::Bool(!v.as_bool()));
    }

    // Literals
    if s == "true" {
        return Some(StateValue::Bool(true));
    }
    if s == "false" {
        return Some(StateValue::Bool(false));
    }
    if s == "null" {
        return Some(StateValue::Null);
    }
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        if s.len() >= 2 {
            return Some(StateValue::String(s[1..s.len() - 1].to_string()));
        }
    }
    if let Ok(i) = s.parse::<i64>() {
        return Some(StateValue::Int(i));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Some(StateValue::Float(f));
    }

    // Lookup in local_scope first
    if let Some(scope) = local_scope {
        if let Some(val) = scope.get(s) {
            return Some(val.clone());
        }
        if let Some((head, tail)) = s.split_once('.') {
            if let Some(root_val) = scope.get(head) {
                if let Some(resolved) = root_val.get_path(tail) {
                    return Some(resolved.clone());
                }
                return Some(StateValue::Null);
            }
        }
    }

    // Lookup in store
    store.get(s)
}

fn split_binary_op<'a>(s: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let mut in_single = false;
    let mut in_double = false;
    let mut depth = 0usize;
    let bytes = s.as_bytes();
    let op_bytes = op.as_bytes();
    let mut i = 0;
    while i + op_bytes.len() <= bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'(' | b'[' if !in_single && !in_double => depth += 1,
            b')' | b']' if !in_single && !in_double => depth = depth.saturating_sub(1),
            _ if !in_single && !in_double && depth == 0 => {
                if &bytes[i..i + op_bytes.len()] == op_bytes {
                    // Avoid matching `>=` when looking for `>`
                    if (op == ">" || op == "<")
                        && i + 1 < bytes.len()
                        && bytes[i + 1] == b'='
                    {
                        i += 2;
                        continue;
                    }
                    return Some((&s[..i], &s[i + op_bytes.len()..]));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn values_equal(a: &StateValue, b: &StateValue) -> bool {
    if a == b {
        return true;
    }
    if let (Some(fa), Some(fb)) = (a.as_f64(), b.as_f64()) {
        if (fa - fb).abs() < 1e-9 {
            return true;
        }
    }
    a.to_display_string() == b.to_display_string()
}

fn compare_values(a: &StateValue, b: &StateValue) -> Option<std::cmp::Ordering> {
    if let (Some(fa), Some(fb)) = (a.as_f64(), b.as_f64()) {
        return fa.partial_cmp(&fb);
    }
    Some(a.to_display_string().cmp(&b.to_display_string()))
}

/// Finds the matching event handler attribute on a node for `event_kind`.
/// Returns `Some((raw_handler_template, should_stop_propagation))`.
pub(crate) fn find_matching_event_attr<'a>(
    raw_attrs: &'a BTreeMap<String, String>,
    event_kind: &DomEventKind,
) -> Option<(&'a str, bool)> {
    let candidate_bases: Vec<String> = match event_kind {
        DomEventKind::Click => vec![
            "on-click".into(),
            "@click".into(),
            "onclick".into(),
            "data-on-click".into(),
        ],
        DomEventKind::RightClick => vec![
            "on-right-click".into(),
            "@right-click".into(),
            "on-contextmenu".into(),
            "@contextmenu".into(),
            "data-on-right-click".into(),
        ],
        DomEventKind::MiddleClick => vec![
            "on-middle-click".into(),
            "@middle-click".into(),
            "on-auxclick".into(),
            "@auxclick".into(),
            "data-on-middle-click".into(),
        ],
        DomEventKind::DoubleClick => vec![
            "on-double-click".into(),
            "on-dblclick".into(),
            "@double-click".into(),
            "@dblclick".into(),
            "data-on-double-click".into(),
        ],
        DomEventKind::ScrollUp => vec![
            "on-scroll-up".into(),
            "@scroll-up".into(),
            "data-on-scroll-up".into(),
        ],
        DomEventKind::ScrollDown => vec![
            "on-scroll-down".into(),
            "@scroll-down".into(),
            "data-on-scroll-down".into(),
        ],
        DomEventKind::Input { .. } => vec![
            "on-input".into(),
            "@input".into(),
            "on-change".into(),
            "@change".into(),
            "data-on-input".into(),
        ],
        DomEventKind::KeyDown { key } => {
            let k = key.trim().to_ascii_lowercase();
            let mut aliases = vec![k.clone()];
            match k.as_str() {
                "enter" => aliases.push("return".into()),
                "return" => aliases.push("enter".into()),
                "escape" => aliases.push("esc".into()),
                "esc" => aliases.push("escape".into()),
                " " | "spacebar" => aliases.push("space".into()),
                _ => {}
            }
            let mut list = Vec::new();
            for alias in aliases {
                list.push(format!("on-keydown-{alias}"));
                list.push(format!("@keydown.{alias}"));
                list.push(format!("@keydown-{alias}"));
                list.push(format!("on-key-{alias}"));
                list.push(format!("data-on-keydown-{alias}"));
            }
            list.push("on-keydown".into());
            list.push("@keydown".into());
            list
        }
        DomEventKind::PointerDown => vec![
            "on-pointer-down".into(),
            "on-pointerdown".into(),
            "on-mousedown".into(),
            "@pointer-down".into(),
            "@pointerdown".into(),
            "@mousedown".into(),
            "data-on-pointer-down".into(),
        ],
    };

    for base in &candidate_bases {
        let stop_key = format!("{base}.stop");
        if let Some(val) = raw_attrs.get(&stop_key) {
            return Some((val.as_str(), true));
        }
        if let Some(val) = raw_attrs.get(base) {
            return Some((val.as_str(), false));
        }
    }
    None
}

/// Evaluates an event handler string (interpolating `{{ ... }}` and `$event.value` / `$event.key`)
/// and parses it into a `Vec<ActionCommand>`.
pub(crate) fn evaluate_event_handler(
    raw_handler: &str,
    event_kind: &DomEventKind,
    local_scope: &BTreeMap<String, StateValue>,
    store: &dyn ReactiveStore,
) -> Vec<ActionCommand> {
    if raw_handler.trim().is_empty() || raw_handler == "true" {
        return Vec::new();
    }

    let mut scope = local_scope.clone();
    let mut prepared = raw_handler.to_string();

    match event_kind {
        DomEventKind::Input { value } => {
            let mut ev_map = BTreeMap::new();
            ev_map.insert("value".to_string(), StateValue::String(value.clone()));
            scope.insert("$event".to_string(), StateValue::Map(ev_map));
            scope
                .entry("value".to_string())
                .or_insert_with(|| StateValue::String(value.clone()));
            prepared = prepared.replace("{{ $event.value }}", value);
            prepared = prepared.replace("{{$event.value}}", value);
            prepared = prepared.replace("$event.value", value);
            prepared = prepared.replace("$value", value);
        }
        DomEventKind::KeyDown { key } => {
            let mut ev_map = BTreeMap::new();
            ev_map.insert("key".to_string(), StateValue::String(key.clone()));
            scope.insert("$event".to_string(), StateValue::Map(ev_map));
            prepared = prepared.replace("{{ $event.key }}", key);
            prepared = prepared.replace("{{$event.key}}", key);
            prepared = prepared.replace("$event.key", key);
        }
        _ => {}
    }

    let interpolated = if prepared.contains("{{") && prepared.contains("}}") {
        interpolate_with_fallback(store, &prepared, Some(&scope)).unwrap_or(prepared)
    } else {
        prepared
    };

    ActionCommand::parse_dsl_list(&interpolated).unwrap_or_default()
}

/// Applies all non-`data-for` reactive directives on a single node slot.
/// Returns the `DirtyFlags` incurred by changes on this node.
pub(crate) fn sync_single_node_directives(
    kind: &DomNodeKind,
    tag: &str,
    element_id: &mut Option<String>,
    classes: &mut Vec<String>,
    attributes: &mut BTreeMap<String, String>,
    inline_styles: &mut BTreeMap<String, String>,
    text_content: &mut Option<String>,
    visible: &mut bool,
    raw_text_template: Option<&str>,
    base_inline_styles: &BTreeMap<String, String>,
    directives: &NodeDirectives,
    local_scope: &BTreeMap<String, StateValue>,
    store: &dyn ReactiveStore,
) -> (DirtyFlags, bool) {
    let mut dirty = DirtyFlags::empty();
    let mut clear_text_children = false;
    let scope_opt = if local_scope.is_empty() {
        None
    } else {
        Some(local_scope)
    };

    // 1. Visibility directives: `data-if` and `data-show`
    if directives.data_if.is_some() || directives.data_show.is_some() {
        let mut new_vis = true;
        if let Some(if_expr) = &directives.data_if {
            let v = evaluate_with_fallback(store, if_expr, scope_opt)
                .map(|sv| sv.as_bool())
                .unwrap_or(false);
            new_vis = new_vis && v;
        }
        if let Some(show_expr) = &directives.data_show {
            let v = evaluate_with_fallback(store, show_expr, scope_opt)
                .map(|sv| sv.as_bool())
                .unwrap_or(false);
            new_vis = new_vis && v;
        }
        if *visible != new_vis {
            *visible = new_vis;
            dirty |= DirtyFlags::ALL;
        }
    }

    // 2. Text node `{{ expr }}` interpolation
    if let Some(raw_tpl) = raw_text_template {
        if raw_tpl.contains("{{") && raw_tpl.contains("}}") {
            if let Ok(new_txt) = interpolate_with_fallback(store, raw_tpl, scope_opt) {
                if text_content.as_deref() != Some(new_txt.as_str()) {
                    *text_content = Some(new_txt);
                    dirty |= DirtyFlags::LAYOUT | DirtyFlags::PAINT;
                }
            }
        }
    }

    if *kind == DomNodeKind::Text {
        return (dirty, false);
    }

    // 3. `data-bind="path"`
    if let Some(bind_expr) = &directives.data_bind {
        let new_val = if bind_expr.contains("{{") && bind_expr.contains("}}") {
            interpolate_with_fallback(store, bind_expr, scope_opt).unwrap_or_default()
        } else {
            evaluate_with_fallback(store, bind_expr, scope_opt)
                .map(|v| v.to_display_string())
                .unwrap_or_default()
        };

        if tag.eq_ignore_ascii_case("input") {
            if attributes.get("value") != Some(&new_val) {
                attributes.insert("value".to_string(), new_val);
                dirty |= DirtyFlags::LAYOUT | DirtyFlags::PAINT;
            }
        } else {
            if text_content.as_deref() != Some(new_val.as_str()) {
                *text_content = Some(new_val);
                clear_text_children = true;
                dirty |= DirtyFlags::LAYOUT | DirtyFlags::PAINT;
            }
        }
    }

    // 4. Interpolated regular attributes `attr="... {{ expr }} ..."`
    for (attr_name, raw_val) in &directives.interpolated_attrs {
        // Don't clobber `$event.value` in event handlers before dispatch
        if raw_val.contains("$event") {
            continue;
        }
        if let Ok(interp) = interpolate_with_fallback(store, raw_val, scope_opt) {
            if attributes.get(attr_name) != Some(&interp) {
                attributes.insert(attr_name.clone(), interp.clone());
                if attr_name == "id" {
                    *element_id = Some(interp);
                } else if attr_name == "class" {
                    *classes = parse_classes(&interp);
                } else if attr_name == "style" {
                    *inline_styles = parse_inline_styles(&interp);
                }
                dirty |= DirtyFlags::STYLE | DirtyFlags::LAYOUT | DirtyFlags::PAINT;
            }
        }
    }

    // 5. `:attr="expr"` and `data-bind-attr-X="expr"`
    for (attr_name, expr) in &directives.attr_bindings {
        let resolved = if expr.contains("{{") && expr.contains("}}") {
            interpolate_with_fallback(store, expr, scope_opt).unwrap_or_default()
        } else {
            evaluate_with_fallback(store, expr, scope_opt)
                .map(|v| v.to_display_string())
                .unwrap_or_default()
        };
        if attributes.get(attr_name) != Some(&resolved) {
            attributes.insert(attr_name.clone(), resolved.clone());
            if attr_name == "id" {
                *element_id = if resolved.is_empty() {
                    None
                } else {
                    Some(resolved)
                };
            } else if attr_name == "class" {
                *classes = parse_classes(&resolved);
            } else if attr_name == "style" {
                *inline_styles = parse_inline_styles(&resolved);
            }
            dirty |= DirtyFlags::STYLE | DirtyFlags::LAYOUT | DirtyFlags::PAINT;
        }
    }

    // 6. `:class.foo="bool_expr"` and `data-class-foo="bool_expr"`
    if !directives.class_bindings.is_empty() {
        let mut classes_changed = false;
        for (cls_name, bool_expr) in &directives.class_bindings {
            let active = evaluate_with_fallback(store, bool_expr, scope_opt)
                .map(|v| v.as_bool())
                .unwrap_or(false);
            let has_it = classes.iter().any(|c| c == cls_name);
            if active && !has_it {
                classes.push(cls_name.clone());
                classes_changed = true;
            } else if !active && has_it {
                classes.retain(|c| c != cls_name);
                classes_changed = true;
            }
        }
        if classes_changed {
            attributes.insert("class".to_string(), classes.join(" "));
            dirty |= DirtyFlags::STYLE | DirtyFlags::LAYOUT | DirtyFlags::PAINT;
        }
    }

    // 7. `:style.prop="expr"` and `data-bind-style-prop="expr"`
    if !directives.style_bindings.is_empty() {
        let mut styles_changed = false;
        // Ensure base inline styles are preserved if not overridden
        for (bk, bv) in base_inline_styles {
            if !inline_styles.contains_key(bk) {
                inline_styles.insert(bk.clone(), bv.clone());
            }
        }
        for (prop_name, expr) in &directives.style_bindings {
            let resolved = if expr.contains("{{") && expr.contains("}}") {
                interpolate_with_fallback(store, expr, scope_opt).unwrap_or_default()
            } else {
                evaluate_with_fallback(store, expr, scope_opt)
                    .map(|v| v.to_display_string())
                    .unwrap_or_default()
            };
            if resolved.is_empty() {
                if inline_styles.remove(prop_name).is_some() {
                    styles_changed = true;
                }
            } else if inline_styles.get(prop_name) != Some(&resolved) {
                inline_styles.insert(prop_name.clone(), resolved);
                styles_changed = true;
            }
        }
        if styles_changed {
            attributes.insert("style".to_string(), format_inline_styles(inline_styles));
            dirty |= DirtyFlags::STYLE | DirtyFlags::LAYOUT | DirtyFlags::PAINT;
        }
    }

    (dirty, clear_text_children)
}
