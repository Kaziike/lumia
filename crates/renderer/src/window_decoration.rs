use crate::font::FontManager;
use crate::icons::draw_vector_icon;
use crate::primitives::{
    build_rounded_rect_path, draw_background, draw_border, draw_box_shadow,
};
use std::collections::BTreeMap;
use tiny_skia::Pixmap;
use wm_proto::{
    BoxShadow, Color, ComputedStyle, DomNodeData, DomNodeKind, Edges, FrameBuffer, GradientStop,
    LinearGradient, NodeId, PseudoStates, Rect, TextAlign, WindowPlacement,
};

/// Theme colors and metrics for rendering server-side window decorations.
#[derive(Clone, Debug)]
pub struct WindowDecorationTheme {
    pub titlebar_bg_active: Color,
    pub titlebar_bg_inactive: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub border_urgent: Color,
    pub text_active: Color,
    pub text_inactive: Color,
    pub btn_close_bg: Color,
    pub btn_maximize_bg: Color,
    pub btn_float_bg: Color,
    pub btn_fg: Color,
    pub font_family: String,
    pub font_size: f32,
}

impl Default for WindowDecorationTheme {
    fn default() -> Self {
        Self {
            titlebar_bg_active: Color::rgba(30, 30, 46, 240),
            titlebar_bg_inactive: Color::rgba(24, 24, 37, 225),
            border_active: Color::rgba(137, 180, 250, 255),
            border_inactive: Color::rgba(69, 71, 90, 200),
            border_urgent: Color::rgba(243, 139, 168, 255),
            text_active: Color::rgba(205, 214, 244, 255),
            text_inactive: Color::rgba(147, 153, 178, 230),
            btn_close_bg: Color::rgba(243, 139, 168, 235),
            btn_maximize_bg: Color::rgba(166, 227, 161, 225),
            btn_float_bg: Color::rgba(137, 180, 250, 225),
            btn_fg: Color::rgba(17, 17, 27, 255),
            font_family: "sans-serif".to_string(),
            font_size: 13.0,
        }
    }
}

impl WindowDecorationTheme {
    pub fn from_computed_style(style: &ComputedStyle) -> Self {
        let mut theme = Self::default();
        if !style.background_color.is_transparent() {
            theme.titlebar_bg_active = style.background_color;
        }
        if !style.border_color.is_transparent() {
            theme.border_active = style.border_color;
        }
        if !style.color.is_transparent() {
            theme.text_active = style.color;
        }
        if !style.font_family.is_empty() {
            theme.font_family = style.font_family.clone();
        }
        if style.font_size >= 8.0 {
            theme.font_size = style.font_size;
        }
        theme
    }
}

/// Renders a Server-Side Window Decoration (`FrameBuffer`) for a `WindowPlacement`.
pub fn render_window_decoration(
    font_mgr: &FontManager,
    placement: &WindowPlacement,
    title: &str,
    is_urgent: bool,
    style: Option<&ComputedStyle>,
) -> FrameBuffer {
    let theme = style
        .map(WindowDecorationTheme::from_computed_style)
        .unwrap_or_default();
    render_window_decoration_with_theme(font_mgr, placement, title, is_urgent, &theme)
}

/// Renders a Server-Side Window Decoration (`FrameBuffer`) using an explicit `WindowDecorationTheme`.
pub fn render_window_decoration_with_theme(
    font_mgr: &FontManager,
    placement: &WindowPlacement,
    title: &str,
    is_urgent: bool,
    theme: &WindowDecorationTheme,
) -> FrameBuffer {
    let width = (placement.frame_rect.width.ceil() as u32).max(1);
    let height = (placement.frame_rect.height.ceil() as u32).max(1);

    let Some(mut pixmap) = Pixmap::new(width, height) else {
        return FrameBuffer::new(width, height);
    };

    if placement.is_fullscreen {
        return FrameBuffer {
            width,
            height,
            pixels: pixmap.take(),
            damaged_rects: vec![(0, 0, width, height)],
        };
    }

    let bw = placement.border_width.max(1.0);
    let radius = placement.border_radius.max(0.0);
    let border_color = if is_urgent {
        theme.border_urgent
    } else if placement.is_focused {
        theme.border_active
    } else {
        theme.border_inactive
    };

    let local_frame = Rect::new(0.0, 0.0, width as f32, height as f32);
    let mut pm = pixmap.as_mut();

    // Determine local titlebar rectangle
    let titlebar_h = if let Some(tb) = placement.titlebar_rect {
        tb.height.max(24.0)
    } else {
        let inferred = placement.content_rect.y - placement.frame_rect.y - bw;
        if inferred >= 18.0 {
            inferred
        } else {
            30.0f32.min(height as f32)
        }
    };

    let tb_rect = Rect::new(
        bw * 0.5,
        bw * 0.5,
        (width as f32 - bw).max(1.0),
        titlebar_h.min(height as f32 - bw),
    );

    // 1. Subtle inner glow / shadow on focused or urgent window frame
    if placement.is_focused || is_urgent {
        let glow = BoxShadow {
            offset_x: 0.0,
            offset_y: 0.0,
            blur_radius: 6.0,
            spread_radius: 1.0,
            color: Color::rgba(border_color.r, border_color.g, border_color.b, 95),
            inset: true,
        };
        draw_box_shadow(&mut pm, local_frame, [radius; 4], &glow, 1.0);
    }

    // 2. Titlebar background with top rounded corners and subtle gradient
    let tb_bg = if placement.is_focused {
        theme.titlebar_bg_active
    } else {
        theme.titlebar_bg_inactive
    };
    let tb_grad = if placement.is_focused {
        Some(LinearGradient {
            angle_deg: 135.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: tb_bg,
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::rgba(
                        tb_bg.r.saturating_add(16),
                        tb_bg.g.saturating_add(18),
                        tb_bg.b.saturating_add(28),
                        tb_bg.a,
                    ),
                },
            ],
        })
    } else {
        None
    };

    draw_background(
        &mut pm,
        tb_rect,
        [radius, radius, 0.0, 0.0],
        tb_bg,
        tb_grad.as_ref(),
        1.0,
    );

    // Bottom separator line under titlebar
    draw_border(
        &mut pm,
        tb_rect,
        [0.0; 4],
        Edges {
            top: 0.0,
            right: 0.0,
            bottom: 1.0,
            left: 0.0,
        },
        Color::rgba(border_color.r, border_color.g, border_color.b, 110),
        1.0,
    );

    // 3. Left window mode badge / icon
    let pad_x = 10.0;
    let icon_size = (tb_rect.height - 12.0).clamp(12.0, 18.0);
    let icon_rect = Rect::new(
        tb_rect.x + pad_x,
        tb_rect.y + (tb_rect.height - icon_size) * 0.5,
        icon_size,
        icon_size,
    );
    let dummy_node = dummy_icon_node();
    draw_vector_icon(
        &mut pm,
        if placement.is_floating {
            "float"
        } else {
            "layout"
        },
        &dummy_node,
        icon_rect,
        border_color,
        0.95,
    );

    // 4. Right window control buttons: Float, Maximize, Close
    let btn_size = (tb_rect.height - 10.0).clamp(14.0, 20.0);
    let btn_gap = 6.0;
    let buttons_total_w = btn_size * 3.0 + btn_gap * 2.0 + pad_x;

    let close_rect = Rect::new(
        tb_rect.right() - pad_x - btn_size,
        tb_rect.y + (tb_rect.height - btn_size) * 0.5,
        btn_size,
        btn_size,
    );
    let max_rect = Rect::new(
        close_rect.x - btn_gap - btn_size,
        close_rect.y,
        btn_size,
        btn_size,
    );
    let float_rect = Rect::new(
        max_rect.x - btn_gap - btn_size,
        close_rect.y,
        btn_size,
        btn_size,
    );

    let btn_radius = [btn_size * 0.5; 4];
    draw_background(&mut pm, float_rect, btn_radius, theme.btn_float_bg, None, 0.9);
    draw_vector_icon(
        &mut pm,
        "float",
        &dummy_node,
        float_rect.inset(Edges::all(3.5)),
        theme.btn_fg,
        1.0,
    );

    draw_background(
        &mut pm,
        max_rect,
        btn_radius,
        theme.btn_maximize_bg,
        None,
        0.9,
    );
    draw_vector_icon(
        &mut pm,
        "maximize",
        &dummy_node,
        max_rect.inset(Edges::all(3.5)),
        theme.btn_fg,
        1.0,
    );

    draw_background(&mut pm, close_rect, btn_radius, theme.btn_close_bg, None, 0.95);
    draw_vector_icon(
        &mut pm,
        "close",
        &dummy_node,
        close_rect.inset(Edges::all(3.5)),
        theme.btn_fg,
        1.0,
    );

    // 5. Window title text centered in remaining titlebar space
    let title_x = icon_rect.right() + 8.0;
    let title_w = (tb_rect.width - (title_x - tb_rect.x) - buttons_total_w - 8.0).max(0.0);
    if title_w > 12.0 && !title.is_empty() {
        let title_bounds = Rect::new(title_x, tb_rect.y, title_w, tb_rect.height);
        let text_color = if placement.is_focused {
            theme.text_active
        } else {
            theme.text_inactive
        };
        let weight = if placement.is_focused { 600 } else { 400 };
        font_mgr.draw_text(
            &mut pm,
            title,
            title_bounds,
            &theme.font_family,
            theme.font_size,
            weight,
            text_color,
            TextAlign::Center,
            1.0,
            Some(title_bounds),
        );
    }

    // 6. Outer rounded frame border
    let _ = build_rounded_rect_path(local_frame, [radius; 4]);
    draw_border(
        &mut pm,
        local_frame,
        [radius; 4],
        Edges::all(bw),
        border_color,
        1.0,
    );

    FrameBuffer {
        width,
        height,
        pixels: pixmap.take(),
        damaged_rects: vec![(0, 0, width, height)],
    }
}

fn dummy_icon_node() -> DomNodeData {
    DomNodeData {
        id: NodeId(0),
        kind: DomNodeKind::Element,
        tag: "icon".to_string(),
        element_id: None,
        classes: Vec::new(),
        attributes: BTreeMap::new(),
        inline_styles: BTreeMap::new(),
        text_content: None,
        pseudo_states: PseudoStates::empty(),
        visible: true,
        parent: None,
        children: Vec::new(),
    }
}
