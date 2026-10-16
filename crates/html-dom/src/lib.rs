pub mod directives;
pub mod parser;

pub use directives::DomEventKind;
pub use wm_proto;

use directives::{
    compute_item_key, evaluate_event_handler, evaluate_with_fallback, find_matching_event_attr,
    is_self_repeating_for_node, parse_for_expression, sync_single_node_directives, ForDirective,
    ForInstance, NodeDirectives,
};
use parser::{
    format_inline_styles, parse_classes, parse_html_document, parse_inline_styles, RawParsedNode,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;
use wm_proto::{
    ActionCommand, DirtyFlags, DomElementInfo, DomNodeData, DomNodeKind, DomTreeView, LayoutBox,
    NodeId, PseudoStates, ReactiveStore, StateValue, SurfaceMetadata,
};

/// Conversion trait allowing `set_attr` and `set_inline_style` to accept `&str`, `String`,
/// `Option<&str>`, or `Option<String>`.
pub trait IntoAttrValue {
    fn into_attr_value(self) -> Option<String>;
}

impl IntoAttrValue for &str {
    fn into_attr_value(self) -> Option<String> {
        Some(self.to_string())
    }
}

impl IntoAttrValue for String {
    fn into_attr_value(self) -> Option<String> {
        Some(self)
    }
}

impl IntoAttrValue for &String {
    fn into_attr_value(self) -> Option<String> {
        Some(self.clone())
    }
}

impl IntoAttrValue for Option<&str> {
    fn into_attr_value(self) -> Option<String> {
        self.map(|s| s.to_string())
    }
}

impl IntoAttrValue for Option<String> {
    fn into_attr_value(self) -> Option<String> {
        self
    }
}

impl IntoAttrValue for Option<&String> {
    fn into_attr_value(self) -> Option<String> {
        self.cloned()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ChildSlot {
    Static(NodeId),
    For(ForDirective),
}

#[derive(Clone, Debug)]
pub(crate) struct DomNodeSlot {
    pub data: DomNodeData,
    pub alive: bool,
    pub dirty: DirtyFlags,
    pub raw_text_template: Option<String>,
    pub raw_attributes: BTreeMap<String, String>,
    pub base_inline_styles: BTreeMap<String, String>,
    pub directives: NodeDirectives,
    pub child_slots: Vec<ChildSlot>,
    pub local_scope: BTreeMap<String, StateValue>,
}

/// Arena-allocated DOM document with reactive directive bindings and event bubbling.
#[derive(Clone, Debug)]
pub struct DomDocument {
    pub metadata: SurfaceMetadata,
    pub embedded_styles: Vec<String>,
    root: NodeId,
    nodes: Vec<DomNodeSlot>,
    global_dirty: DirtyFlags,
}

impl DomDocument {
    /// Parses an HTML5/XHTML document or fragment into an arena-backed `DomDocument`.
    pub fn parse(html: &str) -> Self {
        let parsed = parse_html_document(html);
        let mut doc = Self {
            metadata: parsed.metadata,
            embedded_styles: parsed.embedded_styles,
            root: NodeId(0),
            nodes: Vec::new(),
            global_dirty: DirtyFlags::ALL,
        };
        let root_id = doc.instantiate_raw_node(&parsed.root, None, BTreeMap::new());
        doc.root = root_id;
        doc
    }

    /// Fallible wrapper around [`DomDocument::parse`] for callers expecting a `Result`.
    pub fn try_parse(html: &str) -> Result<Self, String> {
        Ok(Self::parse(html))
    }

    /// Alias for [`DomDocument::parse`].
    pub fn from_html(html: &str) -> Self {
        Self::parse(html)
    }

    pub fn metadata(&self) -> &SurfaceMetadata {
        &self.metadata
    }

    pub fn metadata_mut(&mut self) -> &mut SurfaceMetadata {
        &mut self.metadata
    }

    pub fn embedded_styles(&self) -> &[String] {
        &self.embedded_styles
    }

    pub fn dirty_flags(&self) -> DirtyFlags {
        self.global_dirty
    }

    pub fn node_dirty_flags(&self, id: NodeId) -> DirtyFlags {
        self.nodes
            .get(id.index())
            .filter(|s| s.alive)
            .map(|s| s.dirty)
            .unwrap_or_default()
    }

    pub fn is_dirty(&self) -> bool {
        !self.global_dirty.is_empty()
    }

    pub fn mark_dirty(&mut self, id: NodeId, flags: DirtyFlags) {
        if let Some(slot) = self.nodes.get_mut(id.index()) {
            if slot.alive {
                slot.dirty |= flags;
                self.global_dirty |= flags;
            }
        }
    }

    pub fn clear_dirty(&mut self) {
        self.global_dirty = DirtyFlags::empty();
        for slot in &mut self.nodes {
            slot.dirty = DirtyFlags::empty();
        }
    }

    pub fn take_dirty(&mut self) -> DirtyFlags {
        let prev = self.global_dirty;
        self.clear_dirty();
        prev
    }

    pub fn node_count(&self) -> usize {
        self.all_node_ids().len()
    }

    pub fn get_node_mut(&mut self, id: NodeId) -> Option<&mut DomNodeData> {
        let slot = self.nodes.get_mut(id.index())?;
        if slot.alive {
            slot.dirty |= DirtyFlags::ALL;
            self.global_dirty |= DirtyFlags::ALL;
            Some(&mut slot.data)
        } else {
            None
        }
    }

    pub fn find_by_id(&self, element_id: &str) -> Option<NodeId> {
        for id in self.all_node_ids() {
            if let Some(node) = self.get_node(id) {
                if node.element_id.as_deref() == Some(element_id) {
                    return Some(id);
                }
            }
        }
        None
    }

    pub fn find_by_tag(&self, tag: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for id in self.all_node_ids() {
            if let Some(node) = self.get_node(id) {
                if node.kind == DomNodeKind::Element && node.tag.eq_ignore_ascii_case(tag) {
                    out.push(id);
                }
            }
        }
        out
    }

    pub fn find_by_class(&self, class_name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for id in self.all_node_ids() {
            if let Some(node) = self.get_node(id) {
                if node.has_class(class_name) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// Sets or removes an attribute on `node_id`.
    pub fn set_attr(
        &mut self,
        node_id: NodeId,
        name: &str,
        value: impl IntoAttrValue,
    ) -> DirtyFlags {
        let Some(slot) = self.nodes.get_mut(node_id.index()) else {
            return DirtyFlags::empty();
        };
        if !slot.alive {
            return DirtyFlags::empty();
        }

        let val_opt = value.into_attr_value();
        let mut changed = false;

        match val_opt {
            Some(val) => {
                if slot.data.attributes.get(name) != Some(&val) {
                    slot.data.attributes.insert(name.to_string(), val.clone());
                    changed = true;
                }
                slot.raw_attributes.insert(name.to_string(), val.clone());
                if name == "id" {
                    slot.data.element_id = if val.is_empty() { None } else { Some(val) };
                } else if name == "class" {
                    slot.data.classes = parse_classes(&val);
                } else if name == "style" {
                    let parsed = parse_inline_styles(&val);
                    slot.base_inline_styles = parsed.clone();
                    slot.data.inline_styles = parsed;
                }
            }
            None => {
                if slot.data.attributes.remove(name).is_some() {
                    changed = true;
                }
                slot.raw_attributes.remove(name);
                if name == "id" {
                    slot.data.element_id = None;
                } else if name == "class" {
                    slot.data.classes.clear();
                } else if name == "style" {
                    slot.base_inline_styles.clear();
                    slot.data.inline_styles.clear();
                }
            }
        }

        slot.directives = NodeDirectives::from_attributes(&slot.raw_attributes);

        if changed {
            let flags = DirtyFlags::ALL;
            slot.dirty |= flags;
            self.global_dirty |= flags;
            flags
        } else {
            DirtyFlags::empty()
        }
    }

    pub fn remove_attr(&mut self, node_id: NodeId, name: &str) -> DirtyFlags {
        self.set_attr(node_id, name, None::<String>)
    }

    /// Sets the text content of a text node or element node.
    pub fn set_text(&mut self, node_id: NodeId, text: impl Into<String>) -> DirtyFlags {
        let new_text = text.into();
        let Some(slot) = self.nodes.get(node_id.index()) else {
            return DirtyFlags::empty();
        };
        if !slot.alive {
            return DirtyFlags::empty();
        }

        if slot.data.kind == DomNodeKind::Text {
            let slot = &mut self.nodes[node_id.index()];
            if slot.data.text_content.as_deref() != Some(new_text.as_str()) {
                slot.data.text_content = Some(new_text.clone());
                slot.raw_text_template = Some(new_text);
                let flags = DirtyFlags::LAYOUT | DirtyFlags::PAINT;
                slot.dirty |= flags;
                self.global_dirty |= flags;
                return flags;
            }
            return DirtyFlags::empty();
        }

        // For an Element node, remove any direct Text children so `inner_text` does not duplicate
        self.remove_text_children(node_id);
        let slot = &mut self.nodes[node_id.index()];
        if slot.data.text_content.as_deref() != Some(new_text.as_str()) {
            slot.data.text_content = Some(new_text.clone());
            slot.raw_text_template = Some(new_text);
            let flags = DirtyFlags::LAYOUT | DirtyFlags::PAINT;
            slot.dirty |= flags;
            self.global_dirty |= flags;
            flags
        } else {
            DirtyFlags::empty()
        }
    }

    pub fn add_class(&mut self, node_id: NodeId, class_name: &str) -> DirtyFlags {
        let cls = class_name.trim();
        if cls.is_empty() {
            return DirtyFlags::empty();
        }
        let Some(slot) = self.nodes.get_mut(node_id.index()) else {
            return DirtyFlags::empty();
        };
        if !slot.alive {
            return DirtyFlags::empty();
        }
        if !slot.data.classes.iter().any(|c| c == cls) {
            slot.data.classes.push(cls.to_string());
            slot.data
                .attributes
                .insert("class".to_string(), slot.data.classes.join(" "));
            let flags = DirtyFlags::ALL;
            slot.dirty |= flags;
            self.global_dirty |= flags;
            flags
        } else {
            DirtyFlags::empty()
        }
    }

    pub fn remove_class(&mut self, node_id: NodeId, class_name: &str) -> DirtyFlags {
        let cls = class_name.trim();
        let Some(slot) = self.nodes.get_mut(node_id.index()) else {
            return DirtyFlags::empty();
        };
        if !slot.alive {
            return DirtyFlags::empty();
        }
        let before = slot.data.classes.len();
        slot.data.classes.retain(|c| c != cls);
        if slot.data.classes.len() != before {
            slot.data
                .attributes
                .insert("class".to_string(), slot.data.classes.join(" "));
            let flags = DirtyFlags::ALL;
            slot.dirty |= flags;
            self.global_dirty |= flags;
            flags
        } else {
            DirtyFlags::empty()
        }
    }

    /// Toggles `class_name` on `node_id`, returning `true` if the class is now present.
    pub fn toggle_class(&mut self, node_id: NodeId, class_name: &str) -> bool {
        let has_it = self
            .get_node(node_id)
            .map(|n| n.has_class(class_name))
            .unwrap_or(false);
        if has_it {
            self.remove_class(node_id, class_name);
            false
        } else {
            self.add_class(node_id, class_name);
            true
        }
    }

    pub fn set_inline_style(
        &mut self,
        node_id: NodeId,
        prop: &str,
        value: impl IntoAttrValue,
    ) -> DirtyFlags {
        let prop_name = prop.trim();
        if prop_name.is_empty() {
            return DirtyFlags::empty();
        }
        let Some(slot) = self.nodes.get_mut(node_id.index()) else {
            return DirtyFlags::empty();
        };
        if !slot.alive {
            return DirtyFlags::empty();
        }

        let val_opt = value.into_attr_value();
        let changed = match val_opt {
            Some(v) if !v.trim().is_empty() => {
                let v_trim = v.trim().to_string();
                slot.base_inline_styles
                    .insert(prop_name.to_string(), v_trim.clone());
                slot.data
                    .inline_styles
                    .insert(prop_name.to_string(), v_trim)
                    .as_deref()
                    != Some(v.trim())
            }
            _ => {
                slot.base_inline_styles.remove(prop_name);
                slot.data.inline_styles.remove(prop_name).is_some()
            }
        };

        if changed {
            slot.data.attributes.insert(
                "style".to_string(),
                format_inline_styles(&slot.data.inline_styles),
            );
            let flags = DirtyFlags::ALL;
            slot.dirty |= flags;
            self.global_dirty |= flags;
            flags
        } else {
            DirtyFlags::empty()
        }
    }

    /// Sets or clears `state` flags on `node_id`, returning `true` if the node's pseudo-states changed.
    pub fn set_pseudo_state(
        &mut self,
        node_id: NodeId,
        state: PseudoStates,
        enabled: bool,
    ) -> bool {
        let Some(slot) = self.nodes.get_mut(node_id.index()) else {
            return false;
        };
        if !slot.alive {
            return false;
        }
        let before = slot.data.pseudo_states;
        slot.data.pseudo_states.set(state, enabled);
        if slot.data.pseudo_states != before {
            let flags = DirtyFlags::STYLE | DirtyFlags::LAYOUT | DirtyFlags::PAINT;
            slot.dirty |= flags;
            self.global_dirty |= flags;
            true
        } else {
            false
        }
    }

    /// Sets `PseudoStates::HOVER` on `hovered_node` and all its ancestors, clearing
    /// `PseudoStates::HOVER` on all other nodes. Returns `true` if any node's hover state changed.
    pub fn update_hover_chain(&mut self, hovered_node: Option<NodeId>) -> bool {
        let mut hover_chain = BTreeSet::new();
        let mut cur = hovered_node;
        while let Some(id) = cur {
            if let Some(slot) = self.nodes.get(id.index()) {
                if slot.alive {
                    hover_chain.insert(id);
                    cur = slot.data.parent;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let mut any_changed = false;
        for (idx, slot) in self.nodes.iter_mut().enumerate() {
            if !slot.alive {
                continue;
            }
            let id = NodeId(idx);
            let should_hover = hover_chain.contains(&id);
            let was_hover = slot.data.pseudo_states.contains(PseudoStates::HOVER);
            if should_hover != was_hover {
                slot.data
                    .pseudo_states
                    .set(PseudoStates::HOVER, should_hover);
                let flags = DirtyFlags::STYLE | DirtyFlags::LAYOUT | DirtyFlags::PAINT;
                slot.dirty |= flags;
                self.global_dirty |= flags;
                any_changed = true;
            }
        }
        any_changed
    }

    /// Exports a `DomElementInfo` tree starting at `node_id` for CLI / IPC DOM inspection.
    pub fn export_element_info(
        &self,
        surface_name: &str,
        node_id: NodeId,
        layouts: Option<&HashMap<NodeId, LayoutBox>>,
        max_depth: Option<usize>,
    ) -> DomElementInfo {
        let Some(node) = self.get_node(node_id) else {
            return DomElementInfo {
                surface: surface_name.to_string(),
                node_id: node_id.index(),
                tag: String::new(),
                id: None,
                classes: Vec::new(),
                attrs: BTreeMap::new(),
                pseudo_states: Vec::new(),
                text_content: String::new(),
                visible: false,
                layout_box: None,
                children: Vec::new(),
            };
        };

        let mut children_info = Vec::new();
        if max_depth.map_or(true, |d| d > 0) {
            let next_depth = max_depth.map(|d| d.saturating_sub(1));
            for &child_id in &node.children {
                if let Some(child_node) = self.get_node(child_id) {
                    if child_node.kind == DomNodeKind::Element {
                        children_info.push(self.export_element_info(
                            surface_name,
                            child_id,
                            layouts,
                            next_depth,
                        ));
                    }
                }
            }
        }

        DomElementInfo {
            surface: surface_name.to_string(),
            node_id: node.id.index(),
            tag: node.tag.clone(),
            id: node.element_id.clone(),
            classes: node.classes.clone(),
            attrs: node.attributes.clone(),
            pseudo_states: node
                .pseudo_states
                .to_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            text_content: self.inner_text(node_id),
            visible: node.visible,
            layout_box: layouts
                .and_then(|m| m.get(&node_id))
                .map(|lb| lb.border_box),
            children: children_info,
        }
    }

    /// Evaluates all reactive directives (`{{ expr }}`, `data-bind`, `:attr`, `:class.*`,
    /// `:style.*`, `data-if`, `data-show`, and keyed `data-for` lists) against `store`.
    /// Returns the combined `DirtyFlags` produced by this synchronization pass.
    pub fn sync_reactive_state(&mut self, store: &dyn ReactiveStore) -> DirtyFlags {
        let root_id = self.root;
        let flags = self.sync_subtree(root_id, store);
        self.global_dirty |= flags;
        flags
    }

    /// Dispatches a `DomEventKind` starting at `target` and bubbling up the `parent` chain
    /// (stopping early if a handler has the `.stop` modifier).
    pub fn dispatch_dom_event(
        &self,
        target: NodeId,
        event_kind: DomEventKind,
        store: &dyn ReactiveStore,
    ) -> Vec<ActionCommand> {
        let mut actions = Vec::new();
        let mut cur = Some(target);

        while let Some(node_id) = cur {
            let Some(slot) = self.nodes.get(node_id.index()) else {
                break;
            };
            if !slot.alive {
                break;
            }

            if let Some((raw_handler, stop_propagation)) =
                find_matching_event_attr(&slot.raw_attributes, &event_kind)
            {
                let mut cmds =
                    evaluate_event_handler(raw_handler, &event_kind, &slot.local_scope, store);
                actions.append(&mut cmds);
                if stop_propagation {
                    break;
                }
            }

            cur = slot.data.parent;
        }

        actions
    }

    // ---- Internal Arena & Directive Helpers ----

    fn instantiate_raw_node(
        &mut self,
        raw: &RawParsedNode,
        parent: Option<NodeId>,
        local_scope: BTreeMap<String, StateValue>,
    ) -> NodeId {
        match raw {
            RawParsedNode::Text(txt) => {
                let id = NodeId(self.nodes.len());
                let data = DomNodeData {
                    id,
                    kind: DomNodeKind::Text,
                    tag: "#text".to_string(),
                    element_id: None,
                    classes: Vec::new(),
                    attributes: BTreeMap::new(),
                    inline_styles: BTreeMap::new(),
                    text_content: Some(txt.clone()),
                    pseudo_states: PseudoStates::empty(),
                    visible: true,
                    parent,
                    children: Vec::new(),
                };
                self.nodes.push(DomNodeSlot {
                    data,
                    alive: true,
                    dirty: DirtyFlags::ALL,
                    raw_text_template: Some(txt.clone()),
                    raw_attributes: BTreeMap::new(),
                    base_inline_styles: BTreeMap::new(),
                    directives: NodeDirectives::default(),
                    child_slots: Vec::new(),
                    local_scope,
                });
                id
            }
            RawParsedNode::Element {
                tag,
                attributes,
                children,
            } => {
                let id = NodeId(self.nodes.len());
                let element_id = attributes.get("id").cloned().filter(|s| !s.is_empty());
                let classes = attributes
                    .get("class")
                    .map(|c| parse_classes(c))
                    .unwrap_or_default();
                let inline_styles = attributes
                    .get("style")
                    .map(|s| parse_inline_styles(s))
                    .unwrap_or_default();
                let directives = NodeDirectives::from_attributes(attributes);

                let data = DomNodeData {
                    id,
                    kind: DomNodeKind::Element,
                    tag: tag.clone(),
                    element_id,
                    classes,
                    attributes: attributes.clone(),
                    inline_styles: inline_styles.clone(),
                    text_content: None,
                    pseudo_states: PseudoStates::empty(),
                    visible: true,
                    parent,
                    children: Vec::new(),
                };

                self.nodes.push(DomNodeSlot {
                    data,
                    alive: true,
                    dirty: DirtyFlags::ALL,
                    raw_text_template: None,
                    raw_attributes: attributes.clone(),
                    base_inline_styles: inline_styles,
                    directives,
                    child_slots: Vec::new(),
                    local_scope: local_scope.clone(),
                });

                // Check if this element itself is a container `data-for`
                if let Some(for_attr) = attributes.get("data-for") {
                    if let Some((item_var, index_var, list_expr)) = parse_for_expression(for_attr) {
                        let key_expr = attributes.get("data-key").cloned();
                        let for_dir = ForDirective {
                            item_var,
                            index_var,
                            list_expr,
                            key_expr,
                            template_nodes: children.clone(),
                            instances: Vec::new(),
                        };
                        self.nodes[id.index()].child_slots = vec![ChildSlot::For(for_dir)];
                        return id;
                    }
                }

                // Otherwise, process children (detecting any self-repeating Vue-style `data-for` child elements)
                let mut child_slots = Vec::new();
                let mut initial_children = Vec::new();

                for child in children {
                    if let RawParsedNode::Element {
                        tag: c_tag,
                        attributes: c_attrs,
                        children: c_children,
                    } = child
                    {
                        if let Some(for_attr) = c_attrs.get("data-for") {
                            if let Some((item_var, index_var, list_expr)) =
                                parse_for_expression(for_attr)
                            {
                                if is_self_repeating_for_node(c_attrs, c_children, &item_var) {
                                    let key_expr = c_attrs.get("data-key").cloned();
                                    let mut stripped_attrs = c_attrs.clone();
                                    stripped_attrs.remove("data-for");
                                    stripped_attrs.remove("data-key");
                                    let template_node = RawParsedNode::Element {
                                        tag: c_tag.clone(),
                                        attributes: stripped_attrs,
                                        children: c_children.clone(),
                                    };
                                    child_slots.push(ChildSlot::For(ForDirective {
                                        item_var,
                                        index_var,
                                        list_expr,
                                        key_expr,
                                        template_nodes: vec![template_node],
                                        instances: Vec::new(),
                                    }));
                                    continue;
                                }
                            }
                        }
                    }

                    let child_id = self.instantiate_raw_node(child, Some(id), local_scope.clone());
                    child_slots.push(ChildSlot::Static(child_id));
                    initial_children.push(child_id);
                }

                self.nodes[id.index()].child_slots = child_slots;
                self.nodes[id.index()].data.children = initial_children;
                id
            }
        }
    }

    fn sync_subtree(&mut self, node_id: NodeId, store: &dyn ReactiveStore) -> DirtyFlags {
        let Some(slot) = self.nodes.get(node_id.index()) else {
            return DirtyFlags::empty();
        };
        if !slot.alive {
            return DirtyFlags::empty();
        }

        // 1. Sync single-node directives on `node_id`
        let (node_dirty, clear_text_children) = {
            let slot = &mut self.nodes[node_id.index()];
            let (d, clear_txt) = sync_single_node_directives(
                &slot.data.kind,
                &slot.data.tag,
                &mut slot.data.element_id,
                &mut slot.data.classes,
                &mut slot.data.attributes,
                &mut slot.data.inline_styles,
                &mut slot.data.text_content,
                &mut slot.data.visible,
                slot.raw_text_template.as_deref(),
                &slot.base_inline_styles,
                &slot.directives,
                &slot.local_scope,
                store,
            );
            slot.dirty |= d;
            (d, clear_txt)
        };

        if clear_text_children {
            self.remove_text_children(node_id);
        }

        let mut total_dirty = node_dirty;

        // 2. Reconcile any `ChildSlot::For` directives on `node_id`
        let has_for_slots = self.nodes[node_id.index()]
            .child_slots
            .iter()
            .any(|cs| matches!(cs, ChildSlot::For(_)));

        if has_for_slots {
            let for_dirty = self.reconcile_for_slots(node_id, store);
            total_dirty |= for_dirty;
        }

        // 3. Recursively sync all current children
        let children: Vec<NodeId> = self.nodes[node_id.index()].data.children.clone();
        for child_id in children {
            total_dirty |= self.sync_subtree(child_id, store);
        }

        total_dirty
    }

    fn reconcile_for_slots(&mut self, container_id: NodeId, store: &dyn ReactiveStore) -> DirtyFlags {
        let parent_scope = self.nodes[container_id.index()].local_scope.clone();
        let mut child_slots = std::mem::take(&mut self.nodes[container_id.index()].child_slots);
        let prev_children = self.nodes[container_id.index()].data.children.clone();

        let mut new_children: Vec<NodeId> = Vec::new();
        let mut dirty = DirtyFlags::empty();

        for slot_item in &mut child_slots {
            match slot_item {
                ChildSlot::Static(cid) => {
                    if self
                        .nodes
                        .get(cid.index())
                        .map(|s| s.alive)
                        .unwrap_or(false)
                    {
                        new_children.push(*cid);
                    }
                }
                ChildSlot::For(for_dir) => {
                    let parent_scope_opt = if parent_scope.is_empty() {
                        None
                    } else {
                        Some(&parent_scope)
                    };
                    let items: Vec<StateValue> =
                        match evaluate_with_fallback(store, &for_dir.list_expr, parent_scope_opt) {
                            Ok(StateValue::List(list)) => list,
                            _ => Vec::new(),
                        };

                    let mut old_instances: Vec<Option<ForInstance>> =
                        std::mem::take(&mut for_dir.instances)
                            .into_iter()
                            .map(Some)
                            .collect();
                    let mut new_instances: Vec<ForInstance> = Vec::with_capacity(items.len());

                    for (idx, item) in items.iter().enumerate() {
                        let mut item_scope = parent_scope.clone();
                        item_scope.insert(for_dir.item_var.clone(), item.clone());
                        item_scope.insert("index".to_string(), StateValue::Int(idx as i64));
                        if let Some(idx_var) = &for_dir.index_var {
                            item_scope.insert(idx_var.clone(), StateValue::Int(idx as i64));
                        }

                        let key = compute_item_key(
                            store,
                            for_dir.key_expr.as_deref(),
                            &for_dir.item_var,
                            item,
                            idx,
                            &item_scope,
                        );

                        // Look for an existing unused instance with matching key
                        let matched_pos = old_instances.iter().position(|opt| {
                            opt.as_ref().map(|inst| inst.key == key).unwrap_or(false)
                        });

                        if let Some(pos) = matched_pos {
                            let inst = old_instances[pos].take().unwrap();
                            for &root_nid in &inst.root_nodes {
                                self.update_subtree_scope(root_nid, &item_scope);
                            }
                            new_children.extend_from_slice(&inst.root_nodes);
                            new_instances.push(inst);
                        } else {
                            // Instantiate fresh nodes from template_nodes
                            let mut root_nodes = Vec::with_capacity(for_dir.template_nodes.len());
                            for tpl in &for_dir.template_nodes {
                                let nid = self.instantiate_raw_node(
                                    tpl,
                                    Some(container_id),
                                    item_scope.clone(),
                                );
                                root_nodes.push(nid);
                            }
                            new_children.extend_from_slice(&root_nodes);
                            new_instances.push(ForInstance { key, root_nodes });
                            dirty |= DirtyFlags::ALL;
                        }
                    }

                    // Mark any unmatched old instances as dead
                    for leftover in old_instances.into_iter().flatten() {
                        for root_nid in leftover.root_nodes {
                            self.kill_subtree(root_nid);
                        }
                        dirty |= DirtyFlags::ALL;
                    }

                    for_dir.instances = new_instances;
                }
            }
        }

        if new_children != prev_children {
            dirty |= DirtyFlags::ALL;
        }

        let container_slot = &mut self.nodes[container_id.index()];
        container_slot.child_slots = child_slots;
        container_slot.data.children = new_children;
        container_slot.dirty |= dirty;
        dirty
    }

    fn update_subtree_scope(&mut self, node_id: NodeId, scope: &BTreeMap<String, StateValue>) {
        let Some(slot) = self.nodes.get_mut(node_id.index()) else {
            return;
        };
        if !slot.alive {
            return;
        }
        for (k, v) in scope {
            slot.local_scope.insert(k.clone(), v.clone());
        }
        let children = slot.data.children.clone();
        for child_id in children {
            self.update_subtree_scope(child_id, scope);
        }
    }

    fn kill_subtree(&mut self, node_id: NodeId) {
        let Some(slot) = self.nodes.get_mut(node_id.index()) else {
            return;
        };
        slot.alive = false;
        let children = std::mem::take(&mut slot.data.children);
        for child_id in children {
            self.kill_subtree(child_id);
        }
    }

    fn remove_text_children(&mut self, node_id: NodeId) {
        let children = self.nodes[node_id.index()].data.children.clone();
        let mut kept_children = Vec::new();
        for cid in children {
            let is_text = self
                .nodes
                .get(cid.index())
                .map(|s| s.data.kind == DomNodeKind::Text)
                .unwrap_or(false);
            if is_text {
                self.kill_subtree(cid);
            } else {
                kept_children.push(cid);
            }
        }
        let slot = &mut self.nodes[node_id.index()];
        slot.data.children = kept_children;
        slot.child_slots.retain(|cs| match cs {
            ChildSlot::Static(cid) => slot.data.children.contains(cid),
            ChildSlot::For(_) => true,
        });
    }

    fn collect_preorder_ids(&self, id: NodeId, out: &mut Vec<NodeId>) {
        let Some(slot) = self.nodes.get(id.index()) else {
            return;
        };
        if !slot.alive {
            return;
        }
        out.push(id);
        for &child_id in &slot.data.children {
            self.collect_preorder_ids(child_id, out);
        }
    }
}

impl FromStr for DomDocument {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

impl DomTreeView for DomDocument {
    fn root_id(&self) -> NodeId {
        self.root
    }

    fn get_node(&self, id: NodeId) -> Option<&DomNodeData> {
        self.nodes
            .get(id.index())
            .filter(|slot| slot.alive)
            .map(|slot| &slot.data)
    }

    fn all_node_ids(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect_preorder_ids(self.root, &mut out);
        out
    }

    fn children_of(&self, id: NodeId) -> &[NodeId] {
        self.nodes
            .get(id.index())
            .filter(|slot| slot.alive)
            .map(|slot| slot.data.children.as_slice())
            .unwrap_or(&[])
    }

    fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        self.nodes
            .get(id.index())
            .filter(|slot| slot.alive)
            .and_then(|slot| slot.data.parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_proto::{
        AppsAction, Edges, LayoutBox, Point, Rect, ShellAction, StatePatch, StatePath, SysAction,
        WmAction, WorkspaceTarget,
    };

    /// Mock `ReactiveStore` for testing `html-dom` directives and event dispatch independently.
    #[derive(Default)]
    struct MockStore {
        root: BTreeMap<String, StateValue>,
    }

    impl MockStore {
        fn new() -> Self {
            Self::default()
        }

        fn insert_path(&mut self, path: &str, value: StateValue) {
            let parts: Vec<&str> = path.split('.').collect();
            Self::insert_nested(&mut self.root, &parts, value);
        }

        fn insert_nested(
            map: &mut BTreeMap<String, StateValue>,
            parts: &[&str],
            value: StateValue,
        ) {
            if parts.is_empty() {
                return;
            }
            if parts.len() == 1 {
                map.insert(parts[0].to_string(), value);
                return;
            }
            let entry = map
                .entry(parts[0].to_string())
                .or_insert_with(|| StateValue::Map(BTreeMap::new()));
            if let StateValue::Map(sub) = entry {
                Self::insert_nested(sub, &parts[1..], value);
            }
        }
    }

    impl ReactiveStore for MockStore {
        fn get(&self, path: &str) -> Option<StateValue> {
            let root_val = StateValue::Map(self.root.clone());
            root_val.get_path(path).cloned()
        }

        fn set(&mut self, path: &str, value: StateValue) -> Vec<StatePath> {
            self.insert_path(path, value);
            vec![StatePath::new(path)]
        }

        fn apply_patch(&mut self, patch: StatePatch) -> Vec<StatePath> {
            match patch {
                StatePatch::Set { path, value } => self.set(&path, value),
                _ => Vec::new(),
            }
        }

        fn evaluate_expr(
            &self,
            expr: &str,
            local_scope: Option<&BTreeMap<String, StateValue>>,
        ) -> Result<StateValue, String> {
            let trimmed = expr.trim();
            if let Some( inner) = trimmed.strip_prefix('!') {
                let v = self.evaluate_expr(inner, local_scope)?;
                return Ok(StateValue::Bool(!v.as_bool()));
            }
            if let Some(scope) = local_scope {
                if let Some(v) = scope.get(trimmed) {
                    return Ok(v.clone());
                }
                if let Some((head, tail)) = trimmed.split_once('.') {
                    if let Some(root_item) = scope.get(head) {
                        return Ok(root_item.get_path(tail).cloned().unwrap_or(StateValue::Null));
                    }
                }
            }
            Ok(self.get(trimmed).unwrap_or(StateValue::Null))
        }

        fn interpolate_string(
            &self,
            template: &str,
            local_scope: Option<&BTreeMap<String, StateValue>>,
        ) -> Result<String, String> {
            let mut out = String::new();
            let mut rest = template;
            while let Some(start) = rest.find("{{") {
                out.push_str(&rest[..start]);
                let after = &rest[start + 2..];
                if let Some(end) = after.find("}}") {
                    let expr = after[..end].trim();
                    let val = self.evaluate_expr(expr, local_scope)?;
                    out.push_str(&val.to_display_string());
                    rest = &after[end + 2..];
                } else {
                    out.push_str(&rest[start..]);
                    return Ok(out);
                }
            }
            out.push_str(rest);
            Ok(out)
        }

        fn snapshot(&self) -> StateValue {
            StateValue::Map(self.root.clone())
        }
    }

    #[test]
    fn test_html_parsing_void_tags_comments_and_metadata() {
        let html = r#"
            <!DOCTYPE html>
            <!-- Top bar shell surface -->
            <html>
                <head>
                    <meta name="shell-surface" content="top-panel" data-layer="top" data-anchor="top,left,right" data-height="38" data-exclusive-zone="38" data-keyboard-mode="none" />
                    <style>.bar { background: #111; }</style>
                </head>
                <body id="panel-body" class="bar dark-theme" style="height: 38px; --accent: #38bdf8;">
                    <div class="left">
                        <img src="logo.png" alt="Logo">
                        <input type="text" placeholder="Search...">
                        <br>
                        <hr />
                        <svg viewBox="0 0 24 24">
                            <path d="M0 0h24v24H0z">
                        </svg>
                        <span class="title">Lumina &amp; Shell</span>
                    </div>
                </body>
            </html>
        "#;

        let doc = DomDocument::parse(html);
        assert_eq!(doc.metadata.name, "top-panel");
        assert_eq!(doc.metadata.layer, "top");
        assert_eq!(doc.metadata.anchor, vec!["top", "left", "right"]);
        assert_eq!(doc.metadata.height, Some(38));
        assert_eq!(doc.metadata.exclusive_zone, 38);
        assert_eq!(doc.metadata.keyboard_mode, "none");
        assert!(doc.metadata.visible_by_default);
        assert_eq!(doc.embedded_styles.len(), 1);

        let root = doc.get_node(doc.root_id()).expect("root node");
        assert_eq!(root.tag, "body");
        assert_eq!(root.element_id.as_deref(), Some("panel-body"));
        assert!(root.has_class("bar"));
        assert!(root.has_class("dark-theme"));
        assert_eq!(
            root.inline_styles.get("height").map(|s| s.as_str()),
            Some("38px")
        );
        assert_eq!(
            root.inline_styles.get("--accent").map(|s| s.as_str()),
            Some("#38bdf8")
        );

        // Verify void tags did not swallow subsequent siblings
        let left_id = root.children[0];
        let left_children = doc.children_of(left_id);
        // Expected children of .left: img, input, br, hr, svg, span
        assert_eq!(left_children.len(), 6);
        assert_eq!(doc.get_node(left_children[0]).unwrap().tag, "img");
        assert_eq!(doc.get_node(left_children[1]).unwrap().tag, "input");
        assert_eq!(doc.get_node(left_children[2]).unwrap().tag, "br");
        assert_eq!(doc.get_node(left_children[3]).unwrap().tag, "hr");
        assert_eq!(doc.get_node(left_children[4]).unwrap().tag, "svg");
        assert_eq!(doc.get_node(left_children[5]).unwrap().tag, "span");
        assert_eq!(doc.inner_text(left_children[5]), "Lumina & Shell");
    }

    #[test]
    fn test_body_data_attributes_metadata_and_custom_wm_tags() {
        let html = r#"
            <body data-name="osd" data-layer="overlay" data-anchor="bottom" data-width="300" data-height="64" data-exclusive-zone="0" data-keyboard-mode="on-demand" data-visible="false">
                <window id="win-1">
                    <titlebar class="title">Terminal</titlebar>
                </window>
                <workspace-bar id="ws-bar"></workspace-bar>
            </body>
        "#;

        let doc = DomDocument::parse(html);
        assert_eq!(doc.metadata.name, "osd");
        assert_eq!(doc.metadata.layer, "overlay");
        assert_eq!(doc.metadata.anchor, vec!["bottom"]);
        assert_eq!(doc.metadata.width, Some(300));
        assert_eq!(doc.metadata.height, Some(64));
        assert_eq!(doc.metadata.exclusive_zone, 0);
        assert_eq!(doc.metadata.keyboard_mode, "on-demand");
        assert!(!doc.metadata.visible_by_default);

        assert!(doc.find_by_id("win-1").is_some());
        assert!(doc.find_by_id("ws-bar").is_some());
        assert_eq!(doc.find_by_tag("titlebar").len(), 1);
    }

    #[test]
    fn test_reactive_interpolation_bindings_classes_styles_and_visibility() {
        let html = r#"
            <div id="status">
                <span id="clock">Time: {{ clock.time }}</span>
                <span id="cpu-text" data-bind="sys.cpu.label"></span>
                <input id="search-input" data-bind="apps.query" />
                <div id="meter"
                     :class.high="sys.cpu.high"
                     data-class-critical="sys.cpu.critical"
                     :style.width="{{ sys.cpu.usage }}%"
                     data-bind-style---cpu-pct="sys.cpu.usage"
                     :title="sys.cpu.tooltip"
                     data-bind-attr-aria-label="sys.cpu.tooltip">
                </div>
                <div id="bat-warning" data-if="sys.battery.low" data-show="!sys.battery.charging">
                    Low Battery!
                </div>
            </div>
        "#;

        let mut doc = DomDocument::parse(html);
        doc.clear_dirty();

        let mut store = MockStore::new();
        store.insert_path("clock.time", StateValue::from("14:32"));
        store.insert_path("sys.cpu.label", StateValue::from("CPU 42%"));
        store.insert_path("apps.query", StateValue::from("firefox"));
        store.insert_path("sys.cpu.high", StateValue::from(true));
        store.insert_path("sys.cpu.critical", StateValue::from(false));
        store.insert_path("sys.cpu.usage", StateValue::from(42));
        store.insert_path("sys.cpu.tooltip", StateValue::from("8 cores active"));
        store.insert_path("sys.battery.low", StateValue::from(true));
        store.insert_path("sys.battery.charging", StateValue::from(false));

        let flags = doc.sync_reactive_state(&store);
        assert!(flags.contains(DirtyFlags::LAYOUT | DirtyFlags::PAINT));

        let clock_id = doc.find_by_id("clock").unwrap();
        assert_eq!(doc.inner_text(clock_id), "Time: 14:32");

        let cpu_text_id = doc.find_by_id("cpu-text").unwrap();
        assert_eq!(doc.inner_text(cpu_text_id), "CPU 42%");

        let input_id = doc.find_by_id("search-input").unwrap();
        assert_eq!(
            doc.get_node(input_id).unwrap().get_attr("value"),
            Some("firefox")
        );

        let meter_id = doc.find_by_id("meter").unwrap();
        let meter = doc.get_node(meter_id).unwrap();
        assert!(meter.has_class("high"));
        assert!(!meter.has_class("critical"));
        assert_eq!(
            meter.inline_styles.get("width").map(|s| s.as_str()),
            Some("42%")
        );
        assert_eq!(
            meter.inline_styles.get("--cpu-pct").map(|s| s.as_str()),
            Some("42")
        );
        assert_eq!(meter.get_attr("title"), Some("8 cores active"));
        assert_eq!(meter.get_attr("aria-label"), Some("8 cores active"));

        let warn_id = doc.find_by_id("bat-warning").unwrap();
        assert!(doc.get_node(warn_id).unwrap().visible);

        // Now start charging -> bat-warning should become invisible
        store.insert_path("sys.battery.charging", StateValue::from(true));
        store.insert_path("sys.cpu.high", StateValue::from(false));
        store.insert_path("sys.cpu.critical", StateValue::from(true));
        doc.sync_reactive_state(&store);

        assert!(!doc.get_node(warn_id).unwrap().visible);
        let meter = doc.get_node(meter_id).unwrap();
        assert!(!meter.has_class("high"));
        assert!(meter.has_class("critical"));
    }

    #[test]
    fn test_data_for_keyed_reconciliation_and_scoped_events() {
        let html = r#"
            <div id="ws-container" data-for="ws in wm.workspaces" data-key="ws.idx">
                <button class="ws-btn"
                        :class.active="ws.is_active"
                        :class.urgent="ws.is_urgent"
                        on-click="wm:focus-workspace({{ ws.idx }})">
                    <span>{{ ws.idx }}: {{ ws.name }}</span>
                </button>
            </div>
        "#;

        let mut doc = DomDocument::parse(html);
        let mut store = MockStore::new();

        let make_ws = |idx: i64, name: &str, active: bool, urgent: bool| -> StateValue {
            let mut m = BTreeMap::new();
            m.insert("idx".to_string(), StateValue::Int(idx));
            m.insert("name".to_string(), StateValue::String(name.to_string()));
            m.insert("is_active".to_string(), StateValue::Bool(active));
            m.insert("is_urgent".to_string(), StateValue::Bool(urgent));
            StateValue::Map(m)
        };

        store.insert_path(
            "wm.workspaces",
            StateValue::List(vec![
                make_ws(1, "term", true, false),
                make_ws(2, "web", false, true),
                make_ws(3, "code", false, false),
            ]),
        );

        doc.sync_reactive_state(&store);

        let container_id = doc.find_by_id("ws-container").unwrap();
        let children_initial = doc.children_of(container_id).to_vec();
        assert_eq!(children_initial.len(), 3);

        let btn1 = children_initial[0];
        let btn2 = children_initial[1];
        let btn3 = children_initial[2];

        assert_eq!(doc.inner_text(btn1), "1: term");
        assert!(doc.get_node(btn1).unwrap().has_class("active"));
        assert!(!doc.get_node(btn1).unwrap().has_class("urgent"));

        assert_eq!(doc.inner_text(btn2), "2: web");
        assert!(!doc.get_node(btn2).unwrap().has_class("active"));
        assert!(doc.get_node(btn2).unwrap().has_class("urgent"));

        // Dispatch click on the nested <span> inside btn2 -> should bubble to btn2 and resolve {{ ws.idx }} = 2
        let span2_id = doc.children_of(btn2)[0];
        let cmds = doc.dispatch_dom_event(span2_id, DomEventKind::Click, &store);
        assert_eq!(
            cmds,
            vec![ActionCommand::Wm(WmAction::FocusWorkspace {
                target: WorkspaceTarget::Index(2)
            })]
        );

        // Update state: workspace 2 becomes active, workspace 1 inactive, workspace 4 added
        store.insert_path(
            "wm.workspaces",
            StateValue::List(vec![
                make_ws(1, "term", false, false),
                make_ws(2, "web-updated", true, false),
                make_ws(3, "code", false, false),
                make_ws(4, "music", false, false),
            ]),
        );

        doc.sync_reactive_state(&store);

        let children_after = doc.children_of(container_id).to_vec();
        assert_eq!(children_after.len(), 4);
        // Keyed reconciliation preserves NodeIds for existing keys 1, 2, 3
        assert_eq!(children_after[0], btn1);
        assert_eq!(children_after[1], btn2);
        assert_eq!(children_after[2], btn3);

        assert!(!doc.get_node(btn1).unwrap().has_class("active"));
        assert!(doc.get_node(btn2).unwrap().has_class("active"));
        assert_eq!(doc.inner_text(btn2), "2: web-updated");
        assert_eq!(doc.inner_text(children_after[3]), "4: music");
    }

    #[test]
    fn test_hover_chain_and_pseudo_states() {
        let html = r#"
            <div id="root">
                <div id="group">
                    <button id="btn-a">A</button>
                    <button id="btn-b">B</button>
                </div>
            </div>
        "#;

        let mut doc = DomDocument::parse(html);
        let root_id = doc.find_by_id("root").unwrap();
        let group_id = doc.find_by_id("group").unwrap();
        let btn_a = doc.find_by_id("btn-a").unwrap();
        let btn_b = doc.find_by_id("btn-b").unwrap();

        assert!(doc.update_hover_chain(Some(btn_a)));
        assert!(doc
            .get_node(btn_a)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));
        assert!(doc
            .get_node(group_id)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));
        assert!(doc
            .get_node(root_id)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));
        assert!(!doc
            .get_node(btn_b)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));

        // Calling again with same node returns false
        assert!(!doc.update_hover_chain(Some(btn_a)));

        // Switching hover to btn_b clears btn_a and sets btn_b
        assert!(doc.update_hover_chain(Some(btn_b)));
        assert!(!doc
            .get_node(btn_a)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));
        assert!(doc
            .get_node(btn_b)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));
        assert!(doc
            .get_node(group_id)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));

        // Clearing hover
        assert!(doc.update_hover_chain(None));
        assert!(!doc
            .get_node(btn_b)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));
        assert!(!doc
            .get_node(root_id)
            .unwrap()
            .pseudo_states
            .contains(PseudoStates::HOVER));
        assert!(!doc.update_hover_chain(None));
    }

    #[test]
    fn test_event_bubbling_stop_modifier_and_input_keydown_events() {
        let html = r#"
            <div id="backdrop" on-click="shell:close-widget('launcher')">
                <div id="modal" on-click.stop="">
                    <input id="query"
                           on-input="apps:set-query('$event.value')"
                           on-keydown-enter="apps:launch-selected; shell:close-widget('launcher')"
                           on-keydown-escape.stop="shell:close-widget('launcher')" />
                    <button id="bubble-btn" on-click="apps:launch('alacritty')">
                        Launch
                    </button>
                    <button id="stop-btn"
                            on-click.stop="apps:launch('firefox')"
                            on-scroll-up="sys:change-volume(5)"
                            on-scroll-down="sys:change-volume(-5)">
                        Launch Only
                    </button>
                </div>
            </div>
        "#;

        let doc = DomDocument::parse(html);
        let store = MockStore::new();

        let backdrop = doc.find_by_id("backdrop").unwrap();
        let query = doc.find_by_id("query").unwrap();
        let bubble_btn = doc.find_by_id("bubble-btn").unwrap();
        let stop_btn = doc.find_by_id("stop-btn").unwrap();

        // Clicking backdrop directly closes widget
        let cmds = doc.dispatch_dom_event(backdrop, DomEventKind::Click, &store);
        assert_eq!(
            cmds,
            vec![ActionCommand::Shell(ShellAction::CloseWidget {
                name: "launcher".to_string()
            })]
        );

        // Clicking bubble-btn triggers its own action, then bubbles to #modal which has `on-click.stop=""`, stopping before #backdrop!
        let cmds = doc.dispatch_dom_event(bubble_btn, DomEventKind::Click, &store);
        assert_eq!(
            cmds,
            vec![ActionCommand::Apps(AppsAction::Launch {
                app_id: "alacritty".to_string()
            })]
        );

        // Clicking stop-btn stops immediately at stop-btn
        let cmds = doc.dispatch_dom_event(stop_btn, DomEventKind::Click, &store);
        assert_eq!(
            cmds,
            vec![ActionCommand::Apps(AppsAction::Launch {
                app_id: "firefox".to_string()
            })]
        );

        // Scroll events on stop-btn
        let cmds_up = doc.dispatch_dom_event(stop_btn, DomEventKind::ScrollUp, &store);
        assert_eq!(
            cmds_up,
            vec![ActionCommand::Sys(SysAction::ChangeVolume { delta: 5 })]
        );

        // Input event with $event.value interpolation
        let cmds_in = doc.dispatch_dom_event(
            query,
            DomEventKind::Input {
                value: "term".to_string(),
            },
            &store,
        );
        assert_eq!(
            cmds_in,
            vec![ActionCommand::Apps(AppsAction::SetQuery {
                query: "term".to_string()
            })]
        );

        // KeyDown Enter event
        let cmds_key = doc.dispatch_dom_event(
            query,
            DomEventKind::KeyDown {
                key: "Enter".to_string(),
            },
            &store,
        );
        assert_eq!(
            cmds_key,
            vec![
                ActionCommand::Apps(AppsAction::LaunchSelected),
                ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "launcher".to_string()
                }),
            ]
        );
    }

    #[test]
    fn test_export_element_info_and_mutations() {
        let html = r#"
            <div id="panel" class="bar">
                <span id="label">Initial</span>
            </div>
        "#;

        let mut doc = DomDocument::parse(html);
        let panel_id = doc.find_by_id("panel").unwrap();
        let label_id = doc.find_by_id("label").unwrap();

        doc.add_class(panel_id, "elevated");
        assert!(doc.toggle_class(panel_id, "pinned"));
        assert!(!doc.toggle_class(panel_id, "pinned"));
        doc.set_inline_style(panel_id, "opacity", "0.95");
        doc.set_text(label_id, "Updated Text");
        doc.set_pseudo_state(label_id, PseudoStates::URGENT, true);

        let mut layouts = HashMap::new();
        layouts.insert(
            panel_id,
            LayoutBox {
                border_box: Rect::new(0.0, 0.0, 1920.0, 42.0),
                content_box: Rect::new(0.0, 0.0, 1920.0, 42.0),
                padding: Edges::ZERO,
                border: Edges::ZERO,
                margin: Edges::ZERO,
                z_index: 10,
            },
        );

        let info = doc.export_element_info("panel", panel_id, Some(&layouts), Some(2));
        assert_eq!(info.surface, "panel");
        assert_eq!(info.tag, "div");
        assert_eq!(info.id.as_deref(), Some("panel"));
        assert_eq!(info.classes, vec!["bar", "elevated"]);
        assert_eq!(info.layout_box, Some(Rect::new(0.0, 0.0, 1920.0, 42.0)));
        assert_eq!(info.children.len(), 1);
        assert_eq!(info.children[0].text_content, "Updated Text");
        assert_eq!(info.children[0].pseudo_states, vec!["urgent"]);
        assert_eq!(
            Point::new(10.0, 10.0),
            Point { x: 10.0, y: 10.0 }
        );
    }

    #[test]
    fn test_vue_style_data_for_reordering_and_idempotent_dirty_flags() {
        let html = r#"
            <div id="list">
                <button class="item" data-for="item in items" data-key="item.id" :class.selected="item.sel">
                    {{ item.label }}
                </button>
            </div>
        "#;

        let mut doc = DomDocument::parse(html);
        let mut store = MockStore::new();

        let make_item = |id: &str, label: &str, sel: bool| -> StateValue {
            let mut m = BTreeMap::new();
            m.insert("id".to_string(), StateValue::String(id.to_string()));
            m.insert("label".to_string(), StateValue::String(label.to_string()));
            m.insert("sel".to_string(), StateValue::Bool(sel));
            StateValue::Map(m)
        };

        store.insert_path(
            "items",
            StateValue::List(vec![
                make_item("a", "Alpha", true),
                make_item("b", "Beta", false),
            ]),
        );

        let flags1 = doc.sync_reactive_state(&store);
        assert_eq!(flags1, DirtyFlags::ALL);
        doc.clear_dirty();

        // Syncing again without state changes should yield empty DirtyFlags
        let flags2 = doc.sync_reactive_state(&store);
        assert_eq!(flags2, DirtyFlags::empty());

        let list_id = doc.find_by_id("list").unwrap();
        let initial_children = doc.children_of(list_id).to_vec();
        assert_eq!(initial_children.len(), 2);
        let id_a = initial_children[0];
        let id_b = initial_children[1];

        // Swap order of 'a' and 'b'
        store.insert_path(
            "items",
            StateValue::List(vec![
                make_item("b", "Beta", true),
                make_item("a", "Alpha", false),
            ]),
        );

        let flags3 = doc.sync_reactive_state(&store);
        assert!(flags3.contains(DirtyFlags::LAYOUT));

        let reordered = doc.children_of(list_id).to_vec();
        assert_eq!(reordered, vec![id_b, id_a]);
        assert!(doc.get_node(id_b).unwrap().has_class("selected"));
        assert!(!doc.get_node(id_a).unwrap().has_class("selected"));
    }
}
