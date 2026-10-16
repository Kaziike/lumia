use crate::engine::LuminaWmCore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wm_proto::{
    Color, FrameBuffer, Point, Rect, StatePatch, SurfaceMetadata, WindowDescriptor, WindowId,
    WindowManagerEngine, WindowPlacement,
};

/// Wayland `zwlr_layer_shell_v1` layer tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerTier {
    Background,
    Bottom,
    Top,
    Overlay,
}

impl LayerTier {
    pub fn from_str_name(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "background" | "bg" => Self::Background,
            "bottom" => Self::Bottom,
            "overlay" => Self::Overlay,
            _ => Self::Top,
        }
    }
}

/// Wayland `zwlr_layer_surface_v1` anchor edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorEdges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl AnchorEdges {
    pub fn from_strings(anchors: &[String]) -> Self {
        let mut out = Self::default();
        for a in anchors {
            match a.trim().to_ascii_lowercase().as_str() {
                "top" => out.top = true,
                "bottom" => out.bottom = true,
                "left" => out.left = true,
                "right" => out.right = true,
                _ => {}
            }
        }
        out
    }
}

/// Wayland `zwlr_layer_surface_v1` keyboard interactivity mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardInteractivity {
    None,
    OnDemand,
    Exclusive,
}

impl KeyboardInteractivity {
    pub fn from_str_name(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "exclusive" => Self::Exclusive,
            "on_demand" | "on-demand" | "ondemand" => Self::OnDemand,
            _ => Self::None,
        }
    }
}

/// Registered HTML/CSS layer-shell surface hosted inside the compositor.
#[derive(Clone, Debug)]
pub struct HostedLayerSurface {
    pub metadata: SurfaceMetadata,
    pub tier: LayerTier,
    pub anchors: AnchorEdges,
    pub keyboard_mode: KeyboardInteractivity,
    pub visible: bool,
    pub computed_rect: Rect,
    pub buffer: Option<FrameBuffer>,
}

/// Pending `xdg_toplevel.configure` event queued for a Wayland client window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XdgToplevelConfigure {
    pub window_id: WindowId,
    pub width: u32,
    pub height: u32,
    pub is_activated: bool,
    pub is_fullscreen: bool,
    pub is_maximized: bool,
    pub serial: u32,
}

/// Embedded Wayland Compositor & Layer-Shell Host bridging `LuminaWmCore` with client surfaces.
pub struct WaylandCompositorHost {
    display_socket_name: String,
    next_serial: u32,
    layer_surfaces: BTreeMap<String, HostedLayerSurface>,
    client_buffers: BTreeMap<WindowId, FrameBuffer>,
    pending_configures: Vec<XdgToplevelConfigure>,
    frame_counter: u64,
}

impl Default for WaylandCompositorHost {
    fn default() -> Self {
        Self::new("wayland-lumina-0")
    }
}

impl WaylandCompositorHost {
    pub fn new(display_socket_name: impl Into<String>) -> Self {
        Self {
            display_socket_name: display_socket_name.into(),
            next_serial: 1,
            layer_surfaces: BTreeMap::new(),
            client_buffers: BTreeMap::new(),
            pending_configures: Vec::new(),
            frame_counter: 0,
        }
    }

    pub fn display_socket_name(&self) -> &str {
        &self.display_socket_name
    }

    pub fn frame_counter(&self) -> u64 {
        self.frame_counter
    }

    /// Registers or updates an HTML/CSS `wlr-layer-shell` surface and updates output exclusive zones.
    pub fn register_layer_surface(
        &mut self,
        wm: &mut LuminaWmCore,
        meta: SurfaceMetadata,
        buffer: Option<FrameBuffer>,
    ) {
        let output_rect = wm
            .outputs()
            .first()
            .map(|o| o.rect)
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 1920.0, 1080.0));

        let anchors = AnchorEdges::from_strings(&meta.anchor);
        let tier = LayerTier::from_str_name(&meta.layer);
        let keyboard_mode = KeyboardInteractivity::from_str_name(&meta.keyboard_mode);
        let computed_rect = Self::compute_layer_surface_rect(&meta, anchors, output_rect);

        wm.apply_surface_exclusive_zone("eDP-1", &meta);

        let name = meta.name.clone();
        self.layer_surfaces.insert(
            name,
            HostedLayerSurface {
                visible: meta.visible_by_default,
                metadata: meta,
                tier,
                anchors,
                keyboard_mode,
                computed_rect,
                buffer,
            },
        );
    }

    pub fn set_layer_surface_visibility(
        &mut self,
        wm: &mut LuminaWmCore,
        name: &str,
        visible: bool,
    ) -> bool {
        if let Some(surf) = self.layer_surfaces.get_mut(name) {
            surf.visible = visible;
            self.recalculate_exclusive_zones(wm);
            true
        } else {
            false
        }
    }

    pub fn update_layer_surface_buffer(&mut self, name: &str, buffer: FrameBuffer) {
        if let Some(surf) = self.layer_surfaces.get_mut(name) {
            surf.buffer = Some(buffer);
        }
    }

    pub fn layer_surfaces(&self) -> &BTreeMap<String, HostedLayerSurface> {
        &self.layer_surfaces
    }

    /// Maps a new `xdg_toplevel` client window into `LuminaWmCore` and emits `xdg_toplevel.configure`.
    pub fn map_xdg_toplevel(
        &mut self,
        wm: &mut LuminaWmCore,
        win: WindowDescriptor,
        buffer: Option<FrameBuffer>,
    ) -> Vec<StatePatch> {
        let win_id = win.id;
        if let Some(buf) = buffer {
            self.client_buffers.insert(win_id, buf);
        }
        let patches = wm.on_window_opened(win);
        self.queue_configures_from_wm(wm);
        patches
    }

    /// Unmaps an `xdg_toplevel` client window from `LuminaWmCore`.
    pub fn unmap_xdg_toplevel(
        &mut self,
        wm: &mut LuminaWmCore,
        win_id: WindowId,
    ) -> Vec<StatePatch> {
        self.client_buffers.remove(&win_id);
        let patches = wm.on_window_closed(win_id);
        self.queue_configures_from_wm(wm);
        patches
    }

    /// Drains pending `xdg_toplevel.configure` events generated after layout changes.
    pub fn drain_pending_configures(&mut self) -> Vec<XdgToplevelConfigure> {
        std::mem::take(&mut self.pending_configures)
    }

    /// Advances one compositor frame tick, synchronizing window placements with `xdg_toplevel` configures.
    pub fn tick(&mut self, wm: &mut LuminaWmCore) -> Vec<StatePatch> {
        self.frame_counter += 1;
        self.queue_configures_from_wm(wm);
        wm.export_state_patches()
    }

    /// Hit-tests pointer coordinates against overlay/top layer surfaces first, then client windows.
    pub fn hit_test_surface_or_window(
        &self,
        wm: &LuminaWmCore,
        point: Point,
    ) -> Option<HitTarget> {
        // 1. Check Overlay & Top layer surfaces
        for tier in [LayerTier::Overlay, LayerTier::Top] {
            for surf in self.layer_surfaces.values() {
                if surf.visible && surf.tier == tier && surf.computed_rect.contains(point) {
                    return Some(HitTarget::LayerSurface(surf.metadata.name.clone()));
                }
            }
        }

        // 2. Check client windows on active workspace in reverse z-order
        let mut placements = wm.compute_window_placements(wm.primary_usable_rect());
        placements.sort_by_key(|p| std::cmp::Reverse(p.z_order));
        for p in placements {
            if p.frame_rect.contains(point) {
                let in_titlebar = p
                    .titlebar_rect
                    .map(|tb| tb.contains(point))
                    .unwrap_or(false);
                return Some(HitTarget::ClientWindow {
                    window_id: p.window_id,
                    in_titlebar,
                });
            }
        }

        // 3. Check Bottom / Background layer surfaces
        for tier in [LayerTier::Bottom, LayerTier::Background] {
            for surf in self.layer_surfaces.values() {
                if surf.visible && surf.tier == tier && surf.computed_rect.contains(point) {
                    return Some(HitTarget::LayerSurface(surf.metadata.name.clone()));
                }
            }
        }

        None
    }

    /// Composites background, layer surfaces, and SSD-decorated client windows into an output `FrameBuffer`.
    pub fn composite_output_frame(&self, wm: &LuminaWmCore) -> FrameBuffer {
        let output_rect = wm
            .outputs()
            .first()
            .map(|o| o.rect)
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 1920.0, 1080.0));

        let out_w = output_rect.width.max(1.0) as u32;
        let out_h = output_rect.height.max(1.0) as u32;
        let mut fb = FrameBuffer::new(out_w, out_h);

        // 1. Fill dark desktop backdrop (#0f172a)
        fill_rect(
            &mut fb,
            Rect::new(0.0, 0.0, out_w as f32, out_h as f32),
            Color::rgb(15, 23, 42),
        );

        // 2. Paint Background & Bottom layer surfaces
        for tier in [LayerTier::Background, LayerTier::Bottom] {
            self.paint_layer_tier(&mut fb, tier);
        }

        // 3. Paint active workspace windows in ascending z_order
        let mut placements = wm.compute_window_placements(wm.primary_usable_rect());
        placements.sort_by_key(|p| p.z_order);
        for placement in &placements {
            self.paint_window_placement(&mut fb, placement);
        }

        // 4. Paint Top & Overlay layer surfaces
        for tier in [LayerTier::Top, LayerTier::Overlay] {
            self.paint_layer_tier(&mut fb, tier);
        }

        fb
    }

    fn recalculate_exclusive_zones(&self, wm: &mut LuminaWmCore) {
        let Some(mut out) = wm.outputs().first().cloned() else {
            return;
        };
        let mut usable = out.rect;
        for surf in self.layer_surfaces.values() {
            if !surf.visible || surf.metadata.exclusive_zone <= 0 {
                continue;
            }
            let zone = surf.metadata.exclusive_zone as f32;
            if surf.anchors.top && !surf.anchors.bottom {
                usable.y += zone;
                usable.height = (usable.height - zone).max(100.0);
            } else if surf.anchors.bottom && !surf.anchors.top {
                usable.height = (usable.height - zone).max(100.0);
            } else if surf.anchors.left && !surf.anchors.right {
                usable.x += zone;
                usable.width = (usable.width - zone).max(100.0);
            } else if surf.anchors.right && !surf.anchors.left {
                usable.width = (usable.width - zone).max(100.0);
            }
        }
        out.usable_rect = usable;
        wm.add_output(out);
    }

    fn queue_configures_from_wm(&mut self, wm: &LuminaWmCore) {
        let placements = wm.compute_window_placements(wm.primary_usable_rect());
        let windows = wm.windows();
        for p in placements {
            let is_maximized = windows
                .iter()
                .find(|w| w.id == p.window_id)
                .map(|w| w.is_maximized)
                .unwrap_or(false);
            let serial = self.next_serial;
            self.next_serial = self.next_serial.wrapping_add(1);
            self.pending_configures.push(XdgToplevelConfigure {
                window_id: p.window_id,
                width: p.content_rect.width.max(1.0) as u32,
                height: p.content_rect.height.max(1.0) as u32,
                is_activated: p.is_focused,
                is_fullscreen: p.is_fullscreen,
                is_maximized,
                serial,
            });
        }
    }

    fn compute_layer_surface_rect(
        meta: &SurfaceMetadata,
        anchors: AnchorEdges,
        output_rect: Rect,
    ) -> Rect {
        let w = if anchors.left && anchors.right {
            output_rect.width
        } else {
            meta.width.unwrap_or(400) as f32
        };
        let h = if anchors.top && anchors.bottom {
            output_rect.height
        } else {
            meta.height.unwrap_or(44) as f32
        };

        let x = if anchors.left && !anchors.right {
            output_rect.x
        } else if anchors.right && !anchors.left {
            output_rect.right() - w
        } else {
            output_rect.x + (output_rect.width - w) * 0.5
        };

        let y = if anchors.top && !anchors.bottom {
            output_rect.y
        } else if anchors.bottom && !anchors.top {
            output_rect.bottom() - h
        } else {
            output_rect.y + (output_rect.height - h) * 0.5
        };

        Rect::new(x, y, w, h)
    }

    fn paint_layer_tier(&self, fb: &mut FrameBuffer, tier: LayerTier) {
        for surf in self.layer_surfaces.values() {
            if !surf.visible || surf.tier != tier {
                continue;
            }
            if let Some(buf) = &surf.buffer {
                blit_buffer(fb, buf, surf.computed_rect.x as i32, surf.computed_rect.y as i32);
            } else {
                fill_rect(fb, surf.computed_rect, Color::rgba(30, 41, 59, 230));
            }
        }
    }

    fn paint_window_placement(&self, fb: &mut FrameBuffer, p: &WindowPlacement) {
        let border_col = if p.is_focused {
            Color::rgb(56, 189, 248) // Active sky-blue border
        } else {
            Color::rgb(51, 65, 85) // Inactive slate border
        };
        fill_rect(fb, p.frame_rect, border_col);

        if let Some(tb) = p.titlebar_rect {
            let tb_col = if p.is_focused {
                Color::rgb(30, 41, 59)
            } else {
                Color::rgb(15, 23, 42)
            };
            fill_rect(fb, tb, tb_col);
        }

        if let Some(client_buf) = self.client_buffers.get(&p.window_id) {
            blit_buffer(
                fb,
                client_buf,
                p.content_rect.x as i32,
                p.content_rect.y as i32,
            );
        } else {
            fill_rect(fb, p.content_rect, Color::rgb(24, 34, 52));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitTarget {
    LayerSurface(String),
    ClientWindow {
        window_id: WindowId,
        in_titlebar: bool,
    },
}

fn fill_rect(fb: &mut FrameBuffer, rect: Rect, color: Color) {
    let x0 = (rect.x.max(0.0) as u32).min(fb.width);
    let y0 = (rect.y.max(0.0) as u32).min(fb.height);
    let x1 = ((rect.x + rect.width).max(0.0) as u32).min(fb.width);
    let y1 = ((rect.y + rect.height).max(0.0) as u32).min(fb.height);

    for y in y0..y1 {
        let row_offset = (y * fb.width) as usize * 4;
        for x in x0..x1 {
            let idx = row_offset + (x as usize) * 4;
            fb.pixels[idx] = color.r;
            fb.pixels[idx + 1] = color.g;
            fb.pixels[idx + 2] = color.b;
            fb.pixels[idx + 3] = color.a;
        }
    }
}

fn blit_buffer(dst: &mut FrameBuffer, src: &FrameBuffer, dst_x: i32, dst_y: i32) {
    for sy in 0..src.height {
        let dy = dst_y + sy as i32;
        if dy < 0 || dy >= dst.height as i32 {
            continue;
        }
        for sx in 0..src.width {
            let dx = dst_x + sx as i32;
            if dx < 0 || dx >= dst.width as i32 {
                continue;
            }
            let s_idx = ((sy * src.width + sx) * 4) as usize;
            let d_idx = ((dy as u32 * dst.width + dx as u32) * 4) as usize;
            let sa = src.pixels[s_idx + 3];
            if sa > 0 {
                dst.pixels[d_idx..d_idx + 4].copy_from_slice(&src.pixels[s_idx..s_idx + 4]);
            }
        }
    }
}
