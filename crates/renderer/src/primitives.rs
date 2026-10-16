use tiny_skia::{
    Color as SkiaColor, FillRule, GradientStop as SkiaGradientStop, LinearGradient as SkiaGradient,
    Paint, Path, PathBuilder, PixmapMut, Point as SkiaPoint, SpreadMode, Stroke, Transform,
};
use wm_proto::{BoxShadow, Color, Edges, LinearGradient, Rect};

const KAPPA: f32 = 0.552_284_8;

#[inline]
pub fn to_skia_color(c: Color, opacity: f32) -> SkiaColor {
    let a = ((c.a as f32 / 255.0) * opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    SkiaColor::from_rgba(
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        a,
    )
    .unwrap_or(SkiaColor::TRANSPARENT)
}

/// Builds a smooth anti-aliased rounded rectangle `tiny_skia::Path` honoring
/// per-corner radii `[top_left, top_right, bottom_right, bottom_left]`.
pub fn build_rounded_rect_path(rect: Rect, radii: [f32; 4]) -> Option<Path> {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }

    let mut tl = radii[0].max(0.0);
    let mut tr = radii[1].max(0.0);
    let mut br = radii[2].max(0.0);
    let mut bl = radii[3].max(0.0);

    // Normalize corner radii per CSS Backgrounds & Borders spec if their sum exceeds dimensions
    let top_sum = tl + tr;
    let bottom_sum = bl + br;
    let left_sum = tl + bl;
    let right_sum = tr + br;

    let mut scale = 1.0f32;
    if top_sum > rect.width && top_sum > 0.0 {
        scale = scale.min(rect.width / top_sum);
    }
    if bottom_sum > rect.width && bottom_sum > 0.0 {
        scale = scale.min(rect.width / bottom_sum);
    }
    if left_sum > rect.height && left_sum > 0.0 {
        scale = scale.min(rect.height / left_sum);
    }
    if right_sum > rect.height && right_sum > 0.0 {
        scale = scale.min(rect.height / right_sum);
    }

    if scale < 1.0 {
        tl *= scale;
        tr *= scale;
        br *= scale;
        bl *= scale;
    }

    let x = rect.x;
    let y = rect.y;
    let w = rect.width;
    let h = rect.height;
    let r = x + w;
    let b = y + h;

    let mut pb = PathBuilder::new();

    if tl <= 0.01 && tr <= 0.01 && br <= 0.01 && bl <= 0.01 {
        pb.move_to(x, y);
        pb.line_to(r, y);
        pb.line_to(r, b);
        pb.line_to(x, b);
        pb.close();
        return pb.finish();
    }

    // Start after top-left corner on top edge
    pb.move_to(x + tl, y);

    // Top edge & top-right corner
    pb.line_to(r - tr, y);
    if tr > 0.01 {
        let c = tr * KAPPA;
        pb.cubic_to(r - tr + c, y, r, y + tr - c, r, y + tr);
    }

    // Right edge & bottom-right corner
    pb.line_to(r, b - br);
    if br > 0.01 {
        let c = br * KAPPA;
        pb.cubic_to(r, b - br + c, r - br + c, b, r - br, b);
    }

    // Bottom edge & bottom-left corner
    pb.line_to(x + bl, b);
    if bl > 0.01 {
        let c = bl * KAPPA;
        pb.cubic_to(x + bl - c, b, x, b - bl + c, x, b - bl);
    }

    // Left edge & top-left corner
    pb.line_to(x, y + tl);
    if tl > 0.01 {
        let c = tl * KAPPA;
        pb.cubic_to(x, y + tl - c, x + tl - c, y, x + tl, y);
    }

    pb.close();
    pb.finish()
}

/// Renders a soft multi-pass rounded rectangle drop shadow (or inset shadow) behind the element.
pub fn draw_box_shadow(
    pixmap: &mut PixmapMut<'_>,
    rect: Rect,
    radii: [f32; 4],
    shadow: &BoxShadow,
    opacity: f32,
) {
    if shadow.color.is_transparent() || opacity <= 0.001 {
        return;
    }

    let base_alpha = (shadow.color.a as f32 / 255.0) * opacity.clamp(0.0, 1.0);
    if base_alpha <= 0.002 {
        return;
    }

    if shadow.inset {
        // Inset shadow along the inside perimeter
        let stroke_w = (shadow.blur_radius * 0.5 + shadow.spread_radius.max(1.0)).max(1.5);
        let inset_rect = rect.inset(Edges::all(stroke_w * 0.5));
        if let Some(path) = build_rounded_rect_path(inset_rect, radii) {
            let mut paint = Paint::default();
            paint.anti_alias = true;
            paint.set_color(to_skia_color(shadow.color, opacity * 0.65));
            let stroke = Stroke {
                width: stroke_w,
                ..Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
        return;
    }

    let blur = shadow.blur_radius.max(0.0);
    let spread = shadow.spread_radius;
    let base_rect = Rect::new(
        rect.x + shadow.offset_x - spread,
        rect.y + shadow.offset_y - spread,
        (rect.width + spread * 2.0).max(0.0),
        (rect.height + spread * 2.0).max(0.0),
    );

    if blur <= 0.5 {
        let expanded_radii = radii.map(|r| (r + spread).max(0.0));
        if let Some(path) = build_rounded_rect_path(base_rect, expanded_radii) {
            let mut paint = Paint::default();
            paint.anti_alias = true;
            paint.set_color(to_skia_color(shadow.color, opacity));
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        return;
    }

    // Multi-pass soft feathered drop shadow
    let passes = (blur.ceil() as usize).clamp(4, 10);
    // Normalize pass weights so total accumulated alpha approximates base_alpha at center
    let pass_alpha_scale = 1.45 / (passes as f32);

    for i in (0..passes).rev() {
        let t = (i as f32 + 0.5) / (passes as f32); // 0..1 from inner to outer
        let expand = t * blur * 1.15;
        let falloff = (1.0 - t * 0.85).powi(2);
        let pass_opacity = (opacity * falloff * pass_alpha_scale).clamp(0.003, 1.0);

        let r_rect = Rect::new(
            base_rect.x - expand,
            base_rect.y - expand,
            base_rect.width + expand * 2.0,
            base_rect.height + expand * 2.0,
        );
        let r_radii = radii.map(|r| (r + spread + expand).max(0.0));

        if let Some(path) = build_rounded_rect_path(r_rect, r_radii) {
            let mut paint = Paint::default();
            paint.anti_alias = true;
            paint.set_color(to_skia_color(shadow.color, pass_opacity));
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// Fills an anti-aliased rounded rectangle with `background_color` and/or `background_gradient`.
pub fn draw_background(
    pixmap: &mut PixmapMut<'_>,
    rect: Rect,
    radii: [f32; 4],
    bg_color: Color,
    bg_gradient: Option<&LinearGradient>,
    opacity: f32,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 || opacity <= 0.001 {
        return;
    }

    let has_solid = !bg_color.is_transparent();
    let has_grad = bg_gradient.is_some_and(|g| !g.stops.is_empty());
    if !has_solid && !has_grad {
        return;
    }

    let Some(path) = build_rounded_rect_path(rect, radii) else {
        return;
    };

    if has_solid {
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.set_color(to_skia_color(bg_color, opacity));
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    if let Some(grad) = bg_gradient {
        if grad.stops.len() >= 2 {
            let (start, end) = compute_gradient_endpoints(rect, grad.angle_deg);
            let stops: Vec<SkiaGradientStop> = grad
                .stops
                .iter()
                .map(|s| {
                    SkiaGradientStop::new(
                        s.offset.clamp(0.0, 1.0),
                        to_skia_color(s.color, opacity),
                    )
                })
                .collect();

            if let Some(shader) = SkiaGradient::new(
                start,
                end,
                stops,
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                let mut paint = Paint::default();
                paint.anti_alias = true;
                paint.shader = shader;
                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        } else if let Some(single) = grad.stops.first() {
            let mut paint = Paint::default();
            paint.anti_alias = true;
            paint.set_color(to_skia_color(single.color, opacity));
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// Computes start and end points for a CSS linear-gradient angle in degrees.
/// CSS convention: 0deg = bottom-to-top, 90deg = left-to-right, 180deg = top-to-bottom.
fn compute_gradient_endpoints(rect: Rect, angle_deg: f32) -> (SkiaPoint, SkiaPoint) {
    let cx = rect.x + rect.width * 0.5;
    let cy = rect.y + rect.height * 0.5;

    let rad = (angle_deg - 90.0).to_radians();
    let dx = rad.cos();
    let dy = rad.sin();

    let half_len = (rect.width * dx.abs() + rect.height * dy.abs()) * 0.5;
    let half_len = half_len.max(1.0);

    let start = SkiaPoint::from_xy(cx - dx * half_len, cy - dy * half_len);
    let end = SkiaPoint::from_xy(cx + dx * half_len, cy + dy * half_len);
    (start, end)
}

/// Strokes the border of a rounded rectangle (or individual edges if asymmetric).
pub fn draw_border(
    pixmap: &mut PixmapMut<'_>,
    rect: Rect,
    radii: [f32; 4],
    border_width: Edges,
    border_color: Color,
    opacity: f32,
) {
    if border_color.is_transparent() || opacity <= 0.001 {
        return;
    }

    let max_bw = border_width
        .top
        .max(border_width.right)
        .max(border_width.bottom)
        .max(border_width.left);
    if max_bw <= 0.01 {
        return;
    }

    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(to_skia_color(border_color, opacity));

    let all_sides_present = border_width.top > 0.01
        && border_width.right > 0.01
        && border_width.bottom > 0.01
        && border_width.left > 0.01;

    if all_sides_present {
        let avg_bw =
            (border_width.top + border_width.right + border_width.bottom + border_width.left)
                * 0.25;
        let half = avg_bw * 0.5;
        let stroke_rect = rect.inset(Edges::all(half));
        let stroke_radii = radii.map(|r| (r - half).max(0.0));
        if let Some(path) = build_rounded_rect_path(stroke_rect, stroke_radii) {
            let stroke = Stroke {
                width: avg_bw,
                ..Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    } else {
        // Draw active individual border edges (e.g. border-bottom on a header or border-left on an accent bar)
        if border_width.top > 0.01 {
            draw_line_segment(
                pixmap,
                rect.x,
                rect.y + border_width.top * 0.5,
                rect.right(),
                rect.y + border_width.top * 0.5,
                border_width.top,
                &paint,
            );
        }
        if border_width.bottom > 0.01 {
            draw_line_segment(
                pixmap,
                rect.x,
                rect.bottom() - border_width.bottom * 0.5,
                rect.right(),
                rect.bottom() - border_width.bottom * 0.5,
                border_width.bottom,
                &paint,
            );
        }
        if border_width.left > 0.01 {
            draw_line_segment(
                pixmap,
                rect.x + border_width.left * 0.5,
                rect.y,
                rect.x + border_width.left * 0.5,
                rect.bottom(),
                border_width.left,
                &paint,
            );
        }
        if border_width.right > 0.01 {
            draw_line_segment(
                pixmap,
                rect.right() - border_width.right * 0.5,
                rect.y,
                rect.right() - border_width.right * 0.5,
                rect.bottom(),
                border_width.right,
                &paint,
            );
        }
    }
}

fn draw_line_segment(
    pixmap: &mut PixmapMut<'_>,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    width: f32,
    paint: &Paint<'_>,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: width.max(1.0),
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, paint, &stroke, Transform::identity(), None);
    }
}
