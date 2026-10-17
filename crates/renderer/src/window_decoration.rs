use crate::font::FontManager;
use crate::icons::draw_vector_icon;
use crate::primitives::{
    build_rounded_rect_path, draw_background, draw_border, draw_box_shadow,
};
use std::collections::BTreeMap;
use tiny_skia::Pixmap;
use wm_proto::{
    BoxShadow, Color, ComputedStyle, DomNodeData, DomNodeKind, Edges, FrameBuffer, GradientStop,
    LinearGradient, NodeId, Point, PseudoStates, Rect, TextAlign, WindowPlacement,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarHit {
    Float,
    Maximize,
    Close,
    DragArea,
}

/// Hit-tests a global screen point against a window's server-side titlebar and control buttons.
pub fn hit_test_titlebar(placement: &WindowPlacement, pt: Point) -> Option<TitlebarHit> {
    if placement.is_fullscreen || !placement.frame_rect.contains(pt) {
        return None;
    }
    let bw = placement.border_width.max(1.0);
    let titlebar_h = if let Some(tb) = placement.titlebar_rect {
        tb.height.max(24.0)
    } else {
        let inferred = placement.content_rect.y - placement.frame_rect.y - bw;
        if inferred >= 18.0 {
            inferred
        } else {
            30.0f32.min(placement.frame_rect.height)
        }
    };

    let local_x = pt.x - placement.frame_rect.x;
    let local_y = pt.y - placement.frame_rect.y;
    let width = placement.frame_rect.width;

    if local_y < 0.0 || local_y > bw + titlebar_h {
        return None;
    }

    let tb_rect = Rect::new(
        bw * 0.5,
        bw * 0.5,
        (width - bw).max(1.0),
        titlebar_h.min(placement.frame_rect.height - bw),
    );
    let pad_x = 10.0;
    let btn_size = (tb_rect.height - 10.0).clamp(14.0, 20.0);
    let btn_gap = 6.0;

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

    let local_pt = Point::new(local_x, local_y);
    if close_rect.inset(Edges::all(-2.0)).contains(local_pt) {
        return Some(TitlebarHit::Close);
    }
    if max_rect.inset(Edges::all(-2.0)).contains(local_pt) {
        return Some(TitlebarHit::Maximize);
    }
    if float_rect.inset(Edges::all(-2.0)).contains(local_pt) {
        return Some(TitlebarHit::Float);
    }

    Some(TitlebarHit::DragArea)
}

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
            titlebar_bg_active: Color::rgba(30, 30, 46, 245),
            titlebar_bg_inactive: Color::rgba(24, 24, 37, 235),
            border_active: Color::rgba(137, 180, 250, 255),
            border_inactive: Color::rgba(69, 71, 90, 210),
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

/// Renders a Server-Side Window Decoration AND rich interactive application content inside the client area.
pub fn render_window_with_app_content(
    font_mgr: &FontManager,
    placement: &WindowPlacement,
    app_id: &str,
    title: &str,
    is_urgent: bool,
    terminal_lines: &[String],
    terminal_input: &str,
    style: Option<&ComputedStyle>,
) -> FrameBuffer {
    render_window_with_client_buffer(
        font_mgr,
        placement,
        app_id,
        title,
        is_urgent,
        None,
        terminal_lines,
        terminal_input,
        style,
    )
}

/// Renders a Server-Side Window Decoration and blits a live Wayland client `FrameBuffer` (if attached)
/// or falls back to the built-in application surface renderer.
#[allow(clippy::too_many_arguments)]
pub fn render_window_with_client_buffer(
    font_mgr: &FontManager,
    placement: &WindowPlacement,
    app_id: &str,
    title: &str,
    is_urgent: bool,
    client_buffer: Option<&FrameBuffer>,
    terminal_lines: &[String],
    terminal_input: &str,
    style: Option<&ComputedStyle>,
) -> FrameBuffer {
    let theme = style
        .map(WindowDecorationTheme::from_computed_style)
        .unwrap_or_default();
    render_window_full(
        font_mgr,
        placement,
        Some(app_id),
        title,
        is_urgent,
        client_buffer,
        terminal_lines,
        terminal_input,
        &theme,
    )
}

/// Renders a Server-Side Window Decoration (`FrameBuffer`) using an explicit `WindowDecorationTheme`.
pub fn render_window_decoration_with_theme(
    font_mgr: &FontManager,
    placement: &WindowPlacement,
    title: &str,
    is_urgent: bool,
    theme: &WindowDecorationTheme,
) -> FrameBuffer {
    render_window_full(
        font_mgr,
        placement,
        Some(title),
        title,
        is_urgent,
        None,
        &[],
        "",
        theme,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_window_full(
    font_mgr: &FontManager,
    placement: &WindowPlacement,
    app_id: Option<&str>,
    title: &str,
    is_urgent: bool,
    client_buffer: Option<&FrameBuffer>,
    terminal_lines: &[String],
    terminal_input: &str,
    theme: &WindowDecorationTheme,
) -> FrameBuffer {
    let width = (placement.frame_rect.width.ceil() as u32).max(1);
    let height = (placement.frame_rect.height.ceil() as u32).max(1);

    let Some(mut pixmap) = Pixmap::new(width, height) else {
        return FrameBuffer::new(width, height);
    };

    let bw = if placement.is_fullscreen {
        0.0
    } else {
        placement.border_width.max(1.5)
    };
    let radius = if placement.is_fullscreen {
        0.0
    } else {
        placement.border_radius.max(0.0)
    };
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
    let titlebar_h = if placement.is_fullscreen {
        0.0
    } else if let Some(tb) = placement.titlebar_rect {
        tb.height.max(24.0)
    } else {
        let inferred = placement.content_rect.y - placement.frame_rect.y - bw;
        if inferred >= 18.0 {
            inferred
        } else {
            30.0f32.min(height as f32)
        }
    };

    // 0. Draw client surface background & application content inside the window body
    let body_y = (bw + titlebar_h).min(height as f32);
    let body_rect = Rect::new(
        bw,
        body_y,
        (width as f32 - bw * 2.0).max(1.0),
        (height as f32 - body_y - bw).max(1.0),
    );
    if body_rect.width > 8.0 && body_rect.height > 8.0 {
        if let Some(client_fb) = client_buffer {
            blit_client_buffer_into_pixmap(&mut pm, body_rect, client_fb);
        } else {
            paint_app_client_surface(
                &mut pm,
                font_mgr,
                body_rect,
                [0.0, 0.0, (radius - bw).max(0.0), (radius - bw).max(0.0)],
                app_id.unwrap_or(title),
                title,
                placement.is_focused,
                terminal_lines,
                terminal_input,
                theme,
            );
        }
    }

    if placement.is_fullscreen {
        return FrameBuffer {
            width,
            height,
            pixels: pixmap.take(),
            damaged_rects: vec![(0, 0, width, height)],
        };
    }

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

fn blit_client_buffer_into_pixmap(
    pm: &mut tiny_skia::PixmapMut<'_>,
    rect: Rect,
    client_fb: &FrameBuffer,
) {
    let pm_w = pm.width() as usize;
    let pm_h = pm.height() as usize;
    let rx = rect.x.round().max(0.0) as usize;
    let ry = rect.y.round().max(0.0) as usize;
    let rw = (rect.width.round().max(1.0) as usize).min(pm_w.saturating_sub(rx));
    let rh = (rect.height.round().max(1.0) as usize).min(pm_h.saturating_sub(ry));
    let src_w = client_fb.width as usize;
    let src_h = client_fb.height as usize;

    if rw == 0 || rh == 0 || src_w == 0 || src_h == 0 {
        return;
    }

    let dst_data = pm.data_mut();

    // Fast path: 1:1 unscaled copy when client buffer matches or exceeds body rect
    if src_w == rw && src_h == rh {
        for y in 0..rh {
            let src_off = y * src_w * 4;
            let dst_off = ((ry + y) * pm_w + rx) * 4;
            let row_bytes = rw * 4;
            dst_data[dst_off..dst_off + row_bytes]
                .copy_from_slice(&client_fb.pixels[src_off..src_off + row_bytes]);
            // Ensure alpha is opaque inside the client area
            for px in dst_data[dst_off..dst_off + row_bytes].chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        return;
    }

    // Scaled blit when client buffer size differs during interactive resize
    for y in 0..rh {
        let sy = (y * src_h) / rh;
        let src_row = sy * src_w * 4;
        let dst_row = ((ry + y) * pm_w + rx) * 4;
        for x in 0..rw {
            let sx = (x * src_w) / rw;
            let s_idx = src_row + sx * 4;
            let d_idx = dst_row + x * 4;
            dst_data[d_idx] = client_fb.pixels[s_idx];
            dst_data[d_idx + 1] = client_fb.pixels[s_idx + 1];
            dst_data[d_idx + 2] = client_fb.pixels[s_idx + 2];
            dst_data[d_idx + 3] = 255;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_app_client_surface(
    pm: &mut tiny_skia::PixmapMut<'_>,
    font_mgr: &FontManager,
    rect: Rect,
    corners: [f32; 4],
    app_id: &str,
    title: &str,
    is_focused: bool,
    terminal_lines: &[String],
    terminal_input: &str,
    theme: &WindowDecorationTheme,
) {
    let lower_id = app_id.to_ascii_lowercase();
    let lower_title = title.to_ascii_lowercase();

    if lower_id.contains("term")
        || lower_id.contains("alacritty")
        || lower_id.contains("kitty")
        || lower_id.contains("wezterm")
        || lower_title.contains("terminal")
        || lower_title.contains("zsh")
    {
        paint_terminal_app(
            pm,
            font_mgr,
            rect,
            corners,
            is_focused,
            terminal_lines,
            terminal_input,
            theme,
        );
    } else if lower_id.contains("firefox")
        || lower_id.contains("browser")
        || lower_id.contains("chrom")
        || lower_id.contains("brave")
        || lower_title.contains("firefox")
    {
        paint_browser_app(pm, font_mgr, rect, corners, theme);
    } else if lower_id.contains("nautilus")
        || lower_id.contains("files")
        || lower_id.contains("dolphin")
        || lower_id.contains("thunar")
        || lower_title.contains("files")
    {
        paint_files_app(pm, font_mgr, rect, corners, theme);
    } else if lower_id.contains("code")
        || lower_id.contains("zed")
        || lower_id.contains("nvim")
        || lower_title.contains("code")
    {
        paint_code_editor_app(pm, font_mgr, rect, corners, theme);
    } else {
        paint_generic_app(pm, font_mgr, rect, corners, app_id, title, theme);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_terminal_app(
    pm: &mut tiny_skia::PixmapMut<'_>,
    font_mgr: &FontManager,
    rect: Rect,
    corners: [f32; 4],
    is_focused: bool,
    terminal_lines: &[String],
    terminal_input: &str,
    theme: &WindowDecorationTheme,
) {
    // Deep terminal backdrop
    draw_background(pm, rect, corners, Color::rgba(17, 17, 27, 252), None, 1.0);

    // Sub-tab bar
    let tab_bar = Rect::new(rect.x, rect.y, rect.width, 28.0);
    draw_background(pm, tab_bar, [0.0; 4], Color::rgba(24, 24, 37, 255), None, 1.0);

    let tab1 = Rect::new(rect.x + 8.0, rect.y + 4.0, 175.0, 22.0);
    draw_background(
        pm,
        tab1,
        [6.0, 6.0, 0.0, 0.0],
        Color::rgba(30, 30, 46, 255),
        None,
        1.0,
    );
    font_mgr.draw_text(
        pm,
        "1: zsh — ~/lumina-wm",
        tab1.inset(Edges::xy(8.0, 2.0)),
        "monospace",
        11.5,
        600,
        theme.border_active,
        TextAlign::Left,
        1.0,
        Some(tab1),
    );

    let tab2 = Rect::new(rect.x + 188.0, rect.y + 4.0, 145.0, 22.0);
    draw_background(
        pm,
        tab2,
        [6.0, 6.0, 0.0, 0.0],
        Color::rgba(24, 24, 37, 255),
        None,
        1.0,
    );
    font_mgr.draw_text(
        pm,
        "2: luminactl monitor",
        tab2.inset(Edges::xy(8.0, 2.0)),
        "monospace",
        11.5,
        400,
        theme.text_inactive,
        TextAlign::Left,
        1.0,
        Some(tab2),
    );

    let mut y = rect.y + 38.0;
    let pad_x = rect.x + 14.0;
    let avail_w = (rect.width - 28.0).max(40.0);
    let line_h = 20.0;

    // Neofetch-style system summary card at top of terminal
    let banner_lines: &[(&str, Color)] = &[
        (
            "╭─── Lumina WM v0.1.0 (Pure-Rust Wayland + HTML/CSS Shell) ───╮",
            Color::rgb(137, 180, 250),
        ),
        (
            "│ Engine   : Taffy Flexbox/Grid + CSS3 Cascade + Tiny-Skia    │",
            Color::rgb(203, 166, 247),
        ),
        (
            "│ WebKit   : 0% (Zero Browser Engine / Zero Electron)         │",
            Color::rgb(166, 227, 161),
        ),
        (
            "│ Tip      : Type shell commands right here & press [Enter]!  │",
            Color::rgb(249, 226, 175),
        ),
        (
            "╰─────────────────────────────────────────────────────────────╯",
            Color::rgb(137, 180, 250),
        ),
    ];

    for &(txt, col) in banner_lines {
        if y + line_h > rect.bottom() - 28.0 {
            break;
        }
        let line_rect = Rect::new(pad_x, y, avail_w, line_h);
        font_mgr.draw_text(
            pm,
            txt,
            line_rect,
            "monospace",
            12.0,
            500,
            col,
            TextAlign::Left,
            1.0,
            Some(line_rect),
        );
        y += line_h;
    }

    y += 6.0;

    // Calculate how many history lines fit above the live prompt
    let max_lines = ((rect.bottom() - 32.0 - y) / line_h).floor().max(0.0) as usize;
    let start_idx = terminal_lines.len().saturating_sub(max_lines);
    for line in &terminal_lines[start_idx..] {
        let line_rect = Rect::new(pad_x, y, avail_w, line_h);
        let color = if line.starts_with("$ ") || line.starts_with("kaz@") {
            Color::rgb(166, 227, 161)
        } else if line.starts_with("error") {
            Color::rgb(243, 139, 168)
        } else {
            Color::rgb(205, 214, 244)
        };
        font_mgr.draw_text(
            pm,
            line,
            line_rect,
            "monospace",
            12.5,
            400,
            color,
            TextAlign::Left,
            1.0,
            Some(line_rect),
        );
        y += line_h;
    }

    // Active interactive prompt line
    if y + line_h <= rect.bottom() - 4.0 {
        let cursor = if is_focused { "█" } else { "" };
        let prompt = format!("kaz@lumina ~/lumina-wm $ {terminal_input}{cursor}");
        let prompt_rect = Rect::new(pad_x, y, avail_w, line_h);
        font_mgr.draw_text(
            pm,
            &prompt,
            prompt_rect,
            "monospace",
            13.0,
            600,
            Color::rgb(137, 220, 235),
            TextAlign::Left,
            1.0,
            Some(prompt_rect),
        );
    }
}

fn paint_browser_app(
    pm: &mut tiny_skia::PixmapMut<'_>,
    font_mgr: &FontManager,
    rect: Rect,
    corners: [f32; 4],
    theme: &WindowDecorationTheme,
) {
    // Browser backdrop
    draw_background(pm, rect, corners, Color::rgba(22, 22, 34, 252), None, 1.0);

    // Navigation toolbar
    let nav_rect = Rect::new(rect.x, rect.y, rect.width, 38.0);
    draw_background(
        pm,
        nav_rect,
        [0.0; 4],
        Color::rgba(30, 30, 46, 255),
        None,
        1.0,
    );

    // URL pill
    let url_w = (rect.width - 120.0).max(120.0);
    let url_rect = Rect::new(rect.x + 76.0, rect.y + 6.0, url_w, 26.0);
    draw_background(
        pm,
        url_rect,
        [8.0; 4],
        Color::rgba(17, 17, 27, 240),
        None,
        1.0,
    );
    draw_border(
        pm,
        url_rect,
        [8.0; 4],
        Edges::all(1.0),
        Color::rgba(137, 180, 250, 90),
        1.0,
    );

    let nav_btns = Rect::new(rect.x + 12.0, rect.y + 6.0, 56.0, 26.0);
    font_mgr.draw_text(
        pm,
        "<  >  ↻",
        nav_btns,
        "sans-serif",
        13.0,
        700,
        theme.text_active,
        TextAlign::Left,
        1.0,
        Some(nav_btns),
    );

    font_mgr.draw_text(
        pm,
        "https://lumina-wm.dev/docs/html-css-compositor",
        url_rect.inset(Edges::xy(12.0, 4.0)),
        "sans-serif",
        12.0,
        500,
        Color::rgb(166, 227, 161),
        TextAlign::Left,
        1.0,
        Some(url_rect),
    );

    // Hero Banner Card inside webpage
    let content_x = rect.x + 20.0;
    let content_w = (rect.width - 40.0).max(60.0);
    let hero_rect = Rect::new(content_x, rect.y + 54.0, content_w, 96.0);
    let hero_grad = LinearGradient {
        angle_deg: 135.0,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: Color::rgba(49, 50, 82, 240),
            },
            GradientStop {
                offset: 1.0,
                color: Color::rgba(30, 30, 56, 240),
            },
        ],
    };
    draw_background(
        pm,
        hero_rect,
        [12.0; 4],
        Color::rgba(36, 38, 58, 255),
        Some(&hero_grad),
        1.0,
    );
    draw_border(
        pm,
        hero_rect,
        [12.0; 4],
        Edges::all(1.0),
        Color::rgba(137, 180, 250, 120),
        1.0,
    );

    font_mgr.draw_text(
        pm,
        "Lumina WM — Declarative HTML5 + CSS3 Wayland Desktop",
        Rect::new(hero_rect.x + 16.0, hero_rect.y + 16.0, hero_rect.width - 32.0, 26.0),
        "sans-serif",
        15.0,
        700,
        Color::rgb(205, 214, 244),
        TextAlign::Left,
        1.0,
        Some(hero_rect),
    );
    font_mgr.draw_text(
        pm,
        "Powered by Rust + Taffy Layout + CSS3 Cascade + Tiny-Skia 2D SIMD Rasterizer",
        Rect::new(hero_rect.x + 16.0, hero_rect.y + 44.0, hero_rect.width - 32.0, 22.0),
        "sans-serif",
        12.5,
        400,
        Color::rgb(166, 173, 200),
        TextAlign::Left,
        1.0,
        Some(hero_rect),
    );
    font_mgr.draw_text(
        pm,
        "• 5 Layout Modes (Tiling, Scrolling, BSP, Monocle, Floating)   • Live IPC & Hot-Reload",
        Rect::new(hero_rect.x + 16.0, hero_rect.y + 66.0, hero_rect.width - 32.0, 22.0),
        "sans-serif",
        12.0,
        500,
        Color::rgb(137, 180, 250),
        TextAlign::Left,
        1.0,
        Some(hero_rect),
    );

    // Feature grid cards below hero
    let cards = [
        (
            "1. Reactive HTML DOM",
            "Bind wm.*, sys.*, apps.*, notif.* directly via {{ expr }} & data-for",
            Color::rgb(137, 180, 250),
        ),
        (
            "2. Pure-Rust CSS3",
            "Flexbox, Grid, var(--*), linear-gradient, :hover, :focus, box-shadow",
            Color::rgb(203, 166, 247),
        ),
        (
            "3. Zero WebKit / Electron",
            "Ultra-low memory footprint (< 25MB RAM) with instant 2ms redraws",
            Color::rgb(166, 227, 161),
        ),
        (
            "4. Live DevTools CLI",
            "Inspect DOM & styles in real time with `luminactl dom style`",
            Color::rgb(249, 226, 175),
        ),
    ];

    let card_y_start = hero_rect.bottom() + 14.0;
    let col_w = ((content_w - 14.0) * 0.5).max(80.0);
    let card_h = 68.0;

    for (i, (head, desc, accent)) in cards.iter().enumerate() {
        let col = (i % 2) as f32;
        let row = (i / 2) as f32;
        let cx = content_x + col * (col_w + 14.0);
        let cy = card_y_start + row * (card_h + 12.0);
        if cy + card_h > rect.bottom() - 8.0 {
            break;
        }
        let c_rect = Rect::new(cx, cy, col_w, card_h);
        draw_background(
            pm,
            c_rect,
            [10.0; 4],
            Color::rgba(31, 32, 48, 250),
            None,
            1.0,
        );
        draw_border(
            pm,
            c_rect,
            [10.0; 4],
            Edges::all(1.0),
            Color::rgba(accent.r, accent.g, accent.b, 100),
            1.0,
        );
        font_mgr.draw_text(
            pm,
            head,
            Rect::new(cx + 12.0, cy + 10.0, col_w - 24.0, 22.0),
            "sans-serif",
            13.0,
            700,
            *accent,
            TextAlign::Left,
            1.0,
            Some(c_rect),
        );
        font_mgr.draw_text(
            pm,
            desc,
            Rect::new(cx + 12.0, cy + 34.0, col_w - 24.0, 24.0),
            "sans-serif",
            11.5,
            400,
            Color::rgb(180, 188, 214),
            TextAlign::Left,
            1.0,
            Some(c_rect),
        );
    }
}

fn paint_files_app(
    pm: &mut tiny_skia::PixmapMut<'_>,
    font_mgr: &FontManager,
    rect: Rect,
    corners: [f32; 4],
    theme: &WindowDecorationTheme,
) {
    draw_background(pm, rect, corners, Color::rgba(24, 24, 37, 252), None, 1.0);

    // Sidebar
    let side_w = 150.0f32.min(rect.width * 0.35);
    let side_rect = Rect::new(rect.x, rect.y, side_w, rect.height);
    draw_background(
        pm,
        side_rect,
        [0.0, 0.0, 0.0, corners[3]],
        Color::rgba(19, 19, 30, 255),
        None,
        1.0,
    );

    let places = [
        "★  lumina-wm",
        "⌂  Home",
        "▤  assets/",
        "⚙  crates/",
        "⬇  Downloads",
    ];
    for (idx, place) in places.iter().enumerate() {
        let py = rect.y + 12.0 + (idx as f32) * 32.0;
        let item_r = Rect::new(rect.x + 8.0, py, side_w - 16.0, 26.0);
        if idx == 0 {
            draw_background(
                pm,
                item_r,
                [6.0; 4],
                Color::rgba(137, 180, 250, 45),
                None,
                1.0,
            );
        }
        font_mgr.draw_text(
            pm,
            place,
            item_r.inset(Edges::xy(8.0, 3.0)),
            "sans-serif",
            12.0,
            if idx == 0 { 700 } else { 400 },
            if idx == 0 {
                theme.border_active
            } else {
                theme.text_active
            },
            TextAlign::Left,
            1.0,
            Some(item_r),
        );
    }

    // Main file grid
    let main_x = rect.x + side_w + 16.0;
    let main_w = (rect.width - side_w - 32.0).max(60.0);
    let entries = [
        ("assets/", "Folder", Color::rgb(137, 180, 250)),
        ("crates/", "Folder", Color::rgb(137, 180, 250)),
        ("topbar.html", "HTML Surface", Color::rgb(249, 226, 175)),
        ("dock.html", "HTML Surface", Color::rgb(249, 226, 175)),
        ("launcher.html", "HTML Modal", Color::rgb(249, 226, 175)),
        ("theme-catppuccin.css", "CSS3 Theme", Color::rgb(203, 166, 247)),
        ("theme-cyberpunk.css", "CSS3 Theme", Color::rgb(203, 166, 247)),
        ("Cargo.toml", "Rust Workspace", Color::rgb(166, 227, 161)),
    ];

    let cols = ((main_w / 155.0).floor() as usize).clamp(1, 4);
    let cell_w = ((main_w - ((cols - 1) as f32 * 10.0)) / cols as f32).max(90.0);
    let cell_h = 56.0;

    for (i, (name, kind, col)) in entries.iter().enumerate() {
        let c = i % cols;
        let r = i / cols;
        let ex = main_x + c as f32 * (cell_w + 10.0);
        let ey = rect.y + 14.0 + r as f32 * (cell_h + 10.0);
        if ey + cell_h > rect.bottom() - 8.0 {
            break;
        }
        let card = Rect::new(ex, ey, cell_w, cell_h);
        draw_background(pm, card, [8.0; 4], Color::rgba(34, 35, 52, 255), None, 1.0);
        draw_border(
            pm,
            card,
            [8.0; 4],
            Edges::all(1.0),
            Color::rgba(col.r, col.g, col.b, 90),
            1.0,
        );
        font_mgr.draw_text(
            pm,
            name,
            Rect::new(ex + 10.0, ey + 8.0, cell_w - 20.0, 20.0),
            "sans-serif",
            12.0,
            600,
            *col,
            TextAlign::Left,
            1.0,
            Some(card),
        );
        font_mgr.draw_text(
            pm,
            kind,
            Rect::new(ex + 10.0, ey + 28.0, cell_w - 20.0, 18.0),
            "sans-serif",
            11.0,
            400,
            theme.text_inactive,
            TextAlign::Left,
            1.0,
            Some(card),
        );
    }
}

fn paint_code_editor_app(
    pm: &mut tiny_skia::PixmapMut<'_>,
    font_mgr: &FontManager,
    rect: Rect,
    corners: [f32; 4],
    theme: &WindowDecorationTheme,
) {
    draw_background(pm, rect, corners, Color::rgba(20, 20, 32, 252), None, 1.0);

    let code_lines: &[(&str, Color)] = &[
        (
            " 1  /* assets/css/theme-catppuccin.css — Live Hot-Reload */",
            Color::rgb(127, 132, 156),
        ),
        (" 2  :root {", Color::rgb(203, 166, 247)),
        (" 3    --bg-surface: rgba(30, 30, 46, 0.92);", Color::rgb(137, 180, 250)),
        (" 4    --accent: #89b4fa;", Color::rgb(137, 180, 250)),
        (" 5    --wm-gap-inner: 8px;", Color::rgb(166, 227, 161)),
        (" 6    --wm-border-radius: 10px;", Color::rgb(166, 227, 161)),
        (" 7  }", Color::rgb(203, 166, 247)),
        (" 8", Color::rgb(127, 132, 156)),
        (" 9  #topbar.surface-bar {", Color::rgb(249, 226, 175)),
        ("10    display: flex;", Color::rgb(205, 214, 244)),
        ("11    justify-content: space-between;", Color::rgb(205, 214, 244)),
        ("12    background-color: var(--bg-surface);", Color::rgb(205, 214, 244)),
        ("13  }", Color::rgb(249, 226, 175)),
    ];

    let mut y = rect.y + 12.0;
    for &(line, col) in code_lines {
        if y + 20.0 > rect.bottom() - 8.0 {
            break;
        }
        let lr = Rect::new(rect.x + 14.0, y, (rect.width - 28.0).max(40.0), 20.0);
        font_mgr.draw_text(
            pm,
            line,
            lr,
            "monospace",
            12.5,
            500,
            col,
            TextAlign::Left,
            1.0,
            Some(lr),
        );
        y += 20.0;
    }
    let _ = theme;
}

fn paint_generic_app(
    pm: &mut tiny_skia::PixmapMut<'_>,
    font_mgr: &FontManager,
    rect: Rect,
    corners: [f32; 4],
    app_id: &str,
    title: &str,
    theme: &WindowDecorationTheme,
) {
    draw_background(pm, rect, corners, Color::rgba(24, 25, 38, 252), None, 1.0);

    let card = Rect::new(
        rect.x + 20.0,
        rect.y + 20.0,
        (rect.width - 40.0).max(60.0),
        (rect.height - 40.0).min(140.0).max(40.0),
    );
    draw_background(pm, card, [12.0; 4], Color::rgba(34, 36, 54, 255), None, 1.0);
    draw_border(
        pm,
        card,
        [12.0; 4],
        Edges::all(1.0),
        theme.border_active,
        1.0,
    );

    font_mgr.draw_text(
        pm,
        title,
        Rect::new(card.x + 16.0, card.y + 14.0, card.width - 32.0, 26.0),
        "sans-serif",
        15.0,
        700,
        theme.text_active,
        TextAlign::Left,
        1.0,
        Some(card),
    );
    font_mgr.draw_text(
        pm,
        &format!("Wayland Client Surface ({app_id}) — Active in Lumina WM"),
        Rect::new(card.x + 16.0, card.y + 44.0, card.width - 32.0, 22.0),
        "sans-serif",
        12.5,
        400,
        theme.border_active,
        TextAlign::Left,
        1.0,
        Some(card),
    );
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
