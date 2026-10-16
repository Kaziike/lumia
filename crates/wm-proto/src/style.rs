use crate::dom::{DomTreeView, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub fn is_transparent(&self) -> bool {
        self.a == 0
    }

    pub fn to_css_string(&self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!(
                "rgba({}, {}, {}, {:.2})",
                self.r,
                self.g,
                self.b,
                self.a as f32 / 255.0
            )
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "unit", content = "val", rename_all = "snake_case")]
pub enum Dimension {
    Auto,
    Px(f32),
    Percent(f32),
}

impl Default for Dimension {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const ZERO: Self = Self {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    };

    pub const fn all(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub const fn xy(x: f32, y: f32) -> Self {
        Self {
            top: y,
            right: x,
            bottom: y,
            left: x,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Display {
    Flex,
    Grid,
    Block,
    None,
}

impl Default for Display {
    fn default() -> Self {
        Self::Flex
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Relative,
    Absolute,
}

impl Default for Position {
    fn default() -> Self {
        Self::Relative
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlexDirection {
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

impl Default for FlexDirection {
    fn default() -> Self {
        Self::Row
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlexWrap {
    NoWrap,
    Wrap,
}

impl Default for FlexWrap {
    fn default() -> Self {
        Self::NoWrap
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JustifyContent {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

impl Default for JustifyContent {
    fn default() -> Self {
        Self::FlexStart
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignItems {
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
}

impl Default for AlignItems {
    fn default() -> Self {
        Self::Center
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl Default for TextAlign {
    fn default() -> Self {
        Self::Left
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overflow {
    Visible,
    Hidden,
    Scroll,
}

impl Default for Overflow {
    fn default() -> Self {
        Self::Visible
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearGradient {
    pub angle_deg: f32,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoxShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub spread_radius: f32,
    pub color: Color,
    pub inset: bool,
}

/// Complete resolved style for a DOM node (both layout properties and paint properties).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComputedStyle {
    // Layout properties
    pub display: Display,
    pub position: Position,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub gap_row: f32,
    pub gap_column: f32,
    pub grid_columns: u16,
    pub width: Dimension,
    pub height: Dimension,
    pub min_width: Dimension,
    pub min_height: Dimension,
    pub max_width: Dimension,
    pub max_height: Dimension,
    pub padding: Edges,
    pub margin: Edges,
    pub border_width: Edges,
    pub inset_top: Dimension,
    pub inset_right: Dimension,
    pub inset_bottom: Dimension,
    pub inset_left: Dimension,

    // Visual & Paint properties
    pub background_color: Color,
    pub background_gradient: Option<LinearGradient>,
    pub border_color: Color,
    pub border_radius: [f32; 4], // top-left, top-right, bottom-right, bottom-left
    pub box_shadow: Option<BoxShadow>,
    pub color: Color,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u16,
    pub line_height: f32,
    pub text_align: TextAlign,
    pub opacity: f32,
    pub overflow: Overflow,
    pub z_index: i32,
    pub backdrop_blur: f32,

    // Custom properties (`--*`) and `-wm-*` extensions
    pub custom_properties: BTreeMap<String, String>,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            display: Display::Flex,
            position: Position::Relative,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::Center,
            gap_row: 0.0,
            gap_column: 0.0,
            grid_columns: 0,
            width: Dimension::Auto,
            height: Dimension::Auto,
            min_width: Dimension::Auto,
            min_height: Dimension::Auto,
            max_width: Dimension::Auto,
            max_height: Dimension::Auto,
            padding: Edges::ZERO,
            margin: Edges::ZERO,
            border_width: Edges::ZERO,
            inset_top: Dimension::Auto,
            inset_right: Dimension::Auto,
            inset_bottom: Dimension::Auto,
            inset_left: Dimension::Auto,
            background_color: Color::TRANSPARENT,
            background_gradient: None,
            border_color: Color::TRANSPARENT,
            border_radius: [0.0; 4],
            box_shadow: None,
            color: Color::rgb(226, 232, 240), // Default light slate text
            font_family: "sans-serif".to_string(),
            font_size: 14.0,
            font_weight: 400,
            line_height: 1.35,
            text_align: TextAlign::Left,
            opacity: 1.0,
            overflow: Overflow::Visible,
            z_index: 0,
            backdrop_blur: 0.0,
            custom_properties: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchedRuleReport {
    pub selector: String,
    pub specificity: (u16, u16, u16),
    pub declarations: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleInspectionReport {
    pub node_id: usize,
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub matched_rules: Vec<MatchedRuleReport>,
    pub resolved_variables: BTreeMap<String, String>,
    pub computed: ComputedStyle,
}

/// Trait implemented by `crates/css-style`.
pub trait StyleEngine: Send + Sync {
    fn load_stylesheet(&mut self, css_source: &str) -> Result<(), String>;
    fn append_stylesheet(&mut self, css_source: &str) -> Result<(), String>;
    fn set_custom_property(&mut self, name: &str, value: &str);
    fn get_custom_property(&self, name: &str) -> Option<&str>;
    fn compute_styles(&self, dom: &dyn DomTreeView) -> HashMap<NodeId, ComputedStyle>;
    fn inspect_node_style(&self, dom: &dyn DomTreeView, node_id: NodeId) -> StyleInspectionReport;
    fn query_selector_all(&self, dom: &dyn DomTreeView, selector: &str) -> Vec<NodeId>;
}
