use crate::config::WmThemeConfig;
use std::collections::HashMap;
use wm_proto::{LayoutMode, Rect, WindowDescriptor, WindowId, WindowPlacement};

/// Returns the inner layout area after subtracting `outer_gap` from `usable_rect`.
pub fn available_rect(usable_rect: Rect, outer_gap: f32) -> Rect {
    Rect {
        x: usable_rect.x + outer_gap,
        y: usable_rect.y + outer_gap,
        width: (usable_rect.width - 2.0 * outer_gap).max(1.0),
        height: (usable_rect.height - 2.0 * outer_gap).max(1.0),
    }
}

/// Master-Stack tiling layout (`LayoutMode::Tiling`).
pub fn compute_tiling_rects(
    count: usize,
    avail: Rect,
    inner_gap: f32,
    master_ratio: f32,
) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![avail];
    }

    let ratio = master_ratio.clamp(0.15, 0.85);
    let master_w = ((avail.width - inner_gap) * ratio).max(40.0);
    let stack_x = avail.x + master_w + inner_gap;
    let stack_w = (avail.width - master_w - inner_gap).max(40.0);

    let mut out = Vec::with_capacity(count);
    // Master window on the left
    out.push(Rect::new(avail.x, avail.y, master_w, avail.height));

    // Stack windows vertically on the right
    let stack_count = count - 1;
    let total_gaps = inner_gap * (stack_count.saturating_sub(1)) as f32;
    let each_h = ((avail.height - total_gaps) / stack_count as f32).max(20.0);

    for i in 0..stack_count {
        let y = avail.y + i as f32 * (each_h + inner_gap);
        out.push(Rect::new(stack_x, y, stack_w, each_h));
    }

    out
}

/// Niri/PaperWM-style horizontal scrolling strip of columns (`LayoutMode::Scrolling`).
pub fn compute_scrolling_rects(
    count: usize,
    focused_idx: usize,
    avail: Rect,
    inner_gap: f32,
    default_col_ratio: f32,
    custom_col_ratios: Option<&[f32]>,
) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }

    let base_ratio = default_col_ratio.clamp(0.25, 1.0);
    let mut col_widths = Vec::with_capacity(count);
    let mut col_xs = Vec::with_capacity(count);
    let mut cursor_x = 0.0f32;

    for i in 0..count {
        let r = custom_col_ratios
            .and_then(|s| s.get(i).copied())
            .unwrap_or(base_ratio)
            .clamp(0.25, 1.0);
        let w = (avail.width * r).max(100.0);
        col_xs.push(cursor_x);
        col_widths.push(w);
        cursor_x += w + if i + 1 < count { inner_gap } else { 0.0 };
    }

    let total_strip_width = cursor_x;
    let f = focused_idx.min(count - 1);
    let focus_center = col_xs[f] + col_widths[f] * 0.5;

    let scroll_offset = if total_strip_width <= avail.width {
        0.0
    } else {
        let max_scroll = (total_strip_width - avail.width).max(0.0);
        (focus_center - avail.width * 0.5).clamp(0.0, max_scroll)
    };

    (0..count)
        .map(|i| {
            Rect::new(
                avail.x + col_xs[i] - scroll_offset,
                avail.y,
                col_widths[i],
                avail.height,
            )
        })
        .collect()
}

/// Binary Space Partitioning / Dwindle layout (`LayoutMode::Bsp`).
/// Alternates vertical splits (left/right) and horizontal splits (top/bottom).
pub fn compute_bsp_rects(
    count: usize,
    avail: Rect,
    inner_gap: f32,
    split_ratio: f32,
) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let ratio = split_ratio.clamp(0.2, 0.8);
    let mut out = Vec::with_capacity(count);
    let mut rem = avail;

    for i in 0..count {
        if i + 1 == count {
            out.push(rem);
        } else if i % 2 == 0 {
            // Vertical split: left leaf + right remainder
            let w = ((rem.width - inner_gap) * ratio).max(30.0);
            out.push(Rect::new(rem.x, rem.y, w, rem.height));
            rem = Rect::new(
                rem.x + w + inner_gap,
                rem.y,
                (rem.width - w - inner_gap).max(30.0),
                rem.height,
            );
        } else {
            // Horizontal split: top leaf + bottom remainder
            let h = ((rem.height - inner_gap) * ratio).max(30.0);
            out.push(Rect::new(rem.x, rem.y, rem.width, h));
            rem = Rect::new(
                rem.x,
                rem.y + h + inner_gap,
                rem.width,
                (rem.height - h - inner_gap).max(30.0),
            );
        }
    }

    out
}

/// Monocle / Maximized deck layout (`LayoutMode::Monocle`).
pub fn compute_monocle_rects(count: usize, avail: Rect) -> Vec<Rect> {
    vec![avail; count]
}

/// Computes a default cascaded floating rect for a window if its current rect is empty.
pub fn default_floating_rect(index: usize, usable_rect: Rect) -> Rect {
    let w = (usable_rect.width * 0.55).clamp(360.0, 960.0);
    let h = (usable_rect.height * 0.60).clamp(240.0, 680.0);
    let cascade = (index % 6) as f32 * 28.0;
    let x = (usable_rect.x + (usable_rect.width - w) * 0.5 + cascade)
        .min((usable_rect.right() - w).max(usable_rect.x));
    let y = (usable_rect.y + (usable_rect.height - h) * 0.5 + cascade)
        .min((usable_rect.bottom() - h).max(usable_rect.y));
    Rect::new(x, y, w, h)
}

/// Builds a `WindowPlacement` with server-side decorations (titlebar + border) for a window.
pub fn build_window_placement(
    win: &WindowDescriptor,
    frame_rect: Rect,
    theme: &WmThemeConfig,
    is_floating_effective: bool,
    z_order: i32,
) -> WindowPlacement {
    if win.is_fullscreen {
        return WindowPlacement {
            window_id: win.id,
            frame_rect,
            content_rect: frame_rect,
            titlebar_rect: None,
            border_width: 0.0,
            border_radius: 0.0,
            is_focused: win.is_focused,
            is_floating: is_floating_effective,
            is_fullscreen: true,
            z_order,
        };
    }

    let bw = theme.border_width.max(0.0);
    let br = theme.border_radius.max(0.0);
    let inner_w = (frame_rect.width - 2.0 * bw).max(1.0);

    if theme.ssd_enabled && theme.titlebar_height > 0.0 {
        let th = theme
            .titlebar_height
            .min((frame_rect.height - 2.0 * bw).max(0.0));
        let titlebar_rect = Rect::new(frame_rect.x + bw, frame_rect.y + bw, inner_w, th);
        let content_h = (frame_rect.height - 2.0 * bw - th).max(1.0);
        let content_rect = Rect::new(
            frame_rect.x + bw,
            frame_rect.y + bw + th,
            inner_w,
            content_h,
        );
        WindowPlacement {
            window_id: win.id,
            frame_rect,
            content_rect,
            titlebar_rect: Some(titlebar_rect),
            border_width: bw,
            border_radius: br,
            is_focused: win.is_focused,
            is_floating: is_floating_effective,
            is_fullscreen: false,
            z_order,
        }
    } else {
        let content_h = (frame_rect.height - 2.0 * bw).max(1.0);
        let content_rect = Rect::new(frame_rect.x + bw, frame_rect.y + bw, inner_w, content_h);
        WindowPlacement {
            window_id: win.id,
            frame_rect,
            content_rect,
            titlebar_rect: None,
            border_width: bw,
            border_radius: br,
            is_focused: win.is_focused,
            is_floating: is_floating_effective,
            is_fullscreen: false,
            z_order,
        }
    }
}

/// Computes `WindowPlacement`s for all visible windows on a workspace given a `LayoutMode`.
pub fn compute_workspace_placements(
    windows: &[&WindowDescriptor],
    layout_mode: LayoutMode,
    usable_rect: Rect,
    theme: &WmThemeConfig,
    scrolling_ratios: &HashMap<WindowId, f32>,
    floating_z_stack: &[WindowId],
) -> Vec<WindowPlacement> {
    let visible: Vec<&WindowDescriptor> = windows
        .iter()
        .copied()
        .filter(|w| !w.is_minimized)
        .collect();

    if visible.is_empty() {
        return Vec::new();
    }

    let avail = available_rect(usable_rect, theme.outer_gap);

    // Separate tiled windows vs floating/fullscreen/maximized windows
    let all_floating_mode = layout_mode == LayoutMode::Floating;
    let tiled_windows: Vec<&WindowDescriptor> = if all_floating_mode {
        Vec::new()
    } else {
        visible
            .iter()
            .copied()
            .filter(|w| !w.is_floating && !w.is_fullscreen && !w.is_maximized)
            .collect()
    };

    let focused_tiled_idx = tiled_windows
        .iter()
        .position(|w| w.is_focused)
        .unwrap_or(0);

    let tiled_rects = match layout_mode {
        LayoutMode::Tiling => compute_tiling_rects(
            tiled_windows.len(),
            avail,
            theme.inner_gap,
            theme.master_ratio,
        ),
        LayoutMode::Scrolling => {
            let ratios: Vec<f32> = tiled_windows
                .iter()
                .map(|w| {
                    scrolling_ratios
                        .get(&w.id)
                        .copied()
                        .unwrap_or(theme.scrolling_column_ratio)
                })
                .collect();
            compute_scrolling_rects(
                tiled_windows.len(),
                focused_tiled_idx,
                avail,
                theme.inner_gap,
                theme.scrolling_column_ratio,
                Some(&ratios),
            )
        }
        LayoutMode::Bsp => compute_bsp_rects(
            tiled_windows.len(),
            avail,
            theme.inner_gap,
            theme.bsp_split_ratio,
        ),
        LayoutMode::Monocle => compute_monocle_rects(tiled_windows.len(), avail),
        LayoutMode::Floating => Vec::new(),
    };

    let mut tiled_rect_map: HashMap<WindowId, ( usize, Rect )> = HashMap::new();
    for (idx, (win, rect)) in tiled_windows.iter().zip(tiled_rects.into_iter()).enumerate() {
        tiled_rect_map.insert(win.id, (idx, rect));
    }

    let mut placements = Vec::with_capacity(visible.len());

    for (vis_idx, win) in visible.iter().enumerate() {
        let is_floating_effective = all_floating_mode || win.is_floating;

        let (frame_rect, z_order) = if win.is_fullscreen {
            let z = 300 + if win.is_focused { 50 } else { vis_idx as i32 };
            (usable_rect, z)
        } else if win.is_maximized {
            let z = 80 + if win.is_focused { 15 } else { vis_idx as i32 };
            (avail, z)
        } else if is_floating_effective {
            let rect = if win.rect.width > 1.0 && win.rect.height > 1.0 {
                win.rect
            } else {
                default_floating_rect(vis_idx, usable_rect)
            };
            let stack_pos = floating_z_stack
                .iter()
                .position(|&id| id == win.id)
                .unwrap_or(vis_idx);
            let z = 100 + stack_pos as i32 * 2 + if win.is_focused { 1 } else { 0 };
            (rect, z)
        } else if let Some(&(tiled_idx, rect)) = tiled_rect_map.get(&win.id) {
            let z = if layout_mode == LayoutMode::Monocle {
                if win.is_focused {
                    50
                } else {
                    10 + tiled_idx as i32
                }
            } else {
                10 + tiled_idx as i32 + if win.is_focused { 20 } else { 0 }
            };
            (rect, z)
        } else {
            (default_floating_rect(vis_idx, usable_rect), 10)
        };

        placements.push(build_window_placement(
            win,
            frame_rect,
            theme,
            is_floating_effective,
            z_order,
        ));
    }

    placements
}
