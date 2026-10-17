pub mod font;
pub mod icons;
pub mod primitives;
pub mod snapshot;
pub mod window_decoration;

pub use font::FontManager;
pub use icons::{detect_icon_name, draw_vector_icon};
pub use primitives::{build_rounded_rect_path, draw_background, draw_border, draw_box_shadow};
pub use snapshot::{encode_bmp, encode_ppm, save_bmp, save_ppm};
pub use window_decoration::{
    hit_test_titlebar, render_window_decoration, render_window_decoration_with_theme,
    render_window_with_app_content, render_window_with_client_buffer, TitlebarHit,
    WindowDecorationTheme,
};
pub use wm_proto;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use tiny_skia::Pixmap;
use wm_proto::{
    Color, ComputedStyle, Display, DomNodeData, DomNodeKind, DomTreeView, FrameBuffer, LayoutBox,
    NodeId, Overflow, Rect, Size, SurfaceRenderer, WindowPlacement,
};

/// Pure-Rust 2D Vector, Typography, SVG Icon & Window Decoration Renderer
/// powered by `tiny-skia` and `ab_glyph`.
#[derive(Clone, Default)]
pub struct SkiaRenderer {
    font_manager: FontManager,
}

#[derive(Clone, Debug)]
struct PaintItem {
    node_id: NodeId,
    z_index: i32,
    tree_order: usize,
    opacity: f32,
    clip_rect: Option<Rect>,
}

impl SkiaRenderer {
    /// Creates a new `SkiaRenderer` scanning standard Linux font paths with built-in fallback.
    pub fn new() -> Self {
        Self {
            font_manager: FontManager::load_system_fonts(),
        }
    }

    /// Creates a `SkiaRenderer` that uses only the built-in vector/bitmap fallback font.
    pub fn new_fallback_only() -> Self {
        Self {
            font_manager: FontManager::fallback_only(),
        }
    }

    /// Returns a reference to the underlying `FontManager`.
    pub fn font_manager(&self) -> &FontManager {
        &self.font_manager
    }

    /// Measures `(width, height)` of a text string for layout or custom widgets.
    pub fn measure_text(
        &self,
        text: &str,
        font_family: &str,
        font_size: f32,
        font_weight: u16,
    ) -> (f32, f32) {
        self.font_manager
            .measure_text(text, font_family, font_size, font_weight)
    }

    /// Renders a Server-Side Window Decoration (`FrameBuffer`) for a `WindowPlacement`.
    pub fn render_window_decoration(
        &self,
        placement: &WindowPlacement,
        title: &str,
        is_urgent: bool,
        style: Option<&ComputedStyle>,
    ) -> FrameBuffer {
        window_decoration::render_window_decoration(
            &self.font_manager,
            placement,
            title,
            is_urgent,
            style,
        )
    }

    /// Saves a rendered `FrameBuffer` to disk as a binary `.ppm` (`P6`) snapshot.
    pub fn save_ppm(&self, fb: &FrameBuffer, path: &Path) -> io::Result<()> {
        snapshot::save_ppm(fb, path)
    }

    /// Encodes a rendered `FrameBuffer` as a 32-bit `.bmp` byte vector.
    pub fn encode_bmp(&self, fb: &FrameBuffer) -> Vec<u8> {
        snapshot::encode_bmp(fb)
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_paint_items(
        &self,
        dom: &dyn DomTreeView,
        styles: &HashMap<NodeId, ComputedStyle>,
        layouts: &HashMap<NodeId, LayoutBox>,
        node_id: NodeId,
        inherited_z: i32,
        inherited_opacity: f32,
        inherited_clip: Option<Rect>,
        visited: &mut HashSet<NodeId>,
        out: &mut Vec<PaintItem>,
    ) {
        if !visited.insert(node_id) {
            return;
        }
        let Some(node) = dom.get_node(node_id) else {
            return;
        };
        if !node.visible {
            return;
        }
        if matches!(
            node.tag.as_str(),
            "meta" | "style" | "script" | "head" | "link" | "title"
        ) {
            return;
        }

        let default_style = ComputedStyle::default();
        let style = styles
            .get(&node_id)
            .or_else(|| node.parent.and_then(|p| styles.get(&p)))
            .unwrap_or(&default_style);

        if style.display == Display::None {
            return;
        }

        let opacity = (inherited_opacity * style.opacity).clamp(0.0, 1.0);
        if opacity <= 0.001 {
            return;
        }

        let layout_z = layouts.get(&node_id).map(|l| l.z_index).unwrap_or(0);
        let z_index = if style.z_index != 0 {
            style.z_index
        } else if layout_z != 0 {
            layout_z
        } else {
            inherited_z
        };

        let mut current_clip = inherited_clip;
        if matches!(style.overflow, Overflow::Hidden | Overflow::Scroll) {
            if let Some(layout) = layouts.get(&node_id) {
                current_clip = Some(match current_clip {
                    Some(prev) => intersect_rect(prev, layout.border_box),
                    None => layout.border_box,
                });
            }
        }

        let tree_order = out.len();
        out.push(PaintItem {
            node_id,
            z_index,
            tree_order,
            opacity,
            clip_rect: current_clip,
        });

        for &child_id in dom.children_of(node_id) {
            self.collect_paint_items(
                dom,
                styles,
                layouts,
                child_id,
                z_index,
                opacity,
                current_clip,
                visited,
                out,
            );
        }
    }

    fn paint_node(
        &self,
        pixmap: &mut tiny_skia::PixmapMut<'_>,
        dom: &dyn DomTreeView,
        styles: &HashMap<NodeId, ComputedStyle>,
        layouts: &HashMap<NodeId, LayoutBox>,
        item: &PaintItem,
    ) {
        let Some(node) = dom.get_node(item.node_id) else {
            return;
        };

        let default_style = ComputedStyle::default();
        let style = styles
            .get(&item.node_id)
            .or_else(|| node.parent.and_then(|p| styles.get(&p)))
            .unwrap_or(&default_style);

        let layout = layouts
            .get(&item.node_id)
            .or_else(|| node.parent.and_then(|p| layouts.get(&p)))
            .copied();

        let Some(layout_box) = layout else {
            return;
        };

        let border_rect = layout_box.border_box;
        let content_rect = if layout_box.content_box.width > 0.0
            && layout_box.content_box.height > 0.0
        {
            layout_box.content_box
        } else {
            border_rect
                .inset(style.border_width)
                .inset(style.padding)
        };

        if node.kind == DomNodeKind::Element {
            // 1. Box Shadow
            if let Some(shadow) = &style.box_shadow {
                draw_box_shadow(
                    pixmap,
                    border_rect,
                    style.border_radius,
                    shadow,
                    item.opacity,
                );
            }

            // 2. Background (solid color & linear gradient)
            draw_background(
                pixmap,
                border_rect,
                style.border_radius,
                style.background_color,
                style.background_gradient.as_ref(),
                item.opacity,
            );

            // 3. Border
            let bw = if has_nonzero_edges(layout_box.border) {
                layout_box.border
            } else {
                style.border_width
            };
            draw_border(
                pixmap,
                border_rect,
                style.border_radius,
                bw,
                style.border_color,
                item.opacity,
            );

            // 4. Vector / Icon Elements (`<svg>`, `<icon>`, `data-icon="..."`, `.icon-*`)
            let icon_name = detect_icon_name(node);
            if let Some(ref icon) = icon_name {
                let icon_bounds = if content_rect.width >= 4.0 && content_rect.height >= 4.0 {
                    content_rect
                } else {
                    border_rect
                };
                draw_vector_icon(pixmap, icon, node, icon_bounds, style.color, item.opacity);
            }

            // 5. Typography on Element (if text is stored directly on the Element or `<input>`)
            if let Some((text, text_color)) = extract_element_text(dom, node, style, icon_name.as_deref()) {
                let text_bounds = if content_rect.width > 0.0 && content_rect.height > 0.0 {
                    content_rect
                } else {
                    border_rect
                };
                self.font_manager.draw_text(
                    pixmap,
                    &text,
                    text_bounds,
                    &style.font_family,
                    style.font_size,
                    style.font_weight,
                    text_color,
                    style.text_align,
                    item.opacity,
                    item.clip_rect,
                );
            }
        } else if node.kind == DomNodeKind::Text {
            // Typography on dedicated Text DOM nodes
            if let Some(text) = &node.text_content {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    // Check if parent was an `<icon>` or `<svg>` that already consumed this text as an icon name
                    let parent_is_icon = node
                        .parent
                        .and_then(|pid| dom.get_node(pid))
                        .is_some_and(|p| p.tag == "icon" || p.tag == "svg");

                    if !parent_is_icon {
                        let text_bounds = if content_rect.width > 0.0 && content_rect.height > 0.0 {
                            content_rect
                        } else {
                            border_rect
                        };
                        self.font_manager.draw_text(
                            pixmap,
                            trimmed,
                            text_bounds,
                            &style.font_family,
                            style.font_size,
                            style.font_weight,
                            style.color,
                            style.text_align,
                            item.opacity,
                            item.clip_rect,
                        );
                    }
                }
            }
        }
    }
}

impl SurfaceRenderer for SkiaRenderer {
    fn render_surface(
        &mut self,
        dom: &dyn DomTreeView,
        styles: &HashMap<NodeId, ComputedStyle>,
        layouts: &HashMap<NodeId, LayoutBox>,
        viewport: Size,
    ) -> FrameBuffer {
        let width = (viewport.width.ceil() as u32).max(1);
        let height = (viewport.height.ceil() as u32).max(1);

        let Some(mut pixmap) = Pixmap::new(width, height) else {
            return FrameBuffer::new(width, height);
        };

        let mut visited = HashSet::new();
        let mut paint_items = Vec::new();

        let root_id = dom.root_id();
        self.collect_paint_items(
            dom,
            styles,
            layouts,
            root_id,
            0,
            1.0,
            None,
            &mut visited,
            &mut paint_items,
        );

        // Also visit any additional top-level nodes not reachable from `root_id`
        for id in dom.all_node_ids() {
            if !visited.contains(&id) {
                self.collect_paint_items(
                    dom,
                    styles,
                    layouts,
                    id,
                    0,
                    1.0,
                    None,
                    &mut visited,
                    &mut paint_items,
                );
            }
        }

        // Sort in stacking order: primary `z_index`, secondary DOM tree pre-order
        paint_items.sort_by_key(|item| (item.z_index, item.tree_order));

        {
            let mut pm = pixmap.as_mut();
            for item in &paint_items {
                self.paint_node(&mut pm, dom, styles, layouts, item);
            }
        }

        FrameBuffer {
            width,
            height,
            pixels: pixmap.take(),
            damaged_rects: vec![(0, 0, width, height)],
        }
    }
}

fn has_nonzero_edges(e: wm_proto::Edges) -> bool {
    e.top > 0.01 || e.right > 0.01 || e.bottom > 0.01 || e.left > 0.01
}

fn intersect_rect(a: Rect, b: Rect) -> Rect {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = a.right().min(b.right());
    let y2 = a.bottom().min(b.bottom());
    Rect::new(x1, y1, (x2 - x1).max(0.0), (y2 - y1).max(0.0))
}

fn extract_element_text(
    dom: &dyn DomTreeView,
    node: &DomNodeData,
    style: &ComputedStyle,
    icon_name: Option<&str>,
) -> Option<(String, Color)> {
    // Handle `<input>` value or placeholder
    if node.tag == "input" {
        if let Some(val) = node.get_attr("value") {
            if !val.is_empty() {
                return Some((val.to_string(), style.color));
            }
        }
        if let Some(placeholder) = node.get_attr("placeholder") {
            if !placeholder.is_empty() {
                let muted = Color::rgba(
                    style.color.r,
                    style.color.g,
                    style.color.b,
                    (style.color.a as u16 * 140 / 255) as u8,
                );
                return Some((placeholder.to_string(), muted));
            }
        }
    }

    let raw = node.text_content.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }

    // If this element is an `<icon>` or `<svg>` and its text matches the icon name, don't draw text on top of the vector icon
    if (node.tag == "icon" || node.tag == "svg") && icon_name.is_some() {
        return None;
    }
    if let Some(ic) = icon_name {
        if raw.eq_ignore_ascii_case(ic) {
            return None;
        }
    }

    // Avoid double-rendering if a child `DomNodeKind::Text` node already carries text
    let has_text_child = node.children.iter().any(|&cid| {
        dom.get_node(cid).is_some_and(|child| {
            child.kind == DomNodeKind::Text
                && child
                    .text_content
                    .as_deref()
                    .is_some_and(|t| !t.trim().is_empty())
        })
    });
    if has_text_child {
        return None;
    }

    Some((raw.to_string(), style.color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use wm_proto::{
        BoxShadow, Display, Edges, GradientStop, LinearGradient, PseudoStates, TextAlign,
    };

    struct MockDom {
        root: NodeId,
        nodes: BTreeMap<NodeId, DomNodeData>,
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
        parent: Option<usize>,
        children: Vec<usize>,
        text: Option<&str>,
    ) -> DomNodeData {
        DomNodeData {
            id: NodeId(id),
            kind: DomNodeKind::Element,
            tag: tag.to_string(),
            element_id: None,
            classes: Vec::new(),
            attributes: BTreeMap::new(),
            inline_styles: BTreeMap::new(),
            text_content: text.map(|s| s.to_string()),
            pseudo_states: PseudoStates::empty(),
            visible: true,
            parent: parent.map(NodeId),
            children: children.into_iter().map(NodeId).collect(),
        }
    }

    #[test]
    fn test_render_surface_full_pipeline() {
        let mut nodes = BTreeMap::new();
        // Root bar container
        let root_node = make_element(1, "div", None, vec![2, 3, 4], None);
        // Launcher icon button
        let mut icon_btn = make_element(2, "button", Some(1), vec![], None);
        icon_btn
            .attributes
            .insert("data-icon".to_string(), "launcher".to_string());
        // Center window title pill
        let title_pill = make_element(
            3,
            "div",
            Some(1),
            vec![],
            Some("Lumina Desktop — Wayland Compositor"),
        );
        // Hidden overlay node (should be skipped)
        let hidden_node = make_element(4, "div", Some(1), vec![], Some("Should Not Render"));

        nodes.insert(NodeId(1), root_node);
        nodes.insert(NodeId(2), icon_btn);
        nodes.insert(NodeId(3), title_pill);
        nodes.insert(NodeId(4), hidden_node);

        let dom = MockDom {
            root: NodeId(1),
            nodes,
        };

        let mut styles = HashMap::new();
        let mut root_style = ComputedStyle::default();
        root_style.background_color = Color::rgba(30, 30, 46, 240);
        root_style.border_color = Color::rgba(137, 180, 250, 180);
        root_style.border_width = Edges::all(1.0);
        root_style.border_radius = [10.0; 4];
        styles.insert(NodeId(1), root_style);

        let mut btn_style = ComputedStyle::default();
        btn_style.background_gradient = Some(LinearGradient {
            angle_deg: 135.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color::rgb(137, 180, 250),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::rgb(203, 166, 247),
                },
            ],
        });
        btn_style.border_radius = [8.0; 4];
        btn_style.color = Color::rgb(17, 17, 27);
        btn_style.box_shadow = Some(BoxShadow {
            offset_x: 0.0,
            offset_y: 2.0,
            blur_radius: 6.0,
            spread_radius: 0.0,
            color: Color::rgba(0, 0, 0, 120),
            inset: false,
        });
        styles.insert(NodeId(2), btn_style);

        let mut pill_style = ComputedStyle::default();
        pill_style.background_color = Color::rgba(49, 50, 68, 220);
        pill_style.border_radius = [12.0; 4];
        pill_style.color = Color::rgb(205, 214, 244);
        pill_style.font_size = 14.0;
        pill_style.font_weight = 600;
        pill_style.text_align = TextAlign::Center;
        styles.insert(NodeId(3), pill_style);

        let mut hidden_style = ComputedStyle::default();
        hidden_style.display = Display::None;
        styles.insert(NodeId(4), hidden_style);

        let mut layouts = HashMap::new();
        layouts.insert(
            NodeId(1),
            LayoutBox {
                border_box: Rect::new(0.0, 0.0, 800.0, 44.0),
                content_box: Rect::new(8.0, 4.0, 784.0, 36.0),
                padding: Edges::xy(8.0, 4.0),
                border: Edges::all(1.0),
                margin: Edges::ZERO,
                z_index: 0,
            },
        );
        layouts.insert(
            NodeId(2),
            LayoutBox {
                border_box: Rect::new(10.0, 6.0, 32.0, 32.0),
                content_box: Rect::new(14.0, 10.0, 24.0, 24.0),
                padding: Edges::all(4.0),
                border: Edges::ZERO,
                margin: Edges::ZERO,
                z_index: 1,
            },
        );
        layouts.insert(
            NodeId(3),
            LayoutBox {
                border_box: Rect::new(240.0, 6.0, 320.0, 32.0),
                content_box: Rect::new(252.0, 8.0, 296.0, 28.0),
                padding: Edges::xy(12.0, 2.0),
                border: Edges::ZERO,
                margin: Edges::ZERO,
                z_index: 1,
            },
        );
        layouts.insert(
            NodeId(4),
            LayoutBox {
                border_box: Rect::new(600.0, 6.0, 150.0, 32.0),
                content_box: Rect::new(600.0, 6.0, 150.0, 32.0),
                padding: Edges::ZERO,
                border: Edges::ZERO,
                margin: Edges::ZERO,
                z_index: 10,
            },
        );

        let mut renderer = SkiaRenderer::new();
        let frame = renderer.render_surface(&dom, &styles, &layouts, Size::new(800.0, 44.0));

        assert_eq!(frame.width, 800);
        assert_eq!(frame.height, 44);
        assert!(frame.non_transparent_pixel_count() > 10_000);
    }

    #[test]
    fn test_fallback_font_renderer_zero_system_fonts() {
        let mut nodes = BTreeMap::new();
        let root = make_element(1, "div", None, vec![], Some("CPU 42% | RAM 6.4G | 12:34"));
        nodes.insert(NodeId(1), root);
        let dom = MockDom {
            root: NodeId(1),
            nodes,
        };

        let mut styles = HashMap::new();
        let mut style = ComputedStyle::default();
        style.color = Color::WHITE;
        style.font_size = 14.0;
        styles.insert(NodeId(1), style);

        let mut layouts = HashMap::new();
        layouts.insert(
            NodeId(1),
            LayoutBox {
                border_box: Rect::new(0.0, 0.0, 300.0, 32.0),
                content_box: Rect::new(8.0, 4.0, 284.0, 24.0),
                padding: Edges::xy(8.0, 4.0),
                border: Edges::ZERO,
                margin: Edges::ZERO,
                z_index: 0,
            },
        );

        let mut renderer = SkiaRenderer::new_fallback_only();
        let frame = renderer.render_surface(&dom, &styles, &layouts, Size::new(300.0, 32.0));
        assert!(
            frame.non_transparent_pixel_count() > 100,
            "Fallback glyph renderer should produce visible text pixels even without system fonts"
        );
    }

    #[test]
    fn test_window_decoration_and_snapshot_export() {
        let renderer = SkiaRenderer::new();
        let placement = WindowPlacement {
            window_id: 101,
            frame_rect: Rect::new(40.0, 60.0, 640.0, 420.0),
            content_rect: Rect::new(42.0, 92.0, 636.0, 386.0),
            titlebar_rect: Some(Rect::new(40.0, 60.0, 640.0, 32.0)),
            border_width: 2.0,
            border_radius: 10.0,
            is_focused: true,
            is_floating: false,
            is_fullscreen: false,
            z_order: 1,
        };

        let fb = renderer.render_window_decoration(&placement, "Alacritty — zsh", false, None);
        assert_eq!(fb.width, 640);
        assert_eq!(fb.height, 420);
        assert!(fb.non_transparent_pixel_count() > 5_000);

        let ppm = encode_ppm(&fb);
        assert!(ppm.starts_with(b"P6\n640 420\n255\n"));

        let bmp = renderer.encode_bmp(&fb);
        assert!(bmp.starts_with(b"BM"));
        assert_eq!(bmp.len(), 54 + (640 * 420 * 4));

        let ppm_path = std::env::temp_dir().join("lumina_test_window_decoration.ppm");
        renderer
            .save_ppm(&fb, &ppm_path)
            .expect("should save PPM snapshot to disk");
        let saved_bytes = std::fs::read(&ppm_path).expect("should read saved PPM file");
        assert!(saved_bytes.starts_with(b"P6\n640 420\n255\n"));
        let _ = std::fs::remove_file(&ppm_path);
    }

    #[test]
    fn test_z_index_stacking_and_vector_icons() {
        let mut nodes = BTreeMap::new();
        let root = make_element(1, "div", None, vec![2, 3], None);

        // Node 2 has higher z-index (10) and is Red, even though it appears before Node 3 in tree order
        let mut high_z = make_element(2, "div", Some(1), vec![], None);
        high_z
            .attributes
            .insert("data-icon".to_string(), "wifi".to_string());

        // Node 3 has lower z-index (1) and is Blue, overlapping the exact same rect
        let low_z = make_element(3, "div", Some(1), vec![], None);

        nodes.insert(NodeId(1), root);
        nodes.insert(NodeId(2), high_z);
        nodes.insert(NodeId(3), low_z);

        let dom = MockDom {
            root: NodeId(1),
            nodes,
        };

        let mut styles = HashMap::new();
        styles.insert(NodeId(1), ComputedStyle::default());

        let mut s_high = ComputedStyle::default();
        s_high.background_color = Color::rgb(255, 0, 0);
        s_high.color = Color::WHITE;
        s_high.z_index = 10;
        styles.insert(NodeId(2), s_high);

        let mut s_low = ComputedStyle::default();
        s_low.background_color = Color::rgb(0, 0, 255);
        s_low.z_index = 1;
        styles.insert(NodeId(3), s_low);

        let shared_box = LayoutBox {
            border_box: Rect::new(0.0, 0.0, 64.0, 64.0),
            content_box: Rect::new(8.0, 8.0, 48.0, 48.0),
            padding: Edges::all(8.0),
            border: Edges::ZERO,
            margin: Edges::ZERO,
            z_index: 0,
        };
        let mut layouts = HashMap::new();
        layouts.insert(NodeId(1), shared_box);
        layouts.insert(NodeId(2), shared_box);
        layouts.insert(NodeId(3), shared_box);

        let mut renderer = SkiaRenderer::new();
        let fb = renderer.render_surface(&dom, &styles, &layouts, Size::new(64.0, 64.0));
        assert!(fb.non_transparent_pixel_count() > 3000);

        // Top-left corner pixel (2, 2) should be Red (from high_z node 2), not Blue
        let idx = ((2 * 64 + 2) * 4) as usize;
        assert_eq!(fb.pixels[idx], 255);
        assert_eq!(fb.pixels[idx + 2], 0);
    }
}
