pub use wm_proto;
pub use wm_proto::dom::{DomNodeData, DomNodeKind, DomTreeView, NodeId, PseudoStates};
pub use wm_proto::style::*;

use std::collections::{BTreeMap, HashMap, HashSet};

/// W3C selector specificity tuple: `(id_count, class_attr_pseudo_count, tag_count)`.
pub type Specificity = (u16, u16, u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeOp {
    Exists,
    Equals(String),
    Includes(String),
    DashMatch(String),
    Prefix(String),
    Suffix(String),
    Substring(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeSelector {
    pub name: String,
    pub op: AttributeOp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudoClass {
    Root,
    Hover,
    Active,
    Focus,
    FocusVisible,
    Checked,
    Disabled,
    Urgent,
    FirstChild,
    LastChild,
    NthChild(usize),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompoundSelector {
    pub universal: bool,
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attributes: Vec<AttributeSelector>,
    pub pseudo_classes: Vec<PseudoClass>,
}

impl CompoundSelector {
    pub fn is_empty(&self) -> bool {
        !self.universal
            && self.tag.is_none()
            && self.id.is_none()
            && self.classes.is_empty()
            && self.attributes.is_empty()
            && self.pseudo_classes.is_empty()
    }

    pub fn specificity(&self) -> Specificity {
        let id_count = if self.id.is_some() { 1 } else { 0 };
        let class_count =
            (self.classes.len() + self.attributes.len() + self.pseudo_classes.len()) as u16;
        let tag_count = if self.tag.is_some() { 1 } else { 0 };
        (id_count, class_count, tag_count)
    }

    pub fn matches_root_pseudo_only(&self) -> bool {
        self.id.is_none()
            && self.classes.is_empty()
            && self.attributes.is_empty()
            && ((self.tag.is_none()
                && self.pseudo_classes.len() == 1
                && self.pseudo_classes[0] == PseudoClass::Root)
                || (self.universal && self.pseudo_classes.is_empty())
                || (self.tag.as_deref() == Some("html") && self.pseudo_classes.is_empty())
                || (self.tag.as_deref() == Some("root") && self.pseudo_classes.is_empty()))
    }

    pub fn matches(&self, dom: &dyn DomTreeView, node: &DomNodeData) -> bool {
        if node.kind != DomNodeKind::Element {
            return false;
        }

        if let Some(ref tag) = self.tag {
            if !node.tag.eq_ignore_ascii_case(tag) {
                return false;
            }
        }

        if let Some(ref req_id) = self.id {
            let actual_id = node
                .element_id
                .as_deref()
                .or_else(|| node.get_attr("id"));
            if actual_id != Some(req_id.as_str()) {
                return false;
            }
        }

        for cls in &self.classes {
            if !node.has_class(cls) {
                return false;
            }
        }

        for attr in &self.attributes {
            let val_opt = if attr.name.eq_ignore_ascii_case("id") {
                node.element_id
                    .as_deref()
                    .or_else(|| node.get_attr("id"))
            } else {
                node.get_attr(&attr.name)
            };

            match &attr.op {
                AttributeOp::Exists => {
                    if val_opt.is_none()
                        && !(attr.name.eq_ignore_ascii_case("class") && !node.classes.is_empty())
                    {
                        return false;
                    }
                }
                AttributeOp::Equals(expected) => {
                    if val_opt != Some(expected.as_str()) {
                        return false;
                    }
                }
                AttributeOp::Includes(expected) => {
                    let Some(val) = val_opt else { return false };
                    if !val.split_whitespace().any(|w| w == expected) {
                        return false;
                    }
                }
                AttributeOp::DashMatch(expected) => {
                    let Some(val) = val_opt else { return false };
                    if val != expected && !val.starts_with(&format!("{expected}-")) {
                        return false;
                    }
                }
                AttributeOp::Prefix(expected) => {
                    let Some(val) = val_opt else { return false };
                    if !val.starts_with(expected.as_str()) {
                        return false;
                    }
                }
                AttributeOp::Suffix(expected) => {
                    let Some(val) = val_opt else { return false };
                    if !val.ends_with(expected.as_str()) {
                        return false;
                    }
                }
                AttributeOp::Substring(expected) => {
                    let Some(val) = val_opt else { return false };
                    if !val.contains(expected.as_str()) {
                        return false;
                    }
                }
            }
        }

        for pseudo in &self.pseudo_classes {
            match pseudo {
                PseudoClass::Root => {
                    if node.id != dom.root_id() {
                        return false;
                    }
                }
                PseudoClass::Hover => {
                    if !node.pseudo_states.contains(PseudoStates::HOVER) {
                        return false;
                    }
                }
                PseudoClass::Active => {
                    if !node.pseudo_states.contains(PseudoStates::ACTIVE) {
                        return false;
                    }
                }
                PseudoClass::Focus => {
                    if !node.pseudo_states.contains(PseudoStates::FOCUS) {
                        return false;
                    }
                }
                PseudoClass::FocusVisible => {
                    if !node.pseudo_states.contains(PseudoStates::FOCUS_VISIBLE) {
                        return false;
                    }
                }
                PseudoClass::Checked => {
                    if !node.pseudo_states.contains(PseudoStates::CHECKED)
                        && !node.attributes.contains_key("checked")
                    {
                        return false;
                    }
                }
                PseudoClass::Disabled => {
                    if !node.pseudo_states.contains(PseudoStates::DISABLED)
                        && !node.attributes.contains_key("disabled")
                    {
                        return false;
                    }
                }
                PseudoClass::Urgent => {
                    if !node.pseudo_states.contains(PseudoStates::URGENT) {
                        return false;
                    }
                }
                PseudoClass::FirstChild => {
                    let Some(parent_id) = node.parent.or_else(|| dom.parent_of(node.id)) else {
                        return false;
                    };
                    let first_elem = dom
                        .children_of(parent_id)
                        .iter()
                        .copied()
                        .find(|&cid| {
                            dom.get_node(cid)
                                .map(|n| n.kind == DomNodeKind::Element)
                                .unwrap_or(false)
                        });
                    if first_elem != Some(node.id) {
                        return false;
                    }
                }
                PseudoClass::LastChild => {
                    let Some(parent_id) = node.parent.or_else(|| dom.parent_of(node.id)) else {
                        return false;
                    };
                    let last_elem = dom
                        .children_of(parent_id)
                        .iter()
                        .rev()
                        .copied()
                        .find(|&cid| {
                            dom.get_node(cid)
                                .map(|n| n.kind == DomNodeKind::Element)
                                .unwrap_or(false)
                        });
                    if last_elem != Some(node.id) {
                        return false;
                    }
                }
                PseudoClass::NthChild(idx) => {
                    if *idx == 0 {
                        return false;
                    }
                    let Some(parent_id) = node.parent.or_else(|| dom.parent_of(node.id)) else {
                        return false;
                    };
                    let nth_elem = dom
                        .children_of(parent_id)
                        .iter()
                        .copied()
                        .filter(|&cid| {
                            dom.get_node(cid)
                                .map(|n| n.kind == DomNodeKind::Element)
                                .unwrap_or(false)
                        })
                        .nth(*idx - 1);
                    if nth_elem != Some(node.id) {
                        return false;
                    }
                }
            }
        }

        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub raw: String,
    pub chain: Vec<(Option<Combinator>, CompoundSelector)>,
    pub specificity: Specificity,
}

impl Selector {
    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("Empty selector".to_string());
        }

        let tokens = tokenize_selector_chain(trimmed)?;
        if tokens.is_empty() {
            return Err(format!("Failed to parse selector: '{trimmed}'"));
        }

        let mut chain = Vec::with_capacity(tokens.len());
        let mut spec = (0u16, 0u16, 0u16);

        for (comb, raw_part) in tokens {
            let compound = parse_compound_selector(&raw_part)?;
            let c_spec = compound.specificity();
            spec.0 = spec.0.saturating_add(c_spec.0);
            spec.1 = spec.1.saturating_add(c_spec.1);
            spec.2 = spec.2.saturating_add(c_spec.2);
            chain.push((comb, compound));
        }

        Ok(Self {
            raw: trimmed.to_string(),
            chain,
            specificity: spec,
        })
    }

    pub fn matches(&self, dom: &dyn DomTreeView, node_id: NodeId) -> bool {
        if self.chain.is_empty() {
            return false;
        }
        let Some(node) = dom.get_node(node_id) else {
            return false;
        };
        let last_idx = self.chain.len() - 1;
        if !self.chain[last_idx].1.matches(dom, node) {
            return false;
        }
        self.matches_ancestors(dom, node_id, last_idx)
    }

    fn matches_ancestors(&self, dom: &dyn DomTreeView, current_id: NodeId, idx: usize) -> bool {
        if idx == 0 {
            return true;
        }
        let comb = self.chain[idx].0.unwrap_or(Combinator::Descendant);
        let target_compound = &self.chain[idx - 1].1;

        let parent_of = |id: NodeId| -> Option<NodeId> {
            dom.parent_of(id)
                .or_else(|| dom.get_node(id).and_then(|n| n.parent))
        };

        match comb {
            Combinator::Child => {
                let Some(parent_id) = parent_of(current_id) else {
                    return false;
                };
                let Some(parent_node) = dom.get_node(parent_id) else {
                    return false;
                };
                if !target_compound.matches(dom, parent_node) {
                    return false;
                }
                self.matches_ancestors(dom, parent_id, idx - 1)
            }
            Combinator::Descendant => {
                let mut anc_opt = parent_of(current_id);
                while let Some(anc_id) = anc_opt {
                    if let Some(anc_node) = dom.get_node(anc_id) {
                        if target_compound.matches(dom, anc_node)
                            && self.matches_ancestors(dom, anc_id, idx - 1)
                        {
                            return true;
                        }
                    }
                    anc_opt = parent_of(anc_id);
                }
                false
            }
        }
    }

    pub fn is_root_selector(&self) -> bool {
        self.chain.len() == 1 && self.chain[0].1.matches_root_pseudo_only()
    }
}

#[derive(Clone, Debug)]
pub struct CssRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<(String, String)>,
    pub order: usize,
}

/// Pure-Rust CSS3 Cascade, Variable Resolver, and Style Engine.
#[derive(Clone, Debug, Default)]
pub struct CssStyleEngine {
    rules: Vec<CssRule>,
    next_rule_order: usize,
    programmatic_vars: BTreeMap<String, String>,
    stylesheet_root_vars: BTreeMap<String, String>,
}

impl CssStyleEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_css(css_source: &str) -> Result<Self, String> {
        let mut engine = Self::new();
        engine.load_stylesheet(css_source)?;
        Ok(engine)
    }

    pub fn rules(&self) -> &[CssRule] {
        &self.rules
    }

    fn normalize_var_name(name: &str) -> String {
        let trimmed = name.trim();
        if trimmed.starts_with("--") || trimmed.starts_with("-wm-") {
            trimmed.to_string()
        } else {
            format!("--{trimmed}")
        }
    }

    fn collect_root_vars_from_rules(rules: &[CssRule]) -> BTreeMap<String, String> {
        let mut root_vars = BTreeMap::new();
        let mut root_entries: Vec<(Specificity, usize, &[(String, String)])> = Vec::new();
        for rule in rules {
            for sel in &rule.selectors {
                if sel.is_root_selector() {
                    root_entries.push((sel.specificity, rule.order, &rule.declarations));
                }
            }
        }
        root_entries.sort_by_key(|(spec, order, _)| (*spec, *order));
        for (_, _, decls) in root_entries {
            for (k, v) in decls {
                if k.starts_with("--") || k.starts_with("-wm-") {
                    root_vars.insert(k.clone(), v.clone());
                }
            }
        }
        root_vars
    }

    fn document_order_nodes(&self, dom: &dyn DomTreeView) -> Vec<NodeId> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();

        fn dfs(
            dom: &dyn DomTreeView,
            node_id: NodeId,
            visited: &mut HashSet<NodeId>,
            out: &mut Vec<NodeId>,
        ) {
            if !visited.insert(node_id) {
                return;
            }
            if dom.get_node(node_id).is_some() {
                out.push(node_id);
            }
            for &child_id in dom.children_of(node_id) {
                dfs(dom, child_id, visited, out);
            }
            if let Some(node) = dom.get_node(node_id) {
                for &child_id in &node.children {
                    if !visited.contains(&child_id) {
                        dfs(dom, child_id, visited, out);
                    }
                }
            }
        }

        let root = dom.root_id();
        dfs(dom, root, &mut visited, &mut result);

        for id in dom.all_node_ids() {
            if !visited.contains(&id) {
                dfs(dom, id, &mut visited, &mut result);
            }
        }

        result
    }

    fn match_node_rules(&self, dom: &dyn DomTreeView, node_id: NodeId) -> Vec<(Selector, usize, Vec<(String, String)>)> {
        let mut matched = Vec::new();
        for rule in &self.rules {
            let mut best_selector: Option<&Selector> = None;
            for sel in &rule.selectors {
                if sel.matches(dom, node_id) {
                    match best_selector {
                        None => best_selector = Some(sel),
                        Some(prev) if sel.specificity >= prev.specificity => {
                            best_selector = Some(sel);
                        }
                        _ => {}
                    }
                }
            }
            if let Some(sel) = best_selector {
                matched.push((sel.clone(), rule.order, rule.declarations.clone()));
            }
        }
        matched.sort_by_key(|(sel, order, _)| (sel.specificity, *order));
        matched
    }

    fn compute_all_internal(
        &self,
        dom: &dyn DomTreeView,
    ) -> (
        HashMap<NodeId, ComputedStyle>,
        HashMap<NodeId, BTreeMap<String, String>>,
    ) {
        let order = self.document_order_nodes(dom);
        let mut styles: HashMap<NodeId, ComputedStyle> = HashMap::with_capacity(order.len());
        let mut raw_vars_per_node: HashMap<NodeId, BTreeMap<String, String>> =
            HashMap::with_capacity(order.len());
        let mut resolved_vars_per_node: HashMap<NodeId, BTreeMap<String, String>> =
            HashMap::with_capacity(order.len());

        let root_id = dom.root_id();
        let mut root_font_size = ComputedStyle::default().font_size;

        for &node_id in &order {
            let Some(node) = dom.get_node(node_id) else {
                continue;
            };
            let parent_id = dom.parent_of(node_id).or(node.parent);
            let parent_style = parent_id.and_then(|pid| styles.get(&pid)).cloned();

            // 1. Start with default style + inherited properties from parent
            let mut computed = ComputedStyle::default();
            let parent_font_size = if let Some(ref ps) = parent_style {
                computed.color = ps.color;
                computed.font_family = ps.font_family.clone();
                computed.font_size = ps.font_size;
                computed.font_weight = ps.font_weight;
                computed.line_height = ps.line_height;
                computed.text_align = ps.text_align;
                computed.custom_properties = ps.custom_properties.clone();
                ps.font_size
            } else {
                computed.font_size
            };

            // 2. Build custom property environment (inherited + matched rules + programmatic + inline)
            let mut node_raw_vars = if let Some(pid) = parent_id {
                raw_vars_per_node.get(&pid).cloned().unwrap_or_else(|| {
                    let mut base = self.stylesheet_root_vars.clone();
                    for (k, v) in &self.programmatic_vars {
                        base.insert(k.clone(), v.clone());
                    }
                    base
                })
            } else {
                self.stylesheet_root_vars.clone()
            };

            let matched_rules = self.match_node_rules(dom, node_id);
            for (_, _, decls) in &matched_rules {
                for (k, v) in decls {
                    if k.starts_with("--") || k.starts_with("-wm-") {
                        node_raw_vars.insert(k.clone(), v.clone());
                    }
                }
            }

            if node_id == root_id || parent_id.is_none() {
                for (k, v) in &self.programmatic_vars {
                    node_raw_vars.insert(k.clone(), v.clone());
                }
            }

            // Collect inline declarations (from `style` attr if present, then `node.inline_styles`)
            let inline_decls = extract_inline_declarations(node);
            for (k, v) in &inline_decls {
                if k.starts_with("--") || k.starts_with("-wm-") {
                    node_raw_vars.insert(k.clone(), v.clone());
                }
            }

            let resolved_vars = resolve_all_vars(&node_raw_vars);
            computed.custom_properties = resolved_vars.clone();

            // 3. First pass for font-size on this node so `em` and `rem` units resolve accurately
            for (_, _, decls) in &matched_rules {
                for (prop, val) in decls {
                    if prop == "font-size" {
                        let resolved = resolve_var_references(val, &resolved_vars, 0);
                        if let Some(fs) =
                            parse_length_px(&resolved, parent_font_size, root_font_size)
                        {
                            if fs > 0.0 {
                                computed.font_size = fs;
                            }
                        }
                    }
                }
            }
            for (prop, val) in &inline_decls {
                if prop == "font-size" {
                    let resolved = resolve_var_references(val, &resolved_vars, 0);
                    if let Some(fs) = parse_length_px(&resolved, parent_font_size, root_font_size) {
                        if fs > 0.0 {
                            computed.font_size = fs;
                        }
                    }
                }
            }

            if node_id == root_id {
                root_font_size = computed.font_size;
            }

            // 4. Apply all matched rule declarations in cascade order
            let current_font_size = computed.font_size;
            for (_, _, decls) in &matched_rules {
                for (prop, val) in decls {
                    if prop.starts_with("--") || prop.starts_with("-wm-") || prop == "font-size" {
                        continue;
                    }
                    let resolved = resolve_var_references(val, &resolved_vars, 0);
                    apply_declaration(
                        &mut computed,
                        prop,
                        &resolved,
                        current_font_size,
                        root_font_size,
                    );
                }
            }

            // 5. Apply inline declarations on top
            for (prop, val) in &inline_decls {
                if prop.starts_with("--") || prop.starts_with("-wm-") || prop == "font-size" {
                    continue;
                }
                let resolved = resolve_var_references(val, &resolved_vars, 0);
                apply_declaration(
                    &mut computed,
                    prop,
                    &resolved,
                    current_font_size,
                    root_font_size,
                );
            }

            // 6. Visibility override
            if !node.visible {
                computed.display = Display::None;
            }

            raw_vars_per_node.insert(node_id, node_raw_vars);
            resolved_vars_per_node.insert(node_id, resolved_vars);
            styles.insert(node_id, computed);
        }

        (styles, resolved_vars_per_node)
    }
}

impl StyleEngine for CssStyleEngine {
    fn load_stylesheet(&mut self, css_source: &str) -> Result<(), String> {
        self.rules.clear();
        self.next_rule_order = 0;
        let parsed = parse_stylesheet(css_source, &mut self.next_rule_order)?;
        self.stylesheet_root_vars = Self::collect_root_vars_from_rules(&parsed);
        self.rules = parsed;
        Ok(())
    }

    fn append_stylesheet(&mut self, css_source: &str) -> Result<(), String> {
        let mut parsed = parse_stylesheet(css_source, &mut self.next_rule_order)?;
        self.rules.append(&mut parsed);
        self.stylesheet_root_vars = Self::collect_root_vars_from_rules(&self.rules);
        Ok(())
    }

    fn set_custom_property(&mut self, name: &str, value: &str) {
        let key = Self::normalize_var_name(name);
        self.programmatic_vars
            .insert(key, value.trim().to_string());
    }

    fn get_custom_property(&self, name: &str) -> Option<&str> {
        let key = Self::normalize_var_name(name);
        self.programmatic_vars
            .get(&key)
            .or_else(|| self.stylesheet_root_vars.get(&key))
            .map(|s| s.as_str())
    }

    fn compute_styles(&self, dom: &dyn DomTreeView) -> HashMap<NodeId, ComputedStyle> {
        self.compute_all_internal(dom).0
    }

    fn inspect_node_style(&self, dom: &dyn DomTreeView, node_id: NodeId) -> StyleInspectionReport {
        let (mut styles, mut resolved_vars_map) = self.compute_all_internal(dom);
        let matched = self.match_node_rules(dom, node_id);
        let resolved_variables = resolved_vars_map.remove(&node_id).unwrap_or_else(|| {
            let mut base = self.stylesheet_root_vars.clone();
            for (k, v) in &self.programmatic_vars {
                base.insert(k.clone(), v.clone());
            }
            resolve_all_vars(&base)
        });

        let matched_rules = matched
            .into_iter()
            .map(|(sel, _, decls)| {
                let mut decl_map = BTreeMap::new();
                for (k, v) in decls {
                    decl_map.insert(k, v);
                }
                MatchedRuleReport {
                    selector: sel.raw,
                    specificity: sel.specificity,
                    declarations: decl_map,
                }
            })
            .collect();

        let computed = styles.remove(&node_id).unwrap_or_default();
        if let Some(node) = dom.get_node(node_id) {
            StyleInspectionReport {
                node_id: node_id.index(),
                tag: node.tag.clone(),
                id: node
                    .element_id
                    .clone()
                    .or_else(|| node.get_attr("id").map(|s| s.to_string())),
                classes: node.classes.clone(),
                matched_rules,
                resolved_variables,
                computed,
            }
        } else {
            StyleInspectionReport {
                node_id: node_id.index(),
                tag: String::new(),
                id: None,
                classes: Vec::new(),
                matched_rules,
                resolved_variables,
                computed,
            }
        }
    }

    fn query_selector_all(&self, dom: &dyn DomTreeView, selector: &str) -> Vec<NodeId> {
        let Ok(selectors) = parse_selector_list(selector) else {
            return Vec::new();
        };
        if selectors.is_empty() {
            return Vec::new();
        }
        let order = self.document_order_nodes(dom);
        order
            .into_iter()
            .filter(|&node_id| selectors.iter().any(|s| s.matches(dom, node_id)))
            .collect()
    }
}

// ============================================================================
// CSS Comment Stripping, Rule & Selector Parsing
// ============================================================================

pub fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let chars: Vec<char> = css.chars().collect();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && (in_single_quote || in_double_quote) && i + 1 < chars.len() {
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            out.push(c);
            i += 1;
            continue;
        }
        if !in_single_quote && !in_double_quote && c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < chars.len() {
                i += 2;
            } else {
                break;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }

    out
}

fn parse_stylesheet(css_source: &str, next_order: &mut usize) -> Result<Vec<CssRule>, String> {
    let cleaned = strip_comments(css_source);
    let mut rules = Vec::new();
    parse_rules_recursive(&cleaned, next_order, &mut rules)?;
    Ok(rules)
}

fn parse_rules_recursive(
    s: &str,
    next_order: &mut usize,
    rules: &mut Vec<CssRule>,
) -> Result<(), String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        // Handle @-rules gracefully
        if chars[i] == '@' {
            let start = i;
            while i < chars.len() && chars[i] != '{' && chars[i] != ';' {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }
            if chars[i] == ';' {
                i += 1;
                continue;
            }
            let header: String = chars[start..i].iter().collect();
            i += 1; // skip '{'
            let block_start = i;
            let mut depth = 1usize;
            while i < chars.len() && depth > 0 {
                if chars[i] == '{' {
                    depth += 1;
                } else if chars[i] == '}' {
                    depth -= 1;
                }
                i += 1;
            }
            let block_end = if depth == 0 { i - 1 } else { i };
            let inner: String = chars[block_start..block_end].iter().collect();
            let header_lower = header.trim().to_ascii_lowercase();
            if header_lower.starts_with("@media")
                || header_lower.starts_with("@supports")
                || header_lower.starts_with("@layer")
            {
                parse_rules_recursive(&inner, next_order, rules)?;
            }
            continue;
        }

        // Read selector list up to '{'
        let sel_start = i;
        let mut in_quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            if let Some(q) = in_quote {
                if c == q {
                    in_quote = None;
                }
            } else if c == '"' || c == '\'' {
                in_quote = Some(c);
            } else if c == '{' {
                break;
            }
            i += 1;
        }

        if i >= chars.len() {
            break;
        }

        let selector_str: String = chars[sel_start..i].iter().collect();
        i += 1; // skip '{'

        let body_start = i;
        let mut depth = 1usize;
        let mut in_quote: Option<char> = None;
        while i < chars.len() && depth > 0 {
            let c = chars[i];
            if let Some(q) = in_quote {
                if c == q {
                    in_quote = None;
                }
            } else if c == '"' || c == '\'' {
                in_quote = Some(c);
            } else if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
            }
            i += 1;
        }

        let body_end = if depth == 0 { i - 1 } else { i };
        let body_str: String = chars[body_start..body_end].iter().collect();

        let selectors = parse_selector_list(&selector_str)?;
        let declarations = parse_declarations(&body_str);

        if !selectors.is_empty() {
            let order = *next_order;
            *next_order += 1;
            rules.push(CssRule {
                selectors,
                declarations,
                order,
            });
        }
    }

    Ok(())
}

pub fn parse_selector_list(input: &str) -> Result<Vec<Selector>, String> {
    let parts = split_top_level(input, ',');
    let mut selectors = Vec::new();
    for part in parts {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            selectors.push(Selector::parse(trimmed)?);
        }
    }
    Ok(selectors)
}

fn tokenize_selector_chain(input: &str) -> Result<Vec<(Option<Combinator>, String)>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut pending_combinator: Option<Combinator> = None;

    let chars: Vec<char> = input.trim().chars().collect();
    let mut i = 0;
    let mut bracket_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut in_quote: Option<char> = None;

    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_quote {
            if c == q {
                in_quote = None;
            }
            current.push(c);
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            in_quote = Some(c);
            current.push(c);
            i += 1;
            continue;
        }
        if c == '[' {
            bracket_depth += 1;
            current.push(c);
            i += 1;
            continue;
        }
        if c == ']' {
            bracket_depth = bracket_depth.saturating_sub(1);
            current.push(c);
            i += 1;
            continue;
        }
        if c == '(' {
            paren_depth += 1;
            current.push(c);
            i += 1;
            continue;
        }
        if c == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            current.push(c);
            i += 1;
            continue;
        }

        if bracket_depth == 0 && paren_depth == 0 {
            if c == '>' {
                if !current.trim().is_empty() {
                    out.push((pending_combinator.take(), current.trim().to_string()));
                    current.clear();
                }
                pending_combinator = Some(Combinator::Child);
                i += 1;
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                continue;
            } else if c.is_whitespace() {
                if !current.trim().is_empty() {
                    out.push((pending_combinator.take(), current.trim().to_string()));
                    current.clear();
                }
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                if i < chars.len() && chars[i] == '>' {
                    pending_combinator = Some(Combinator::Child);
                    i += 1;
                    while i < chars.len() && chars[i].is_whitespace() {
                        i += 1;
                    }
                } else if !out.is_empty() && pending_combinator.is_none() {
                    pending_combinator = Some(Combinator::Descendant);
                }
                continue;
            }
        }

        current.push(c);
        i += 1;
    }

    if !current.trim().is_empty() {
        out.push((pending_combinator.take(), current.trim().to_string()));
    }

    Ok(out)
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

fn parse_compound_selector(input: &str) -> Result<CompoundSelector, String> {
    let mut comp = CompoundSelector::default();
    let chars: Vec<char> = input.trim().chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' => {
                comp.universal = true;
                i += 1;
            }
            '#' => {
                i += 1;
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                let id: String = chars[start..i].iter().collect();
                if id.is_empty() {
                    return Err(format!("Empty ID selector in '{input}'"));
                }
                comp.id = Some(id);
            }
            '.' => {
                i += 1;
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                let cls: String = chars[start..i].iter().collect();
                if cls.is_empty() {
                    return Err(format!("Empty class selector in '{input}'"));
                }
                comp.classes.push(cls);
            }
            '[' => {
                i += 1;
                let start = i;
                let mut in_quote: Option<char> = None;
                while i < chars.len() {
                    let ch = chars[i];
                    if let Some(q) = in_quote {
                        if ch == q {
                            in_quote = None;
                        }
                    } else if ch == '"' || ch == '\'' {
                        in_quote = Some(ch);
                    } else if ch == ']' {
                        break;
                    }
                    i += 1;
                }
                let attr_content: String = chars[start..i].iter().collect();
                if i < chars.len() && chars[i] == ']' {
                    i += 1;
                }
                comp.attributes.push(parse_attribute_selector(&attr_content)?);
            }
            ':' => {
                i += 1;
                if i < chars.len() && chars[i] == ':' {
                    i += 1;
                }
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                let mut arg: Option<String> = None;
                if i < chars.len() && chars[i] == '(' {
                    i += 1;
                    let arg_start = i;
                    let mut depth = 1usize;
                    while i < chars.len() && depth > 0 {
                        if chars[i] == '(' {
                            depth += 1;
                        } else if chars[i] == ')' {
                            depth -= 1;
                        }
                        i += 1;
                    }
                    let arg_end = if depth == 0 { i - 1 } else { i };
                    arg = Some(chars[arg_start..arg_end].iter().collect());
                }
                comp.pseudo_classes
                    .push(parse_pseudo_class(&name, arg.as_deref())?);
            }
            _ if is_ident_char(c) => {
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                let tag: String = chars[start..i].iter().collect();
                comp.tag = Some(tag.to_ascii_lowercase());
            }
            _ => {
                return Err(format!("Unexpected character '{c}' in selector '{input}'"));
            }
        }
    }

    if comp.is_empty() {
        return Err(format!("Invalid empty compound selector '{input}'"));
    }

    Ok(comp)
}

fn parse_attribute_selector(content: &str) -> Result<AttributeSelector, String> {
    let s = content.trim();
    if s.is_empty() {
        return Err("Empty attribute selector".to_string());
    }

    let ops = ["~=", "|=", "^=", "$=", "*=", "="];
    for op_str in ops {
        if let Some(pos) = s.find(op_str) {
            let name = s[..pos].trim().to_string();
            let raw_val = s[pos + op_str.len()..].trim();
            let val = unquote(raw_val).to_string();
            let op = match op_str {
                "~=" => AttributeOp::Includes(val),
                "|=" => AttributeOp::DashMatch(val),
                "^=" => AttributeOp::Prefix(val),
                "$=" => AttributeOp::Suffix(val),
                "*=" => AttributeOp::Substring(val),
                "=" => AttributeOp::Equals(val),
                _ => unreachable!(),
            };
            return Ok(AttributeSelector { name, op });
        }
    }

    Ok(AttributeSelector {
        name: s.to_string(),
        op: AttributeOp::Exists,
    })
}

fn parse_pseudo_class(name: &str, arg: Option<&str>) -> Result<PseudoClass, String> {
    match name.to_ascii_lowercase().as_str() {
        "root" => Ok(PseudoClass::Root),
        "hover" => Ok(PseudoClass::Hover),
        "active" => Ok(PseudoClass::Active),
        "focus" => Ok(PseudoClass::Focus),
        "focus-visible" => Ok(PseudoClass::FocusVisible),
        "checked" => Ok(PseudoClass::Checked),
        "disabled" => Ok(PseudoClass::Disabled),
        "urgent" => Ok(PseudoClass::Urgent),
        "first-child" => Ok(PseudoClass::FirstChild),
        "last-child" => Ok(PseudoClass::LastChild),
        "nth-child" => {
            let n = arg
                .unwrap_or("1")
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("Invalid :nth-child argument: {:?}", arg))?;
            Ok(PseudoClass::NthChild(n))
        }
        other => Err(format!("Unsupported pseudo-class ':{other}'")),
    }
}

pub fn parse_declarations(body: &str) -> Vec<(String, String)> {
    let mut decls = Vec::new();
    for item in split_top_level(body, ';') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(colon_idx) = trimmed.find(':') else {
            continue;
        };
        let raw_prop = trimmed[..colon_idx].trim();
        let mut raw_val = trimmed[colon_idx + 1..].trim();

        // Strip trailing !important if present
        if raw_val.to_ascii_lowercase().ends_with("!important") {
            raw_val = raw_val[..raw_val.len() - "!important".len()].trim();
        }

        if raw_prop.is_empty() || raw_val.is_empty() {
            continue;
        }

        let prop = if raw_prop.starts_with("--") {
            raw_prop.to_string()
        } else {
            raw_prop.to_ascii_lowercase()
        };

        decls.push((prop, raw_val.to_string()));
    }
    decls
}

fn extract_inline_declarations(node: &DomNodeData) -> Vec<(String, String)> {
    let mut decls = Vec::new();
    if let Some(style_attr) = node.get_attr("style") {
        decls.extend(parse_declarations(style_attr));
    }

    // Order `node.inline_styles` so shorthands are applied before specific longhands
    let mut shorthands = Vec::new();
    let mut longhands = Vec::new();
    for (k, v) in &node.inline_styles {
        let prop = if k.starts_with("--") {
            k.trim().to_string()
        } else {
            k.trim().to_ascii_lowercase()
        };
        let mut val = v.trim();
        if val.to_ascii_lowercase().ends_with("!important") {
            val = val[..val.len() - "!important".len()].trim();
        }
        if is_shorthand_property(&prop) {
            shorthands.push((prop, val.to_string()));
        } else {
            longhands.push((prop, val.to_string()));
        }
    }
    decls.extend(shorthands);
    decls.extend(longhands);
    decls
}

fn is_shorthand_property(prop: &str) -> bool {
    matches!(
        prop,
        "margin"
            | "padding"
            | "border"
            | "border-width"
            | "border-top"
            | "border-right"
            | "border-bottom"
            | "border-left"
            | "background"
            | "flex"
            | "gap"
            | "grid-gap"
            | "inset"
            | "border-radius"
    )
}

// ============================================================================
// CSS Custom Property (`var()`) Resolution
// ============================================================================

fn resolve_all_vars(raw_vars: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut resolved = BTreeMap::new();
    for (k, v) in raw_vars {
        let val = resolve_var_references(v, raw_vars, 0);
        resolved.insert(k.clone(), val);
    }
    resolved
}

pub fn resolve_var_references(
    input: &str,
    vars: &BTreeMap<String, String>,
    depth: usize,
) -> String {
    if depth > 16 || !input.contains("var(") {
        return input.trim().to_string();
    }

    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start_idx) = rest.find("var(") {
        result.push_str(&rest[..start_idx]);
        let after_var = &rest[start_idx + 4..];

        // Find matching closing ')'
        let mut paren_depth = 1usize;
        let mut end_idx = None;
        for (idx, ch) in after_var.char_indices() {
            if ch == '(' {
                paren_depth += 1;
            } else if ch == ')' {
                paren_depth -= 1;
                if paren_depth == 0 {
                    end_idx = Some(idx);
                    break;
                }
            }
        }

        if let Some(close_pos) = end_idx {
            let inner = &after_var[..close_pos];
            let (var_name, fallback_opt) = split_first_top_level_comma(inner);
            let var_key = var_name.trim();

            if let Some(found) = vars.get(var_key) {
                let resolved = resolve_var_references(found, vars, depth + 1);
                result.push_str(&resolved);
            } else if let Some(fallback) = fallback_opt {
                let resolved = resolve_var_references(fallback.trim(), vars, depth + 1);
                result.push_str(&resolved);
            }

            rest = &after_var[close_pos + 1..];
        } else {
            // Unclosed `var(`, append rest as-is
            result.push_str(&rest[start_idx..]);
            rest = "";
            break;
        }
    }

    result.push_str(rest);
    result.trim().to_string()
}

fn split_first_top_level_comma(s: &str) -> (&str, Option<&str>) {
    let mut paren_depth = 0usize;
    let mut in_quote: Option<char> = None;
    for (idx, ch) in s.char_indices() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            continue;
        }
        if ch == '(' {
            paren_depth += 1;
        } else if ch == ')' {
            paren_depth = paren_depth.saturating_sub(1);
        } else if ch == ',' && paren_depth == 0 {
            return (&s[..idx], Some(&s[idx + 1..]));
        }
    }
    (s, None)
}

fn split_top_level(s: &str, delimiter: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut in_quote: Option<char> = None;

    for ch in s.chars() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            current.push(ch);
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            current.push(ch);
            continue;
        }
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            c if c == delimiter && paren_depth == 0 && bracket_depth == 0 => {
                out.push(current.clone());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }

    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn split_whitespace_preserving_parens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    let mut in_quote: Option<char> = None;

    for ch in s.chars() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            current.push(ch);
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            current.push(ch);
            continue;
        }
        if ch == '(' {
            paren_depth += 1;
            current.push(ch);
        } else if ch == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            current.push(ch);
        } else if ch.is_whitespace() && paren_depth == 0 {
            if !current.is_empty() {
                out.push(current.clone());
                current.clear();
            }
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn unquote(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

// ============================================================================
// CSS Value & Property Parsers
// ============================================================================

pub fn parse_color(input: &str) -> Option<Color> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if s.eq_ignore_ascii_case("transparent") || s.eq_ignore_ascii_case("none") {
        return Some(Color::TRANSPARENT);
    }
    let parsed = csscolorparser::parse(s).ok()?;
    let [r, g, b, a] = parsed.to_rgba8();
    Some(Color::rgba(r, g, b, a))
}

fn parse_length_px(input: &str, font_size: f32, root_font_size: f32) -> Option<f32> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if s == "0" || s == "0px" || s == "0.0" {
        return Some(0.0);
    }
    if let Some(num) = s.strip_suffix("rem") {
        return num.trim().parse::<f32>().ok().map(|v| v * root_font_size);
    }
    if let Some(num) = s.strip_suffix("em") {
        return num.trim().parse::<f32>().ok().map(|v| v * font_size);
    }
    if let Some(num) = s.strip_suffix("px") {
        return num.trim().parse::<f32>().ok();
    }
    if let Some(num) = s.strip_suffix("pt") {
        return num.trim().parse::<f32>().ok().map(|v| v * (4.0 / 3.0));
    }
    if let Some(num) = s.strip_suffix('%') {
        return num
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| (v / 100.0) * font_size);
    }
    s.parse::<f32>().ok()
}

fn parse_dimension(input: &str, font_size: f32, root_font_size: f32) -> Option<Dimension> {
    let s = input.trim().to_ascii_lowercase();
    if s == "auto" || s == "none" || s == "initial" || s == "unset" {
        return Some(Dimension::Auto);
    }
    if let Some(pct) = s.strip_suffix('%') {
        return pct.trim().parse::<f32>().ok().map(Dimension::Percent);
    }
    parse_length_px(&s, font_size, root_font_size).map(Dimension::Px)
}

fn parse_edges_shorthand(input: &str, font_size: f32, root_font_size: f32) -> Option<Edges> {
    let tokens = split_whitespace_preserving_parens(input);
    let mut vals = Vec::with_capacity(tokens.len());
    for t in tokens {
        if t.eq_ignore_ascii_case("auto") {
            vals.push(0.0);
        } else {
            vals.push(parse_length_px(&t, font_size, root_font_size)?);
        }
    }
    match vals.as_slice() {
        [all] => Some(Edges::all(*all)),
        [v, h] => Some(Edges::xy(*h, *v)),
        [t, h, b] => Some(Edges {
            top: *t,
            right: *h,
            bottom: *b,
            left: *h,
        }),
        [t, r, b, l, ..] => Some(Edges {
            top: *t,
            right: *r,
            bottom: *b,
            left: *l,
        }),
        [] => None,
    }
}

fn parse_border_radius(input: &str, font_size: f32, root_font_size: f32) -> Option<[f32; 4]> {
    let tokens = split_whitespace_preserving_parens(input);
    let mut vals = Vec::with_capacity(tokens.len());
    for t in tokens {
        vals.push(parse_length_px(&t, font_size, root_font_size)?);
    }
    match vals.as_slice() {
        [r] => Some([*r, *r, *r, *r]),
        [tl_br, tr_bl] => Some([*tl_br, *tr_bl, *tl_br, *tr_bl]),
        [tl, tr_bl, br] => Some([*tl, *tr_bl, *br, *tr_bl]),
        [tl, tr, br, bl, ..] => Some([*tl, *tr, *br, *bl]),
        [] => None,
    }
}

fn parse_border_shorthand(
    input: &str,
    font_size: f32,
    root_font_size: f32,
) -> (Option<f32>, Option<Color>) {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("none") || trimmed == "0" || trimmed == "0px" {
        return (Some(0.0), None);
    }
    let tokens = split_whitespace_preserving_parens(trimmed);
    let mut width: Option<f32> = None;
    let mut color: Option<Color> = None;

    for token in tokens {
        let lower = token.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "solid"
                | "dashed"
                | "dotted"
                | "double"
                | "groove"
                | "ridge"
                | "inset"
                | "outset"
        ) {
            continue;
        }
        if lower == "none" || lower == "hidden" {
            width = Some(0.0);
            continue;
        }
        if width.is_none() {
            if let Some(w) = parse_length_px(&token, font_size, root_font_size) {
                width = Some(w);
                continue;
            }
        }
        if let Some(c) = parse_color(&token) {
            color = Some(c);
        }
    }

    (width, color)
}

pub fn parse_linear_gradient(input: &str) -> Option<LinearGradient> {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    let start_idx = lower.find("linear-gradient(")?;
    let after = &trimmed[start_idx + "linear-gradient(".len()..];

    let mut paren_depth = 1usize;
    let mut end_idx = None;
    for (idx, ch) in after.char_indices() {
        if ch == '(' {
            paren_depth += 1;
        } else if ch == ')' {
            paren_depth -= 1;
            if paren_depth == 0 {
                end_idx = Some(idx);
                break;
            }
        }
    }
    let inner = &after[..end_idx?];
    let args = split_top_level(inner, ',');
    if args.is_empty() {
        return None;
    }

    let mut angle_deg = 180.0f32;
    let mut stop_start_idx = 0usize;

    if let Some(parsed_angle) = parse_gradient_angle_or_direction(args[0].trim()) {
        angle_deg = parsed_angle;
        stop_start_idx = 1;
    }

    let stop_args = &args[stop_start_idx..];
    if stop_args.is_empty() {
        return None;
    }

    let mut raw_stops: Vec<(Color, Option<f32>)> = Vec::with_capacity(stop_args.len());
    for raw_stop in stop_args {
        let parts = split_whitespace_preserving_parens(raw_stop.trim());
        if parts.is_empty() {
            continue;
        }
        let color = parse_color(&parts[0])?;
        let offset = if parts.len() >= 2 {
            let pos_str = parts[1].trim();
            if let Some(pct) = pos_str.strip_suffix('%') {
                pct.trim()
                    .parse::<f32>()
                    .ok()
                    .map(|v| (v / 100.0).clamp(0.0, 1.0))
            } else {
                pos_str.parse::<f32>().ok().map(|v| {
                    if v > 1.0 {
                        (v / 100.0).clamp(0.0, 1.0)
                    } else {
                        v.clamp(0.0, 1.0)
                    }
                })
            }
        } else {
            None
        };
        raw_stops.push((color, offset));
    }

    if raw_stops.is_empty() {
        return None;
    }

    let total = raw_stops.len();
    let stops = raw_stops
        .into_iter()
        .enumerate()
        .map(|(i, (color, offset_opt))| {
            let offset = offset_opt.unwrap_or_else(|| {
                if total <= 1 {
                    0.0
                } else {
                    i as f32 / (total - 1) as f32
                }
            });
            GradientStop { offset, color }
        })
        .collect();

    Some(LinearGradient { angle_deg, stops })
}

fn parse_gradient_angle_or_direction(s: &str) -> Option<f32> {
    let lower = s.trim().to_ascii_lowercase();
    if let Some(deg) = lower.strip_suffix("deg") {
        return deg.trim().parse::<f32>().ok();
    }
    if let Some(turn) = lower.strip_suffix("turn") {
        return turn.trim().parse::<f32>().ok().map(|v| v * 360.0);
    }
    if let Some(grad) = lower.strip_suffix("grad") {
        return grad.trim().parse::<f32>().ok().map(|v| v * 0.9);
    }
    if let Some(rad) = lower.strip_suffix("rad") {
        return rad
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| v.to_degrees());
    }
    if lower.starts_with("to ") {
        let dir = lower["to ".len()..].split_whitespace().collect::<Vec<_>>();
        return match dir.as_slice() {
            ["top"] => Some(0.0),
            ["right"] => Some(90.0),
            ["bottom"] => Some(180.0),
            ["left"] => Some(270.0),
            ["top", "right"] | ["right", "top"] => Some(45.0),
            ["bottom", "right"] | ["right", "bottom"] => Some(135.0),
            ["bottom", "left"] | ["left", "bottom"] => Some(225.0),
            ["top", "left"] | ["left", "top"] => Some(315.0),
            _ => None,
        };
    }
    None
}

pub fn parse_box_shadow(input: &str, font_size: f32, root_font_size: f32) -> Option<BoxShadow> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("none") || trimmed.is_empty() {
        return None;
    }
    // Use first shadow if multiple comma-separated shadows are specified
    let first_shadow = split_top_level(trimmed, ',')
        .into_iter()
        .next()
        .unwrap_or_else(|| trimmed.to_string());
    let tokens = split_whitespace_preserving_parens(&first_shadow);

    let mut inset = false;
    let mut lengths: Vec<f32> = Vec::new();
    let mut color: Option<Color> = None;

    for token in tokens {
        if token.eq_ignore_ascii_case("inset") {
            inset = true;
        } else if let Some(len) = parse_length_px(&token, font_size, root_font_size) {
            lengths.push(len);
        } else if let Some(c) = parse_color(&token) {
            color = Some(c);
        }
    }

    if lengths.len() < 2 {
        return None;
    }

    Some(BoxShadow {
        offset_x: lengths[0],
        offset_y: lengths[1],
        blur_radius: lengths.get(2).copied().unwrap_or(0.0),
        spread_radius: lengths.get(3).copied().unwrap_or(0.0),
        color: color.unwrap_or(Color::rgba(0, 0, 0, 128)),
        inset,
    })
}

fn parse_grid_columns(input: &str) -> Option<u16> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() || s == "none" {
        return Some(0);
    }
    if let Some(start) = s.find("repeat(") {
        let after = &s[start + "repeat(".len()..];
        if let Some(comma) = after.find(',') {
            if let Ok(count) = after[..comma].trim().parse::<u16>() {
                return Some(count);
            }
        }
    }
    if let Ok(n) = s.parse::<u16>() {
        return Some(n);
    }
    let tracks = split_whitespace_preserving_parens(&s);
    if !tracks.is_empty() {
        return Some(tracks.len() as u16);
    }
    None
}

fn parse_backdrop_blur(input: &str, font_size: f32, root_font_size: f32) -> Option<f32> {
    let s = input.trim().to_ascii_lowercase();
    if s == "none" {
        return Some(0.0);
    }
    if let Some(start) = s.find("blur(") {
        let after = &s[start + "blur(".len()..];
        let end = after.find(')')?;
        let inner = after[..end].trim();
        return parse_length_px(inner, font_size, root_font_size);
    }
    parse_length_px(&s, font_size, root_font_size)
}

fn apply_declaration(
    style: &mut ComputedStyle,
    prop: &str,
    val: &str,
    font_size: f32,
    root_font_size: f32,
) {
    let v = val.trim();
    if v.is_empty() {
        return;
    }
    let lower = v.to_ascii_lowercase();

    match prop {
        "display" => {
            style.display = match lower.as_str() {
                "flex" | "inline-flex" => Display::Flex,
                "grid" | "inline-grid" => Display::Grid,
                "block" | "inline-block" | "inline" => Display::Block,
                "none" => Display::None,
                _ => style.display,
            };
        }
        "position" => {
            style.position = match lower.as_str() {
                "relative" | "static" | "sticky" => Position::Relative,
                "absolute" | "fixed" => Position::Absolute,
                _ => style.position,
            };
        }
        "flex-direction" => {
            style.flex_direction = match lower.as_str() {
                "row" => FlexDirection::Row,
                "column" => FlexDirection::Column,
                "row-reverse" => FlexDirection::RowReverse,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => style.flex_direction,
            };
        }
        "flex-wrap" => {
            style.flex_wrap = match lower.as_str() {
                "nowrap" | "no-wrap" => FlexWrap::NoWrap,
                "wrap" | "wrap-reverse" => FlexWrap::Wrap,
                _ => style.flex_wrap,
            };
        }
        "flex-grow" => {
            if let Ok(g) = lower.parse::<f32>() {
                style.flex_grow = g;
            }
        }
        "flex-shrink" => {
            if let Ok(s) = lower.parse::<f32>() {
                style.flex_shrink = s;
            }
        }
        "flex" => {
            if lower == "none" {
                style.flex_grow = 0.0;
                style.flex_shrink = 0.0;
            } else if lower == "auto" {
                style.flex_grow = 1.0;
                style.flex_shrink = 1.0;
            } else if lower == "initial" {
                style.flex_grow = 0.0;
                style.flex_shrink = 1.0;
            } else {
                let parts = split_whitespace_preserving_parens(&lower);
                if let Some(first) = parts.first() {
                    if let Ok(g) = first.parse::<f32>() {
                        style.flex_grow = g;
                        if let Some(second) = parts.get(1) {
                            if let Ok(s) = second.parse::<f32>() {
                                style.flex_shrink = s;
                            }
                        } else {
                            style.flex_shrink = 1.0;
                        }
                    }
                }
            }
        }
        "justify-content" => {
            style.justify_content = match lower.as_str() {
                "flex-start" | "start" | "left" => JustifyContent::FlexStart,
                "flex-end" | "end" | "right" => JustifyContent::FlexEnd,
                "center" => JustifyContent::Center,
                "space-between" => JustifyContent::SpaceBetween,
                "space-around" => JustifyContent::SpaceAround,
                "space-evenly" => JustifyContent::SpaceEvenly,
                _ => style.justify_content,
            };
        }
        "align-items" => {
            style.align_items = match lower.as_str() {
                "stretch" => AlignItems::Stretch,
                "flex-start" | "start" | "self-start" => AlignItems::FlexStart,
                "flex-end" | "end" | "self-end" => AlignItems::FlexEnd,
                "center" => AlignItems::Center,
                "baseline" => AlignItems::Baseline,
                _ => style.align_items,
            };
        }
        "gap" | "grid-gap" => {
            let parts = split_whitespace_preserving_parens(&lower);
            if parts.len() == 1 {
                if let Some(g) = parse_length_px(&parts[0], font_size, root_font_size) {
                    style.gap_row = g;
                    style.gap_column = g;
                }
            } else if parts.len() >= 2 {
                if let Some(r) = parse_length_px(&parts[0], font_size, root_font_size) {
                    style.gap_row = r;
                }
                if let Some(c) = parse_length_px(&parts[1], font_size, root_font_size) {
                    style.gap_column = c;
                }
            }
        }
        "row-gap" | "grid-row-gap" => {
            if let Some(r) = parse_length_px(&lower, font_size, root_font_size) {
                style.gap_row = r;
            }
        }
        "column-gap" | "grid-column-gap" => {
            if let Some(c) = parse_length_px(&lower, font_size, root_font_size) {
                style.gap_column = c;
            }
        }
        "grid-template-columns" => {
            if let Some(cols) = parse_grid_columns(&lower) {
                style.grid_columns = cols;
            }
        }
        "width" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.width = d;
            }
        }
        "height" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.height = d;
            }
        }
        "min-width" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.min_width = d;
            }
        }
        "min-height" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.min_height = d;
            }
        }
        "max-width" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.max_width = d;
            }
        }
        "max-height" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.max_height = d;
            }
        }
        "padding" => {
            if let Some(edges) = parse_edges_shorthand(&lower, font_size, root_font_size) {
                style.padding = edges;
            }
        }
        "padding-top" | "padding-block-start" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.padding.top = px;
            }
        }
        "padding-right" | "padding-inline-end" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.padding.right = px;
            }
        }
        "padding-bottom" | "padding-block-end" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.padding.bottom = px;
            }
        }
        "padding-left" | "padding-inline-start" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.padding.left = px;
            }
        }
        "padding-inline" => {
            if let Some(edges) = parse_edges_shorthand(&lower, font_size, root_font_size) {
                style.padding.left = edges.top;
                style.padding.right = edges.right;
            }
        }
        "padding-block" => {
            if let Some(edges) = parse_edges_shorthand(&lower, font_size, root_font_size) {
                style.padding.top = edges.top;
                style.padding.bottom = edges.right;
            }
        }
        "margin" => {
            if let Some(edges) = parse_edges_shorthand(&lower, font_size, root_font_size) {
                style.margin = edges;
            }
        }
        "margin-top" | "margin-block-start" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.margin.top = px;
            }
        }
        "margin-right" | "margin-inline-end" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.margin.right = px;
            }
        }
        "margin-bottom" | "margin-block-end" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.margin.bottom = px;
            }
        }
        "margin-left" | "margin-inline-start" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.margin.left = px;
            }
        }
        "border-width" => {
            if let Some(edges) = parse_edges_shorthand(&lower, font_size, root_font_size) {
                style.border_width = edges;
            }
        }
        "border-top-width" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_width.top = px;
            }
        }
        "border-right-width" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_width.right = px;
            }
        }
        "border-bottom-width" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_width.bottom = px;
            }
        }
        "border-left-width" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_width.left = px;
            }
        }
        "border" => {
            let (w_opt, c_opt) = parse_border_shorthand(v, font_size, root_font_size);
            if let Some(w) = w_opt {
                style.border_width = Edges::all(w);
            }
            if let Some(c) = c_opt {
                style.border_color = c;
            }
        }
        "border-top" => {
            let (w_opt, c_opt) = parse_border_shorthand(v, font_size, root_font_size);
            if let Some(w) = w_opt {
                style.border_width.top = w;
            }
            if let Some(c) = c_opt {
                style.border_color = c;
            }
        }
        "border-right" => {
            let (w_opt, c_opt) = parse_border_shorthand(v, font_size, root_font_size);
            if let Some(w) = w_opt {
                style.border_width.right = w;
            }
            if let Some(c) = c_opt {
                style.border_color = c;
            }
        }
        "border-bottom" => {
            let (w_opt, c_opt) = parse_border_shorthand(v, font_size, root_font_size);
            if let Some(w) = w_opt {
                style.border_width.bottom = w;
            }
            if let Some(c) = c_opt {
                style.border_color = c;
            }
        }
        "border-left" => {
            let (w_opt, c_opt) = parse_border_shorthand(v, font_size, root_font_size);
            if let Some(w) = w_opt {
                style.border_width.left = w;
            }
            if let Some(c) = c_opt {
                style.border_color = c;
            }
        }
        "border-color" => {
            if let Some(c) = parse_color(v) {
                style.border_color = c;
            }
        }
        "border-radius" => {
            if let Some(r) = parse_border_radius(&lower, font_size, root_font_size) {
                style.border_radius = r;
            }
        }
        "border-top-left-radius" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_radius[0] = px;
            }
        }
        "border-top-right-radius" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_radius[1] = px;
            }
        }
        "border-bottom-right-radius" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_radius[2] = px;
            }
        }
        "border-bottom-left-radius" => {
            if let Some(px) = parse_length_px(&lower, font_size, root_font_size) {
                style.border_radius[3] = px;
            }
        }
        "top" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.inset_top = d;
            }
        }
        "right" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.inset_right = d;
            }
        }
        "bottom" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.inset_bottom = d;
            }
        }
        "left" => {
            if let Some(d) = parse_dimension(&lower, font_size, root_font_size) {
                style.inset_left = d;
            }
        }
        "inset" => {
            let tokens = split_whitespace_preserving_parens(&lower);
            let mut dims = Vec::with_capacity(tokens.len());
            for t in tokens {
                if let Some(d) = parse_dimension(&t, font_size, root_font_size) {
                    dims.push(d);
                }
            }
            match dims.as_slice() {
                [all] => {
                    style.inset_top = *all;
                    style.inset_right = *all;
                    style.inset_bottom = *all;
                    style.inset_left = *all;
                }
                [v, h] => {
                    style.inset_top = *v;
                    style.inset_right = *h;
                    style.inset_bottom = *v;
                    style.inset_left = *h;
                }
                [t, h, b] => {
                    style.inset_top = *t;
                    style.inset_right = *h;
                    style.inset_bottom = *b;
                    style.inset_left = *h;
                }
                [t, r, b, l, ..] => {
                    style.inset_top = *t;
                    style.inset_right = *r;
                    style.inset_bottom = *b;
                    style.inset_left = *l;
                }
                [] => {}
            }
        }
        "background" => {
            if lower.contains("linear-gradient(") {
                if let Some(grad) = parse_linear_gradient(v) {
                    style.background_gradient = Some(grad);
                }
            } else if lower == "none" || lower == "transparent" {
                style.background_color = Color::TRANSPARENT;
                style.background_gradient = None;
            } else if let Some(c) = parse_color(v) {
                style.background_color = c;
                style.background_gradient = None;
            }
        }
        "background-color" => {
            if let Some(c) = parse_color(v) {
                style.background_color = c;
            }
        }
        "background-image" => {
            if lower == "none" {
                style.background_gradient = None;
            } else if lower.contains("linear-gradient(") {
                if let Some(grad) = parse_linear_gradient(v) {
                    style.background_gradient = Some(grad);
                }
            }
        }
        "box-shadow" => {
            if lower == "none" {
                style.box_shadow = None;
            } else if let Some(shadow) = parse_box_shadow(v, font_size, root_font_size) {
                style.box_shadow = Some(shadow);
            }
        }
        "color" => {
            if let Some(c) = parse_color(v) {
                style.color = c;
            }
        }
        "font-family" => {
            let cleaned: String = v
                .chars()
                .filter(|&c| c != '"' && c != '\'')
                .collect::<String>()
                .trim()
                .to_string();
            if !cleaned.is_empty() {
                style.font_family = cleaned;
            }
        }
        "font-size" => {
            if let Some(fs) = parse_length_px(&lower, font_size, root_font_size) {
                if fs > 0.0 {
                    style.font_size = fs;
                }
            }
        }
        "font-weight" => {
            style.font_weight = match lower.as_str() {
                "thin" | "hairline" => 100,
                "extralight" | "extra-light" | "ultralight" => 200,
                "light" | "lighter" => 300,
                "normal" | "regular" => 400,
                "medium" => 500,
                "semibold" | "semi-bold" | "demibold" => 600,
                "bold" => 700,
                "extrabold" | "extra-bold" | "bolder" => 800,
                "black" | "heavy" => 900,
                other => other.parse::<u16>().unwrap_or(style.font_weight),
            };
        }
        "line-height" => {
            if lower == "normal" {
                style.line_height = 1.35;
            } else if let Some(pct) = lower.strip_suffix('%') {
                if let Ok(p) = pct.trim().parse::<f32>() {
                    style.line_height = p / 100.0;
                }
            } else if let Some(px) = lower.strip_suffix("px") {
                if let Ok(p) = px.trim().parse::<f32>() {
                    if style.font_size > 0.0 {
                        style.line_height = p / style.font_size;
                    }
                }
            } else if let Ok(mult) = lower.parse::<f32>() {
                if mult > 0.0 {
                    style.line_height = mult;
                }
            }
        }
        "text-align" => {
            style.text_align = match lower.as_str() {
                "left" | "start" => TextAlign::Left,
                "center" => TextAlign::Center,
                "right" | "end" => TextAlign::Right,
                _ => style.text_align,
            };
        }
        "opacity" => {
            if let Some(pct) = lower.strip_suffix('%') {
                if let Ok(p) = pct.trim().parse::<f32>() {
                    style.opacity = (p / 100.0).clamp(0.0, 1.0);
                }
            } else if let Ok(op) = lower.parse::<f32>() {
                style.opacity = op.clamp(0.0, 1.0);
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            style.overflow = match lower.as_str() {
                "visible" => Overflow::Visible,
                "hidden" | "clip" => Overflow::Hidden,
                "scroll" | "auto" => Overflow::Scroll,
                _ => style.overflow,
            };
        }
        "z-index" => {
            if lower == "auto" {
                style.z_index = 0;
            } else if let Ok(z) = lower.parse::<i32>() {
                style.z_index = z;
            }
        }
        "backdrop-filter" | "-webkit-backdrop-filter" => {
            if let Some(blur) = parse_backdrop_blur(&lower, font_size, root_font_size) {
                style.backdrop_blur = blur;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDom {
        root: NodeId,
        nodes: BTreeMap<NodeId, DomNodeData>,
    }

    impl MockDom {
        fn new(root: NodeId) -> Self {
            Self {
                root,
                nodes: BTreeMap::new(),
            }
        }

        fn add_node(&mut self, node: DomNodeData) {
            self.nodes.insert(node.id, node);
        }
    }

    impl DomTreeView for MockDom {
        fn root_id(&self) -> NodeId {
            self.root
        }

        fn get_node(&self, id: NodeId) -> Option<&DomNodeData> {
            self.nodes.get(&id)
        }

        fn all_node_ids(&self) -> Vec<NodeId> {
            self.nodes.keys().copied().collect()
        }

        fn children_of(&self, id: NodeId) -> &[NodeId] {
            self.nodes
                .get(&id)
                .map(|n| n.children.as_slice())
                .unwrap_or(&[])
        }

        fn parent_of(&self, id: NodeId) -> Option<NodeId> {
            self.nodes.get(&id).and_then(|n| n.parent)
        }
    }

    fn make_element(
        id: usize,
        tag: &str,
        element_id: Option<&str>,
        classes: &[&str],
        parent: Option<usize>,
        children: &[usize],
    ) -> DomNodeData {
        DomNodeData {
            id: NodeId(id),
            kind: DomNodeKind::Element,
            tag: tag.to_string(),
            element_id: element_id.map(|s| s.to_string()),
            classes: classes.iter().map(|s| s.to_string()).collect(),
            attributes: BTreeMap::new(),
            inline_styles: BTreeMap::new(),
            text_content: None,
            pseudo_states: PseudoStates::empty(),
            visible: true,
            parent: parent.map(NodeId),
            children: children.iter().copied().map(NodeId).collect(),
        }
    }

    #[test]
    fn test_specificity_and_selector_parsing() {
        let s1 = Selector::parse("*").unwrap();
        assert_eq!(s1.specificity, (0, 0, 0));

        let s2 = Selector::parse("div.ws-btn.active:hover").unwrap();
        assert_eq!(s2.specificity, (0, 3, 1));

        let s3 = Selector::parse("#topbar > div.modules[data-layer=\"top\"] button:first-child")
            .unwrap();
        assert_eq!(s3.specificity, (1, 3, 2));
    }

    #[test]
    fn test_custom_properties_and_cascade() {
        let mut dom = MockDom::new(NodeId(1));
        let mut root = make_element(1, "div", Some("topbar"), &["panel"], None, &[2, 3]);
        root.attributes
            .insert("data-layer".to_string(), "top".to_string());

        let mut btn1 = make_element(2, "button", None, &["ws-btn", "active"], Some(1), &[]);
        btn1.pseudo_states.insert(PseudoStates::HOVER);

        let mut btn2 = make_element(3, "button", None, &["ws-btn"], Some(1), &[]);
        btn2.inline_styles
            .insert("padding".to_string(), "6px 14px".to_string());
        btn2.visible = false;

        dom.add_node(root);
        dom.add_node(btn1);
        dom.add_node(btn2);

        let css = r#"
            /* Theme variables */
            :root {
                --bg: #1e1e2e;
                --accent: #89b4fa;
                --btn-border: var(--accent);
                --text: rgb(205, 214, 244);
                font-family: "JetBrains Mono";
                font-size: 16px;
                color: var(--text);
            }

            #topbar[data-layer="top"] {
                display: flex;
                justify-content: space-between;
                align-items: center;
                height: 42px;
                padding: 4px 12px;
                background: linear-gradient(135deg, #1e1e2e 0%, #313244 100%);
                box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
                backdrop-filter: blur(12px);
            }

            #topbar > .ws-btn {
                padding: 4px 8px;
                border: 1px solid var(-- fallback-missing, #45475a );
                border-radius: 8px;
                font-weight: 500;
            }

            .ws-btn.active:hover {
                border: 2px solid var(--btn-border);
                background-color: var(--accent);
                color: #11111b;
                font-weight: bold;
            }
        "#;

        let mut engine = CssStyleEngine::from_css(css).unwrap();
        assert_eq!(engine.get_custom_property("--accent"), Some("#89b4fa"));

        // Override --accent at runtime
        engine.set_custom_property("--accent", "#f38ba8");
        assert_eq!(engine.get_custom_property("--accent"), Some("#f38ba8"));

        let styles = engine.compute_styles(&dom);
        let root_style = &styles[&NodeId(1)];
        assert_eq!(root_style.justify_content, JustifyContent::SpaceBetween);
        assert_eq!(root_style.height, Dimension::Px(42.0));
        assert_eq!(root_style.padding, Edges::xy(12.0, 4.0));
        assert_eq!(root_style.backdrop_blur, 12.0);
        assert!(root_style.background_gradient.is_some());
        let grad = root_style.background_gradient.as_ref().unwrap();
        assert_eq!(grad.angle_deg, 135.0);
        assert_eq!(grad.stops.len(), 2);
        assert_eq!(grad.stops[0].offset, 0.0);
        assert_eq!(grad.stops[1].offset, 1.0);
        assert!(root_style.box_shadow.is_some());
        assert_eq!(root_style.box_shadow.as_ref().unwrap().blur_radius, 24.0);

        // Check btn1: matched both `#topbar > .ws-btn` (1, 1, 0) and `.ws-btn.active:hover` (0, 3, 0)
        // Wait: `#topbar > .ws-btn` has ID so specificity (1, 1, 0) > (0, 3, 0), unless we check background-color from .ws-btn.active:hover
        let btn1_style = &styles[&NodeId(2)];
        assert_eq!(btn1_style.background_color, parse_color("#f38ba8").unwrap());
        // Inherited font-family and font-size from :root
        assert_eq!(btn1_style.font_family, "JetBrains Mono");
        assert_eq!(btn1_style.font_size, 16.0);

        // Check btn2: inline style overrides padding, visible=false sets Display::None
        let btn2_style = &styles[&NodeId(3)];
        assert_eq!(btn2_style.padding, Edges::xy(14.0, 6.0));
        assert_eq!(btn2_style.display, Display::None);

        // Query selector all
        let buttons = engine.query_selector_all(&dom, "#topbar > button.ws-btn");
        assert_eq!(buttons, vec![NodeId(2), NodeId(3)]);
        let first_child = engine.query_selector_all(&dom, "button:first-child");
        assert_eq!(first_child, vec![NodeId(2)]);
        let last_child = engine.query_selector_all(&dom, "button:last-child");
        assert_eq!(last_child, vec![NodeId(3)]);

        // Inspect node style
        let report = engine.inspect_node_style(&dom, NodeId(2));
        assert_eq!(report.node_id, 2);
        assert_eq!(report.matched_rules.len(), 2);
        assert_eq!(
            report.resolved_variables.get("--btn-border").map(|s| s.as_str()),
            Some("#f38ba8")
        );
    }

    #[test]
    fn test_all_pseudo_classes_attributes_and_combinators() {
        let mut dom = MockDom::new(NodeId(1));
        let root = make_element(1, "window", Some("main-win"), &["tiled"], None, &[2, 3, 4]);
        let mut titlebar = make_element(2, "titlebar", None, &["header"], Some(1), &[5, 6]);
        titlebar
            .attributes
            .insert("data-mode".to_string(), "floating".to_string());

        let mut input = make_element(3, "input", Some("search-input"), &[], Some(1), &[]);
        input
            .attributes
            .insert("type".to_string(), "text".to_string());
        input
            .pseudo_states
            .insert(PseudoStates::FOCUS | PseudoStates::FOCUS_VISIBLE);

        let mut toggle = make_element(4, "button", None, &["toggle"], Some(1), &[]);
        toggle
            .pseudo_states
            .insert(PseudoStates::CHECKED | PseudoStates::URGENT | PseudoStates::ACTIVE);

        let mut btn_first = make_element(5, "button", None, &["act"], Some(2), &[]);
        btn_first.pseudo_states.insert(PseudoStates::DISABLED);
        let btn_last = make_element(6, "button", None, &["act"], Some(2), &[]);

        dom.add_node(root);
        dom.add_node(titlebar);
        dom.add_node(input);
        dom.add_node(toggle);
        dom.add_node(btn_first);
        dom.add_node(btn_last);

        let css = r#"
            window#main-win {
                width: 80%;
                min-width: 300px;
                max-width: 1200px;
                min-height: 200px;
                max-height: 900px;
                font-size: 16px;
                text-align: center;
            }
            window titlebar[data-mode="floating"] {
                font-size: 1.5rem;
                margin: 2px 4px 6px 8px;
                border-radius: 10px 12px 14px 16px;
            }
            input#search-input[type="text"]:focus:focus-visible {
                font-size: 1.25em;
                border: 2px solid hsl(200, 80%, 60%);
                opacity: 0.9;
                overflow: hidden;
                z-index: 42;
            }
            button.toggle:checked:urgent:active {
                display: grid;
                grid-template-columns: repeat(4, 1fr);
                gap: 6px 10px;
                flex: 2 0;
                position: absolute;
                top: 10px;
                right: 20px;
                bottom: 30px;
                left: 40px;
            }
            titlebar > button:first-child:disabled {
                opacity: 0.4;
            }
            titlebar > button:last-child {
                opacity: 0.8;
            }
        "#;

        let mut engine = CssStyleEngine::from_css(css).unwrap();
        engine
            .append_stylesheet("titlebar > button:last-child { z-index: 7; }")
            .unwrap();

        let styles = engine.compute_styles(&dom);

        let win_s = &styles[&NodeId(1)];
        assert_eq!(win_s.width, Dimension::Percent(80.0));
        assert_eq!(win_s.min_width, Dimension::Px(300.0));
        assert_eq!(win_s.max_width, Dimension::Px(1200.0));
        assert_eq!(win_s.min_height, Dimension::Px(200.0));
        assert_eq!(win_s.max_height, Dimension::Px(900.0));
        assert_eq!(win_s.text_align, TextAlign::Center);

        let tb_s = &styles[&NodeId(2)];
        assert_eq!(tb_s.font_size, 24.0); // 1.5rem of 16px
        assert_eq!(
            tb_s.margin,
            Edges {
                top: 2.0,
                right: 4.0,
                bottom: 6.0,
                left: 8.0
            }
        );
        assert_eq!(tb_s.border_radius, [10.0, 12.0, 14.0, 16.0]);
        assert_eq!(tb_s.text_align, TextAlign::Center); // inherited from window

        let in_s = &styles[&NodeId(3)];
        assert_eq!(in_s.font_size, 20.0); // 1.25em of parent 16px
        assert_eq!(in_s.border_width, Edges::all(2.0));
        assert!((in_s.opacity - 0.9).abs() < 0.001);
        assert_eq!(in_s.overflow, Overflow::Hidden);
        assert_eq!(in_s.z_index, 42);

        let tog_s = &styles[&NodeId(4)];
        assert_eq!(tog_s.display, Display::Grid);
        assert_eq!(tog_s.grid_columns, 4);
        assert_eq!(tog_s.gap_row, 6.0);
        assert_eq!(tog_s.gap_column, 10.0);
        assert_eq!(tog_s.flex_grow, 2.0);
        assert_eq!(tog_s.flex_shrink, 0.0);
        assert_eq!(tog_s.position, Position::Absolute);
        assert_eq!(tog_s.inset_top, Dimension::Px(10.0));
        assert_eq!(tog_s.inset_right, Dimension::Px(20.0));
        assert_eq!(tog_s.inset_bottom, Dimension::Px(30.0));
        assert_eq!(tog_s.inset_left, Dimension::Px(40.0));

        let b1_s = &styles[&NodeId(5)];
        assert!((b1_s.opacity - 0.4).abs() < 0.001);
        assert_eq!(b1_s.font_size, 24.0); // inherited from titlebar

        let b2_s = &styles[&NodeId(6)];
        assert!((b2_s.opacity - 0.8).abs() < 0.001);
        assert_eq!(b2_s.z_index, 7);

        // Verify document order of query_selector_all
        let all_btns = engine.query_selector_all(&dom, "button");
        assert_eq!(all_btns, vec![NodeId(5), NodeId(6), NodeId(4)]);
    }
}
