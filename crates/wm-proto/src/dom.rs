use crate::layout::Rect;
use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Unique identifier for a DOM node in a surface document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize {
        self.0
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct PseudoStates: u16 {
        const HOVER         = 1 << 0;
        const ACTIVE        = 1 << 1;
        const FOCUS         = 1 << 2;
        const FOCUS_VISIBLE = 1 << 3;
        const CHECKED       = 1 << 4;
        const DISABLED      = 1 << 5;
        const URGENT        = 1 << 6;
    }
}

impl PseudoStates {
    pub fn to_names(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.contains(Self::HOVER) {
            out.push("hover");
        }
        if self.contains(Self::ACTIVE) {
            out.push("active");
        }
        if self.contains(Self::FOCUS) {
            out.push("focus");
        }
        if self.contains(Self::FOCUS_VISIBLE) {
            out.push("focus-visible");
        }
        if self.contains(Self::CHECKED) {
            out.push("checked");
        }
        if self.contains(Self::DISABLED) {
            out.push("disabled");
        }
        if self.contains(Self::URGENT) {
            out.push("urgent");
        }
        out
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct DirtyFlags: u8 {
        const STYLE  = 1 << 0;
        const LAYOUT = 1 << 1;
        const PAINT  = 1 << 2;
        const ALL    = Self::STYLE.bits() | Self::LAYOUT.bits() | Self::PAINT.bits();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomNodeKind {
    Element,
    Text,
}

/// Shell surface metadata parsed from `<meta name="shell-surface" ...>` or root attributes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceMetadata {
    pub name: String,
    pub layer: String,
    pub anchor: Vec<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub exclusive_zone: i32,
    pub keyboard_mode: String,
    pub visible_by_default: bool,
}

impl Default for SurfaceMetadata {
    fn default() -> Self {
        Self {
            name: "panel".to_string(),
            layer: "top".to_string(),
            anchor: vec!["top".to_string(), "left".to_string(), "right".to_string()],
            width: None,
            height: Some(42),
            exclusive_zone: 42,
            keyboard_mode: "none".to_string(),
            visible_by_default: true,
        }
    }
}

/// Read-only view of a single DOM node for CSS selector matching, layout, and rendering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomNodeData {
    pub id: NodeId,
    pub kind: DomNodeKind,
    pub tag: String,
    pub element_id: Option<String>,
    pub classes: Vec<String>,
    pub attributes: BTreeMap<String, String>,
    pub inline_styles: BTreeMap<String, String>,
    pub text_content: Option<String>,
    pub pseudo_states: PseudoStates,
    pub visible: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

impl DomNodeData {
    pub fn has_class(&self, class_name: &str) -> bool {
        self.classes.iter().any(|c| c == class_name)
    }

    pub fn get_attr(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(|s| s.as_str())
    }
}

/// Serialized summary of a DOM element returned by `luminactl dom query` / `dom tree`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomElementInfo {
    pub surface: String,
    pub node_id: usize,
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: BTreeMap<String, String>,
    pub pseudo_states: Vec<String>,
    pub text_content: String,
    pub visible: bool,
    pub layout_box: Option<Rect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DomElementInfo>,
}

/// Trait implemented by `crates/html-dom`'s `DomDocument` so `css-style`, `layout-engine`,
/// and `renderer` can inspect the DOM without depending on `html-dom` internals.
pub trait DomTreeView {
    fn root_id(&self) -> NodeId;
    fn get_node(&self, id: NodeId) -> Option<&DomNodeData>;
    fn all_node_ids(&self) -> Vec<NodeId>;
    fn children_of(&self, id: NodeId) -> &[NodeId];
    fn parent_of(&self, id: NodeId) -> Option<NodeId>;

    /// Collects concatenated text content of a node and its descendants.
    fn inner_text(&self, id: NodeId) -> String {
        let Some(node) = self.get_node(id) else {
            return String::new();
        };
        if !node.visible {
            return String::new();
        }
        if let Some(txt) = &node.text_content {
            if node.children.is_empty() {
                return txt.clone();
            }
        }
        let mut out = String::new();
        if let Some(txt) = &node.text_content {
            out.push_str(txt);
        }
        for &child_id in &node.children {
            let child_text = self.inner_text(child_id);
            if !child_text.is_empty() {
                out.push_str(&child_text);
            }
        }
        out
    }
}
