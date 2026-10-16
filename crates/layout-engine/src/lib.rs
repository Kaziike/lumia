pub use css_style;
pub use wm_proto;
pub use wm_proto::dom::{DomNodeData, DomNodeKind, DomTreeView, NodeId};
pub use wm_proto::layout::{LayoutBox, LayoutSolver, Point, Rect, Size};
pub use wm_proto::style::{
    AlignItems, ComputedStyle, Dimension, Display, Edges, FlexDirection, FlexWrap, JustifyContent,
    Overflow, Position,
};

use std::collections::HashMap;
use taffy::prelude::*;

pub const GLYPH_ADVANCE_FACTOR: f32 = 0.56;

#[derive(Clone, Debug)]
struct TextMeasureContext {
    text: String,
    font_size: f32,
    line_height: f32,
}

/// Measures intrinsic text geometry using average glyph advance (`font_size * 0.56`)
/// and word-wrapping when constrained by `max_width`.
pub fn measure_text_intrinsic(
    text: &str,
    font_size: f32,
    line_height: f32,
    max_width: Option<f32>,
) -> Size {
    if text.is_empty() {
        return Size::new(0.0, 0.0);
    }

    let char_width = (font_size * GLYPH_ADVANCE_FACTOR).max(0.1);
    let line_h = (font_size * line_height).max(1.0);

    let paragraphs: Vec<&str> = text.split('\n').collect();
    let mut unconstrained_max_w = 0.0f32;
    for p in &paragraphs {
        let w = p.chars().count() as f32 * char_width;
        if w > unconstrained_max_w {
            unconstrained_max_w = w;
        }
    }

    let Some(avail_w) = max_width else {
        let lines = paragraphs.len().max(1);
        return Size::new(unconstrained_max_w, lines as f32 * line_h);
    };

    if avail_w <= 0.0 || unconstrained_max_w <= avail_w {
        let lines = paragraphs.len().max(1);
        return Size::new(unconstrained_max_w, lines as f32 * line_h);
    }

    let mut total_lines = 0usize;
    let mut max_line_width = 0.0f32;

    for para in paragraphs {
        let words: Vec<&str> = para.split_whitespace().collect();
        if words.is_empty() {
            total_lines += 1;
            continue;
        }

        let mut current_chars = 0usize;
        for word in words {
            let word_chars = word.chars().count();
            if current_chars == 0 {
                current_chars = word_chars;
                let w = (current_chars as f32 * char_width).min(avail_w);
                if w > max_line_width {
                    max_line_width = w;
                }
            } else {
                let candidate_chars = current_chars + 1 + word_chars;
                let candidate_w = candidate_chars as f32 * char_width;
                if candidate_w <= avail_w {
                    current_chars = candidate_chars;
                    if candidate_w > max_line_width {
                        max_line_width = candidate_w;
                    }
                } else {
                    total_lines += 1;
                    current_chars = word_chars;
                    let w = (current_chars as f32 * char_width).min(avail_w);
                    if w > max_line_width {
                        max_line_width = w;
                    }
                }
            }
        }

        if current_chars > 0 {
            total_lines += 1;
            let w = (current_chars as f32 * char_width).min(avail_w);
            if w > max_line_width {
                max_line_width = w;
            }
        }
    }

    let final_lines = total_lines.max(1);
    Size::new(max_line_width, final_lines as f32 * line_h)
}

/// Taffy-backed Flexbox, CSS Grid, Block, and Absolute Layout Engine + Hit-Tester.
#[derive(Clone, Debug, Default)]
pub struct TaffyLayoutEngine;

impl TaffyLayoutEngine {
    pub fn new() -> Self {
        Self
    }

    fn resolve_node_style(
        dom: &dyn DomTreeView,
        node: &DomNodeData,
        styles: &HashMap<NodeId, ComputedStyle>,
    ) -> ComputedStyle {
        if let Some(s) = styles.get(&node.id) {
            let mut st = s.clone();
            if !node.visible {
                st.display = Display::None;
            }
            return st;
        }

        // Fallback for text nodes or unstyled nodes: inherit font metrics from parent
        let mut fallback = ComputedStyle::default();
        let parent_id = dom.parent_of(node.id).or(node.parent);
        if let Some(pid) = parent_id {
            if let Some(ps) = styles.get(&pid) {
                fallback.font_size = ps.font_size;
                fallback.line_height = ps.line_height;
                fallback.font_family = ps.font_family.clone();
                fallback.font_weight = ps.font_weight;
                fallback.color = ps.color;
            }
        }
        if !node.visible {
            fallback.display = Display::None;
        }
        fallback
    }

    fn build_taffy_node(
        dom: &dyn DomTreeView,
        node_id: NodeId,
        is_root: bool,
        viewport: Size,
        styles: &HashMap<NodeId, ComputedStyle>,
        taffy: &mut TaffyTree<TextMeasureContext>,
        node_map: &mut HashMap<NodeId, taffy::NodeId>,
        resolved_styles: &mut HashMap<NodeId, ComputedStyle>,
    ) -> Option<taffy::NodeId> {
        let node = dom.get_node(node_id)?;
        let style = Self::resolve_node_style(dom, node, styles);
        let child_dom_ids: Vec<NodeId> = if !dom.children_of(node_id).is_empty() {
            dom.children_of(node_id).to_vec()
        } else {
            node.children.clone()
        };

        let mut child_taffy_ids = Vec::with_capacity(child_dom_ids.len());
        for child_id in child_dom_ids {
            if let Some(cid) = Self::build_taffy_node(
                dom,
                child_id,
                false,
                viewport,
                styles,
                taffy,
                node_map,
                resolved_styles,
            ) {
                child_taffy_ids.push(cid);
            }
        }

        let is_text_leaf = child_taffy_ids.is_empty()
            && node
                .text_content
                .as_ref()
                .map(|t| !t.is_empty())
                .unwrap_or(false);

        let taffy_style = to_taffy_style(&style, is_root && !is_text_leaf, viewport);

        let taffy_id = if child_taffy_ids.is_empty() {
            if let Some(ref txt) = node.text_content {
                if !txt.is_empty() {
                    let ctx = TextMeasureContext {
                        text: txt.clone(),
                        font_size: style.font_size,
                        line_height: style.line_height,
                    };
                    taffy.new_leaf_with_context(taffy_style, ctx).ok()?
                } else {
                    taffy.new_leaf(taffy_style).ok()?
                }
            } else {
                taffy.new_leaf(taffy_style).ok()?
            }
        } else {
            taffy
                .new_with_children(taffy_style, &child_taffy_ids)
                .ok()?
        };

        node_map.insert(node_id, taffy_id);
        resolved_styles.insert(node_id, style);
        Some(taffy_id)
    }

    fn collect_absolute_layouts(
        dom: &dyn DomTreeView,
        node_id: NodeId,
        parent_origin: Point,
        taffy: &TaffyTree<TextMeasureContext>,
        node_map: &HashMap<NodeId, taffy::NodeId>,
        resolved_styles: &HashMap<NodeId, ComputedStyle>,
        out: &mut HashMap<NodeId, LayoutBox>,
    ) {
        let Some(&taffy_id) = node_map.get(&node_id) else {
            return;
        };
        let Ok(layout) = taffy.layout(taffy_id) else {
            return;
        };
        let style = resolved_styles
            .get(&node_id)
            .cloned()
            .unwrap_or_default();

        let abs_x = parent_origin.x + layout.location.x;
        let abs_y = parent_origin.y + layout.location.y;
        let border_box = Rect::new(abs_x, abs_y, layout.size.width, layout.size.height);
        let content_box = border_box
            .inset(style.border_width)
            .inset(style.padding);

        out.insert(
            node_id,
            LayoutBox {
                border_box,
                content_box,
                padding: style.padding,
                border: style.border_width,
                margin: style.margin,
                z_index: style.z_index,
            },
        );

        let child_ids: Vec<NodeId> = if !dom.children_of(node_id).is_empty() {
            dom.children_of(node_id).to_vec()
        } else if let Some(node) = dom.get_node(node_id) {
            node.children.clone()
        } else {
            Vec::new()
        };

        let next_origin = Point::new(abs_x, abs_y);
        for child_id in child_ids {
            Self::collect_absolute_layouts(
                dom,
                child_id,
                next_origin,
                taffy,
                node_map,
                resolved_styles,
                out,
            );
        }
    }
}

impl LayoutSolver for TaffyLayoutEngine {
    fn compute_layout(
        &mut self,
        dom: &dyn DomTreeView,
        styles: &HashMap<NodeId, ComputedStyle>,
        viewport: Size,
    ) -> HashMap<NodeId, LayoutBox> {
        let mut taffy: TaffyTree<TextMeasureContext> = TaffyTree::new();
        let mut node_map: HashMap<NodeId, taffy::NodeId> = HashMap::new();
        let mut resolved_styles: HashMap<NodeId, ComputedStyle> = HashMap::new();

        let root_id = dom.root_id();
        let Some(root_taffy) = Self::build_taffy_node(
            dom,
            root_id,
            true,
            viewport,
            styles,
            &mut taffy,
            &mut node_map,
            &mut resolved_styles,
        ) else {
            return HashMap::new();
        };

        let avail = taffy::Size {
            width: if viewport.width > 0.0 {
                AvailableSpace::Definite(viewport.width)
            } else {
                AvailableSpace::MaxContent
            },
            height: if viewport.height > 0.0 {
                AvailableSpace::Definite(viewport.height)
            } else {
                AvailableSpace::MaxContent
            },
        };

        let _ = taffy.compute_layout_with_measure(
            root_taffy,
            avail,
            |known_dimensions, available_space, _node_id, node_context, _style| {
                let Some(ctx) = node_context else {
                    return taffy::Size::ZERO;
                };

                let max_width = match known_dimensions.width {
                    Some(w) => Some(w),
                    None => match available_space.width {
                        AvailableSpace::Definite(w) => Some(w),
                        AvailableSpace::MinContent => {
                            let longest_word = ctx
                                .text
                                .split_whitespace()
                                .map(|w| w.chars().count())
                                .max()
                                .unwrap_or(1);
                            Some(longest_word as f32 * ctx.font_size * GLYPH_ADVANCE_FACTOR)
                        }
                        AvailableSpace::MaxContent => None,
                    },
                };

                let measured =
                    measure_text_intrinsic(&ctx.text, ctx.font_size, ctx.line_height, max_width);

                taffy::Size {
                    width: known_dimensions.width.unwrap_or(measured.width),
                    height: known_dimensions.height.unwrap_or(measured.height),
                }
            },
        );

        let mut out = HashMap::with_capacity(node_map.len());
        Self::collect_absolute_layouts(
            dom,
            root_id,
            Point::new(0.0, 0.0),
            &taffy,
            &node_map,
            &resolved_styles,
            &mut out,
        );
        out
    }

    fn hit_test(
        &self,
        dom: &dyn DomTreeView,
        layouts: &HashMap<NodeId, LayoutBox>,
        point: Point,
    ) -> Option<NodeId> {
        #[derive(Clone, Copy, Debug)]
        struct HitCandidate {
            node_id: NodeId,
            is_element: bool,
            effective_z: i32,
            paint_order: usize,
        }

        fn visit(
            dom: &dyn DomTreeView,
            layouts: &HashMap<NodeId, LayoutBox>,
            node_id: NodeId,
            parent_effective_z: i32,
            point: Point,
            paint_counter: &mut usize,
            candidates: &mut Vec<HitCandidate>,
        ) {
            let Some(node) = dom.get_node(node_id) else {
                return;
            };
            if !node.visible {
                return;
            }
            let Some(layout) = layouts.get(&node_id) else {
                return;
            };
            if layout.border_box.width <= 0.0 || layout.border_box.height <= 0.0 {
                return;
            }

            let effective_z = if layout.z_index != 0 {
                parent_effective_z + layout.z_index
            } else {
                parent_effective_z
            };

            let current_order = *paint_counter;
            *paint_counter += 1;

            if layout.border_box.contains(point) {
                candidates.push(HitCandidate {
                    node_id,
                    is_element: node.kind == DomNodeKind::Element,
                    effective_z,
                    paint_order: current_order,
                });
            }

            let mut children: Vec<NodeId> = if !dom.children_of(node_id).is_empty() {
                dom.children_of(node_id).to_vec()
            } else {
                node.children.clone()
            };

            // Stable sort children by z_index so children with higher z_index paint after lower ones
            children.sort_by_key(|cid| layouts.get(cid).map(|l| l.z_index).unwrap_or(0));

            for child_id in children {
                visit(
                    dom,
                    layouts,
                    child_id,
                    effective_z,
                    point,
                    paint_counter,
                    candidates,
                );
            }
        }

        let mut paint_counter = 0usize;
        let mut candidates = Vec::new();
        visit(
            dom,
            layouts,
            dom.root_id(),
            0,
            point,
            &mut paint_counter,
            &mut candidates,
        );

        // Prefer Element nodes over raw Text nodes, sorted by (effective_z, paint_order)
        if let Some(best_elem) = candidates
            .iter()
            .filter(|c| c.is_element)
            .max_by_key(|c| (c.effective_z, c.paint_order))
        {
            return Some(best_elem.node_id);
        }

        candidates
            .iter()
            .max_by_key(|c| (c.effective_z, c.paint_order))
            .map(|c| c.node_id)
    }
}

fn to_taffy_dimension(d: Dimension) -> taffy::Dimension {
    match d {
        Dimension::Auto => taffy::Dimension::Auto,
        Dimension::Px(v) => length(v),
        Dimension::Percent(p) => {
            let ratio = if p.abs() > 1.0 { p / 100.0 } else { p };
            percent(ratio)
        }
    }
}

fn to_taffy_lpa(d: Dimension) -> taffy::LengthPercentageAuto {
    match d {
        Dimension::Auto => taffy::LengthPercentageAuto::Auto,
        Dimension::Px(v) => taffy::LengthPercentageAuto::Length(v),
        Dimension::Percent(p) => {
            let ratio = if p.abs() > 1.0 { p / 100.0 } else { p };
            taffy::LengthPercentageAuto::Percent(ratio)
        }
    }
}

fn to_taffy_style(
    style: &ComputedStyle,
    fill_root_viewport_width: bool,
    viewport: Size,
) -> taffy::Style {
    let display = match style.display {
        Display::Flex => taffy::Display::Flex,
        Display::Grid => taffy::Display::Grid,
        Display::Block => taffy::Display::Block,
        Display::None => taffy::Display::None,
    };

    let position = match style.position {
        Position::Relative => taffy::Position::Relative,
        Position::Absolute => taffy::Position::Absolute,
    };

    let flex_direction = match style.flex_direction {
        FlexDirection::Row => taffy::FlexDirection::Row,
        FlexDirection::Column => taffy::FlexDirection::Column,
        FlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
        FlexDirection::ColumnReverse => taffy::FlexDirection::ColumnReverse,
    };

    let flex_wrap = match style.flex_wrap {
        FlexWrap::NoWrap => taffy::FlexWrap::NoWrap,
        FlexWrap::Wrap => taffy::FlexWrap::Wrap,
    };

    let justify_content = Some(match style.justify_content {
        JustifyContent::FlexStart => taffy::JustifyContent::FlexStart,
        JustifyContent::FlexEnd => taffy::JustifyContent::FlexEnd,
        JustifyContent::Center => taffy::JustifyContent::Center,
        JustifyContent::SpaceBetween => taffy::JustifyContent::SpaceBetween,
        JustifyContent::SpaceAround => taffy::JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly => taffy::JustifyContent::SpaceEvenly,
    });

    let align_items = Some(match style.align_items {
        AlignItems::Stretch => taffy::AlignItems::Stretch,
        AlignItems::FlexStart => taffy::AlignItems::FlexStart,
        AlignItems::FlexEnd => taffy::AlignItems::FlexEnd,
        AlignItems::Center => taffy::AlignItems::Center,
        AlignItems::Baseline => taffy::AlignItems::Baseline,
    });

    let overflow_val = match style.overflow {
        Overflow::Visible => taffy::Overflow::Visible,
        Overflow::Hidden => taffy::Overflow::Hidden,
        Overflow::Scroll => taffy::Overflow::Scroll,
    };

    let width = if fill_root_viewport_width && style.width == Dimension::Auto && viewport.width > 0.0
    {
        length((viewport.width - style.margin.left - style.margin.right).max(0.0))
    } else {
        to_taffy_dimension(style.width)
    };

    let height = to_taffy_dimension(style.height);

    let grid_template_columns = if style.display == Display::Grid && style.grid_columns > 0 {
        vec![flex(1.0_f32); style.grid_columns as usize]
    } else {
        Vec::new()
    };

    taffy::Style {
        display,
        position,
        flex_direction,
        flex_wrap,
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        justify_content,
        align_items,
        gap: taffy::Size {
            width: length(style.gap_column),
            height: length(style.gap_row),
        },
        grid_template_columns,
        size: taffy::Size { width, height },
        min_size: taffy::Size {
            width: to_taffy_dimension(style.min_width),
            height: to_taffy_dimension(style.min_height),
        },
        max_size: taffy::Size {
            width: to_taffy_dimension(style.max_width),
            height: to_taffy_dimension(style.max_height),
        },
        padding: taffy::Rect {
            top: length(style.padding.top),
            right: length(style.padding.right),
            bottom: length(style.padding.bottom),
            left: length(style.padding.left),
        },
        margin: taffy::Rect {
            top: taffy::LengthPercentageAuto::Length(style.margin.top),
            right: taffy::LengthPercentageAuto::Length(style.margin.right),
            bottom: taffy::LengthPercentageAuto::Length(style.margin.bottom),
            left: taffy::LengthPercentageAuto::Length(style.margin.left),
        },
        border: taffy::Rect {
            top: length(style.border_width.top),
            right: length(style.border_width.right),
            bottom: length(style.border_width.bottom),
            left: length(style.border_width.left),
        },
        inset: taffy::Rect {
            top: to_taffy_lpa(style.inset_top),
            right: to_taffy_lpa(style.inset_right),
            bottom: to_taffy_lpa(style.inset_bottom),
            left: to_taffy_lpa(style.inset_left),
        },
        overflow: taffy::Point {
            x: overflow_val,
            y: overflow_val,
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use css_style::CssStyleEngine;
    use std::collections::BTreeMap;
    use wm_proto::dom::PseudoStates;
    use wm_proto::style::StyleEngine;

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

        fn add(&mut self, node: DomNodeData) {
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

    fn elem(
        id: usize,
        tag: &str,
        el_id: Option<&str>,
        classes: &[&str],
        text: Option<&str>,
        parent: Option<usize>,
        children: &[usize],
    ) -> DomNodeData {
        DomNodeData {
            id: NodeId(id),
            kind: DomNodeKind::Element,
            tag: tag.to_string(),
            element_id: el_id.map(|s| s.to_string()),
            classes: classes.iter().map(|s| s.to_string()).collect(),
            attributes: BTreeMap::new(),
            inline_styles: BTreeMap::new(),
            text_content: text.map(|s| s.to_string()),
            pseudo_states: PseudoStates::empty(),
            visible: true,
            parent: parent.map(NodeId),
            children: children.iter().copied().map(NodeId).collect(),
        }
    }

    #[test]
    fn test_flexbox_space_between_and_content_box_insets() {
        let mut dom = MockDom::new(NodeId(1));
        dom.add(elem(
            1,
            "div",
            Some("topbar"),
            &[],
            None,
            None,
            &[2, 3, 4],
        ));
        dom.add(elem(2, "div", Some("left"), &[], None, Some(1), &[]));
        dom.add(elem(3, "div", Some("center"), &[], None, Some(1), &[]));
        dom.add(elem(4, "div", Some("right"), &[], None, Some(1), &[]));

        let css = r#"
            #topbar {
                display: flex;
                flex-direction: row;
                justify-content: space-between;
                align-items: center;
                width: 1000px;
                height: 40px;
                padding: 4px 10px;
                border: 2px solid #fff;
            }
            #left, #center, #right {
                width: 100px;
                height: 24px;
                padding: 2px 4px;
                border: 1px solid #888;
            }
        "#;

        let style_engine = CssStyleEngine::from_css(css).unwrap();
        let styles = style_engine.compute_styles(&dom);
        let mut layout_engine = TaffyLayoutEngine::new();
        let layouts = layout_engine.compute_layout(&dom, &styles, Size::new(1000.0, 40.0));

        let bar = &layouts[&NodeId(1)];
        assert_eq!(bar.border_box, Rect::new(0.0, 0.0, 1000.0, 40.0));
        // Inset by border (2px) + padding (4px vertical, 10px horizontal) -> x=12, y=6, w=976, h=28
        assert_eq!(bar.content_box, Rect::new(12.0, 6.0, 976.0, 28.0));

        let left = &layouts[&NodeId(2)];
        let center = &layouts[&NodeId(3)];
        let right = &layouts[&NodeId(4)];

        assert!((left.border_box.x - 12.0).abs() < 0.01);
        assert!((left.border_box.y - 8.0).abs() < 0.01);
        assert!(center.border_box.x > left.border_box.right());
        assert!((right.border_box.right() - 988.0).abs() < 0.01);
        assert_eq!(
            left.content_box,
            Rect::new(left.border_box.x + 5.0, left.border_box.y + 3.0, 90.0, 18.0)
        );
    }

    #[test]
    fn test_grid_and_text_measurement_wrapping() {
        let mut dom = MockDom::new(NodeId(1));
        dom.add(elem(
            1,
            "div",
            Some("grid"),
            &[],
            None,
            None,
            &[2, 3, 4, 5],
        ));
        dom.add(elem(2, "div", None, &["cell"], Some("Hello"), Some(1), &[]));
        dom.add(elem(
            3,
            "div",
            None,
            &["cell"],
            Some("A longer piece of wrapped text content here"),
            Some(1),
            &[],
        ));
        dom.add(elem(4, "div", None, &["cell"], Some("Item 3"), Some(1), &[]));
        dom.add(elem(5, "div", None, &["cell"], Some("Item 4"), Some(1), &[]));

        let css = r#"
            #grid {
                display: grid;
                grid-template-columns: repeat(2, 1fr);
                gap: 10px;
                width: 210px;
            }
            .cell {
                font-size: 14px;
                line-height: 1.4;
            }
        "#;

        let style_engine = CssStyleEngine::from_css(css).unwrap();
        let styles = style_engine.compute_styles(&dom);
        let mut layout_engine = TaffyLayoutEngine::new();
        let layouts = layout_engine.compute_layout(&dom, &styles, Size::new(210.0, 400.0));

        // 2 columns of 100px each (with 10px column gap)
        let c1 = &layouts[&NodeId(2)];
        let c2 = &layouts[&NodeId(3)];
        let c3 = &layouts[&NodeId(4)];
        let c4 = &layouts[&NodeId(5)];

        assert!((c1.border_box.x - 0.0).abs() < 0.1);
        assert!((c2.border_box.x - 110.0).abs() < 0.1);
        assert!((c3.border_box.x - 0.0).abs() < 0.1);
        assert!((c4.border_box.x - 110.0).abs() < 0.1);
        assert!(c3.border_box.y > c1.border_box.height);

        // Verify wrapped text height > single line height
        let single_line_h = 14.0 * 1.4;
        assert!(c2.border_box.height >= single_line_h * 2.0 - 0.1);
    }

    #[test]
    fn test_hit_testing_with_z_index_and_visibility() {
        let mut dom = MockDom::new(NodeId(1));
        dom.add(elem(1, "div", Some("root"), &[], None, None, &[2, 3, 4]));
        dom.add(elem(2, "div", Some("overlay"), &[], None, Some(1), &[5]));
        dom.add(elem(3, "div", Some("card"), &[], None, Some(1), &[]));
        let mut hidden = elem(4, "div", Some("hidden-top"), &[], None, Some(1), &[]);
        hidden.visible = false;
        dom.add(hidden);
        dom.add(elem(5, "button", Some("modal-btn"), &[], None, Some(2), &[]));

        let css = r#"
            #root {
                position: relative;
                width: 500px;
                height: 500px;
            }
            #overlay {
                position: absolute;
                top: 50px;
                left: 50px;
                width: 200px;
                height: 200px;
                align-items: flex-start;
                z-index: 100;
            }
            #modal-btn {
                width: 80px;
                height: 30px;
            }
            #card {
                position: absolute;
                top: 50px;
                left: 50px;
                width: 200px;
                height: 200px;
                z-index: 10;
            }
            #hidden-top {
                position: absolute;
                top: 50px;
                left: 50px;
                width: 200px;
                height: 200px;
                z-index: 999;
            }
        "#;

        let style_engine = CssStyleEngine::from_css(css).unwrap();
        let styles = style_engine.compute_styles(&dom);
        let mut layout_engine = TaffyLayoutEngine::new();
        let layouts = layout_engine.compute_layout(&dom, &styles, Size::new(500.0, 500.0));

        // Point inside modal-btn (which is inside #overlay with z-index 100, above #card with z-index 10, while #hidden-top is invisible)
        let hit_btn = layout_engine.hit_test(&dom, &layouts, Point::new(60.0, 60.0));
        assert_eq!(hit_btn, Some(NodeId(5)));

        // Point inside #overlay outside #modal-btn
        let hit_overlay = layout_engine.hit_test(&dom, &layouts, Point::new(180.0, 180.0));
        assert_eq!(hit_overlay, Some(NodeId(2)));

        // Point outside #overlay but inside #root
        let hit_root = layout_engine.hit_test(&dom, &layouts, Point::new(400.0, 400.0));
        assert_eq!(hit_root, Some(NodeId(1)));

        // Point outside #root
        let hit_outside = layout_engine.hit_test(&dom, &layouts, Point::new(600.0, 600.0));
        assert_eq!(hit_outside, None);
    }

    #[test]
    fn test_flex_column_grow_and_wrap() {
        let mut dom = MockDom::new(NodeId(1));
        dom.add(elem(1, "div", Some("col"), &[], None, None, &[2, 3]));
        dom.add(elem(2, "div", Some("header"), &[], None, Some(1), &[]));
        dom.add(elem(3, "div", Some("body"), &[], None, Some(1), &[]));

        let css = r#"
            #col {
                display: flex;
                flex-direction: column;
                align-items: stretch;
                width: 300px;
                height: 200px;
                gap: 10px;
            }
            #header {
                height: 40px;
            }
            #body {
                flex-grow: 1;
            }
        "#;

        let style_engine = CssStyleEngine::from_css(css).unwrap();
        let styles = style_engine.compute_styles(&dom);
        let mut layout_engine = TaffyLayoutEngine::new();
        let layouts = layout_engine.compute_layout(&dom, &styles, Size::new(300.0, 200.0));

        let header = &layouts[&NodeId(2)];
        let body = &layouts[&NodeId(3)];

        assert_eq!(header.border_box, Rect::new(0.0, 0.0, 300.0, 40.0));
        assert_eq!(body.border_box, Rect::new(0.0, 50.0, 300.0, 150.0));
    }
}
