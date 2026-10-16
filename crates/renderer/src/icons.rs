use crate::primitives::{build_rounded_rect_path, to_skia_color};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, PathBuilder, PixmapMut, Stroke, Transform,
};
use wm_proto::{Color, DomNodeData, Rect};

/// Extracts the icon identifier if the DOM node represents a vector icon element
/// (`<svg>`, `<icon>`, `data-icon="..."`, `icon="..."`, or `icon-*` CSS class).
pub fn detect_icon_name(node: &DomNodeData) -> Option<String> {
    if let Some(icon) = node
        .get_attr("data-icon")
        .or_else(|| node.get_attr("icon"))
        .or_else(|| node.get_attr("name"))
    {
        if node.tag == "icon"
            || node.tag == "svg"
            || node.tag == "i"
            || node.attributes.contains_key("data-icon")
            || node.attributes.contains_key("icon")
        {
            let trimmed = icon.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_ascii_lowercase());
            }
        }
    }

    for class in &node.classes {
        if let Some(rest) = class.strip_prefix("icon-") {
            if !rest.is_empty() {
                return Some(rest.to_ascii_lowercase());
            }
        }
    }

    if node.tag == "icon" || node.tag == "svg" {
        if let Some(txt) = &node.text_content {
            let trimmed = txt.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_ascii_lowercase());
            }
        }
        return Some("generic".to_string());
    }

    None
}

/// Renders a named vector icon or custom SVG path inside `bounds`.
pub fn draw_vector_icon(
    pixmap: &mut PixmapMut<'_>,
    icon_name: &str,
    node: &DomNodeData,
    bounds: Rect,
    color: Color,
    opacity: f32,
) {
    if bounds.width <= 1.0 || bounds.height <= 1.0 || color.is_transparent() || opacity <= 0.001 {
        return;
    }

    // If the node has an explicit SVG `d` path attribute (`d="..."` or `data-path="..."`), render it
    if let Some(d_str) = node.get_attr("d").or_else(|| node.get_attr("data-path")) {
        if draw_svg_path_data(pixmap, d_str, bounds, color, opacity) {
            return;
        }
    }

    let size = bounds.width.min(bounds.height).clamp(8.0, 48.0);
    let cx = bounds.x + bounds.width * 0.5;
    let cy = bounds.y + bounds.height * 0.5;
    let r = size * 0.38;
    let stroke_w = (size * 0.11).clamp(1.3, 2.6);

    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(to_skia_color(color, opacity));

    let stroke = Stroke {
        width: stroke_w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };

    match icon_name {
        "close" | "x" | "✕" | "×" => {
            let d = r * 0.72;
            let mut pb = PathBuilder::new();
            pb.move_to(cx - d, cy - d);
            pb.line_to(cx + d, cy + d);
            pb.move_to(cx + d, cy - d);
            pb.line_to(cx - d, cy + d);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        "maximize" | "fullscreen" | "square" | "□" => {
            let d = r * 0.78;
            let sq = Rect::new(cx - d, cy - d, d * 2.0, d * 2.0);
            if let Some(path) = build_rounded_rect_path(sq, [2.0; 4]) {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        "minimize" | "minus" | "—" | "-" => {
            let d = r * 0.78;
            let mut pb = PathBuilder::new();
            pb.move_to(cx - d, cy);
            pb.line_to(cx + d, cy);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        "float" | "floating" | "restore" | "window" | "windows" => {
            let d = r * 0.65;
            let front = Rect::new(cx - d, cy - d * 0.6, d * 1.45, d * 1.45);
            let back = Rect::new(cx - d * 0.45, cy - d, d * 1.45, d * 1.45);
            if let Some(p1) = build_rounded_rect_path(front, [1.8; 4]) {
                pixmap.stroke_path(&p1, &paint, &stroke, Transform::identity(), None);
            }
            if let Some(p2) = build_rounded_rect_path(back, [1.8; 4]) {
                pixmap.stroke_path(&p2, &paint, &stroke, Transform::identity(), None);
            }
        }
        "launcher" | "apps" | "grid" | "lumina" | "menu" => {
            // 2x2 rounded grid icon
            let cell = (r * 0.78).max(2.5);
            let gap = (r * 0.22).max(1.0);
            let offsets = [
                (cx - gap * 0.5 - cell, cy - gap * 0.5 - cell),
                (cx + gap * 0.5, cy - gap * 0.5 - cell),
                (cx - gap * 0.5 - cell, cy + gap * 0.5),
                (cx + gap * 0.5, cy + gap * 0.5),
            ];
            for (ox, oy) in offsets {
                let rect = Rect::new(ox, oy, cell, cell);
                if let Some(path) = build_rounded_rect_path(rect, [cell * 0.28; 4]) {
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
        "search" | "magnifier" | "find" => {
            let cr = r * 0.62;
            let scx = cx - r * 0.15;
            let scy = cy - r * 0.15;
            if let Some(circle) = PathBuilder::from_circle(scx, scy, cr) {
                pixmap.stroke_path(&circle, &paint, &stroke, Transform::identity(), None);
            }
            let mut pb = PathBuilder::new();
            let start = cr * 0.707;
            pb.move_to(scx + start, scy + start);
            pb.line_to(cx + r * 0.78, cy + r * 0.78);
            if let Some(handle) = pb.finish() {
                pixmap.stroke_path(&handle, &paint, &stroke, Transform::identity(), None);
            }
        }
        "volume" | "audio" | "speaker" | "sound" | "mute" | "volume-mute" => {
            // Speaker cone
            let mut pb = PathBuilder::new();
            pb.move_to(cx - r * 0.75, cy - r * 0.28);
            pb.line_to(cx - r * 0.35, cy - r * 0.28);
            pb.line_to(cx + r * 0.08, cy - r * 0.68);
            pb.line_to(cx + r * 0.08, cy + r * 0.68);
            pb.line_to(cx - r * 0.35, cy + r * 0.28);
            pb.line_to(cx - r * 0.75, cy + r * 0.28);
            pb.close();
            if let Some(cone) = pb.finish() {
                pixmap.fill_path(
                    &cone,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            if icon_name.contains("mute") {
                let mut xpb = PathBuilder::new();
                xpb.move_to(cx + r * 0.30, cy - r * 0.30);
                xpb.line_to(cx + r * 0.78, cy + r * 0.30);
                xpb.move_to(cx + r * 0.78, cy - r * 0.30);
                xpb.line_to(cx + r * 0.30, cy + r * 0.30);
                if let Some(xpath) = xpb.finish() {
                    pixmap.stroke_path(&xpath, &paint, &stroke, Transform::identity(), None);
                }
            } else {
                let mut wpb = PathBuilder::new();
                wpb.move_to(cx + r * 0.32, cy - r * 0.35);
                wpb.quad_to(cx + r * 0.62, cy, cx + r * 0.32, cy + r * 0.35);
                wpb.move_to(cx + r * 0.55, cy - r * 0.62);
                wpb.quad_to(cx + r * 0.95, cy, cx + r * 0.55, cy + r * 0.62);
                if let Some(waves) = wpb.finish() {
                    pixmap.stroke_path(&waves, &paint, &stroke, Transform::identity(), None);
                }
            }
        }
        "wifi" | "network" | "net" => {
            let mut pb = PathBuilder::new();
            // Outer arc
            pb.move_to(cx - r * 0.85, cy - r * 0.20);
            pb.quad_to(cx, cy - r * 0.95, cx + r * 0.85, cy - r * 0.20);
            // Middle arc
            pb.move_to(cx - r * 0.52, cy + r * 0.15);
            pb.quad_to(cx, cy - r * 0.35, cx + r * 0.52, cy + r * 0.15);
            if let Some(arcs) = pb.finish() {
                pixmap.stroke_path(&arcs, &paint, &stroke, Transform::identity(), None);
            }
            if let Some(dot) = PathBuilder::from_circle(cx, cy + r * 0.55, stroke_w * 0.85) {
                pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
        "battery" | "battery-charging" | "bat" => {
            let bw = r * 1.45;
            let bh = r * 0.88;
            let bx = cx - bw * 0.55;
            let by = cy - bh * 0.5;
            let body = Rect::new(bx, by, bw, bh);
            if let Some(path) = build_rounded_rect_path(body, [2.0; 4]) {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
            // Positive terminal nub
            let nub = Rect::new(bx + bw + 1.0, cy - bh * 0.25, stroke_w * 1.1, bh * 0.5);
            if let Some(npath) = build_rounded_rect_path(nub, [1.0; 4]) {
                pixmap.fill_path(
                    &npath,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            // Inner charge fill
            let fill_rect = Rect::new(
                bx + stroke_w * 1.1,
                by + stroke_w * 1.1,
                (bw - stroke_w * 2.2) * 0.75,
                (bh - stroke_w * 2.2).max(1.0),
            );
            if let Some(fpath) = build_rounded_rect_path(fill_rect, [1.0; 4]) {
                pixmap.fill_path(
                    &fpath,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        "bell" | "notification" | "notifications" | "notif" => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx - r * 0.65, cy + r * 0.35);
            pb.line_to(cx + r * 0.65, cy + r * 0.35);
            pb.line_to(cx + r * 0.48, cy - r * 0.25);
            pb.quad_to(cx, cy - r * 0.88, cx - r * 0.48, cy - r * 0.25);
            pb.close();
            if let Some(bell) = pb.finish() {
                pixmap.stroke_path(&bell, &paint, &stroke, Transform::identity(), None);
            }
            if let Some(clapper) = PathBuilder::from_circle(cx, cy + r * 0.65, stroke_w * 0.75) {
                pixmap.fill_path(
                    &clapper,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        "cpu" | "processor" | "chip" => {
            let d = r * 0.58;
            let body = Rect::new(cx - d, cy - d, d * 2.0, d * 2.0);
            if let Some(path) = build_rounded_rect_path(body, [2.0; 4]) {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
            // Pins on 4 sides
            let mut pb = PathBuilder::new();
            for offset in [-d * 0.45, d * 0.45] {
                pb.move_to(cx + offset, cy - r * 0.92);
                pb.line_to(cx + offset, cy - d);
                pb.move_to(cx + offset, cy + d);
                pb.line_to(cx + offset, cy + r * 0.92);
                pb.move_to(cx - r * 0.92, cy + offset);
                pb.line_to(cx - d, cy + offset);
                pb.move_to(cx + d, cy + offset);
                pb.line_to(cx + r * 0.92, cy + offset);
            }
            if let Some(pins) = pb.finish() {
                pixmap.stroke_path(&pins, &paint, &stroke, Transform::identity(), None);
            }
        }
        "ram" | "memory" => {
            let body = Rect::new(cx - r * 0.85, cy - r * 0.45, r * 1.7, r * 0.75);
            if let Some(path) = build_rounded_rect_path(body, [1.8; 4]) {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
            let mut pb = PathBuilder::new();
            for offset in [-r * 0.5, 0.0, r * 0.5] {
                pb.move_to(cx + offset, cy + r * 0.30);
                pb.line_to(cx + offset, cy + r * 0.68);
            }
            if let Some(pins) = pb.finish() {
                pixmap.stroke_path(&pins, &paint, &stroke, Transform::identity(), None);
            }
        }
        "clock" | "time" => {
            if let Some(circle) = PathBuilder::from_circle(cx, cy, r * 0.82) {
                pixmap.stroke_path(&circle, &paint, &stroke, Transform::identity(), None);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy - r * 0.48);
            pb.line_to(cx, cy);
            pb.line_to(cx + r * 0.38, cy + r * 0.22);
            if let Some(hands) = pb.finish() {
                pixmap.stroke_path(&hands, &paint, &stroke, Transform::identity(), None);
            }
        }
        "terminal" | "console" => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx - r * 0.65, cy - r * 0.38);
            pb.line_to(cx - r * 0.15, cy + r * 0.05);
            pb.line_to(cx - r * 0.65, cy + r * 0.48);
            pb.move_to(cx + r * 0.05, cy + r * 0.48);
            pb.line_to(cx + r * 0.65, cy + r * 0.48);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        "layout" | "tiling" | "columns" => {
            let outer = Rect::new(cx - r * 0.8, cy - r * 0.65, r * 1.6, r * 1.3);
            if let Some(path) = build_rounded_rect_path(outer, [2.0; 4]) {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy - r * 0.65);
            pb.line_to(cx, cy + r * 0.65);
            pb.move_to(cx, cy);
            pb.line_to(cx + r * 0.8, cy);
            if let Some(dividers) = pb.finish() {
                pixmap.stroke_path(&dividers, &paint, &stroke, Transform::identity(), None);
            }
        }
        "power" | "shutdown" => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy - r * 0.85);
            pb.line_to(cx, cy - r * 0.05);
            pb.move_to(cx - r * 0.55, cy - r * 0.48);
            pb.cubic_to(
                cx - r * 0.95,
                cy + r * 0.15,
                cx - r * 0.55,
                cy + r * 0.85,
                cx,
                cy + r * 0.85,
            );
            pb.cubic_to(
                cx + r * 0.55,
                cy + r * 0.85,
                cx + r * 0.95,
                cy + r * 0.15,
                cx + r * 0.55,
                cy - r * 0.48,
            );
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        _ => {
            // Default clean geometric circle/diamond indicator
            if let Some(circle) = PathBuilder::from_circle(cx, cy, r * 0.65) {
                pixmap.stroke_path(&circle, &paint, &stroke, Transform::identity(), None);
            }
            if let Some(dot) = PathBuilder::from_circle(cx, cy, r * 0.25) {
                pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
    }
}

/// Parses a minimal subset of SVG path data (`M x y`, `L x y`, `Z`) normalized into `bounds` (0..24 viewBox).
fn draw_svg_path_data(
    pixmap: &mut PixmapMut<'_>,
    d: &str,
    bounds: Rect,
    color: Color,
    opacity: f32,
) -> bool {
    let tokens: Vec<&str> = d
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.is_empty() {
        return false;
    }

    let scale_x = (bounds.width / 24.0).max(0.1);
    let scale_y = (bounds.height / 24.0).max(0.1);

    let mut pb = PathBuilder::new();
    let mut i = 0usize;
    let mut has_segments = false;

    while i < tokens.len() {
        match tokens[i] {
            "M" if i + 2 < tokens.len() => {
                if let (Ok(x), Ok(y)) = (tokens[i + 1].parse::<f32>(), tokens[i + 2].parse::<f32>())
                {
                    pb.move_to(bounds.x + x * scale_x, bounds.y + y * scale_y);
                }
                i += 3;
            }
            "L" if i + 2 < tokens.len() => {
                if let (Ok(x), Ok(y)) = (tokens[i + 1].parse::<f32>(), tokens[i + 2].parse::<f32>())
                {
                    pb.line_to(bounds.x + x * scale_x, bounds.y + y * scale_y);
                    has_segments = true;
                }
                i += 3;
            }
            "Z" | "z" => {
                pb.close();
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !has_segments {
        return false;
    }

    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.set_color(to_skia_color(color, opacity));
        let stroke = Stroke {
            width: (scale_x.min(scale_y) * 2.0).clamp(1.2, 2.5),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        return true;
    }
    false
}
