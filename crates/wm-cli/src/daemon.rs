use crate::wayland_compositor::{RealWaylandServer, WaylandCompositorEvent};
use css_style::CssStyleEngine;
use html_dom::{DomDocument, DomEventKind};
use ipc_server::{AssetWatcher, IpcError, IpcServer, IpcServerHandle};
use layout_engine::TaffyLayoutEngine;
use reactive_state::StateStore;
use renderer::SkiaRenderer;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use sys_providers::SystemProviderManager;
use wm_core::{LuminaWmCore, WaylandCompositorHost};
use wm_proto::{
    ActionCommand, ComputedStyle, DomElementInfo, DomTreeView, FrameBuffer, IpcCommand,
    IpcPayload, IpcRequest, IpcResponse, LayoutBox, LayoutSolver, NodeId, OutputDescriptor, Point,
    Rect, ReloadTarget, ShellAction, Size, StatePatch, StateProvider, StateValue, StyleEngine,
    SurfaceMetadata, SurfaceRenderer, SysAction, WidgetStatus, WindowDescriptor,
    WindowManagerEngine, WmAction,
};

const DEFAULT_TOPBAR_HTML: &str = include_str!("../../../assets/topbar.html");
const DEFAULT_DOCK_HTML: &str = include_str!("../../../assets/dock.html");
const DEFAULT_LAUNCHER_HTML: &str = include_str!("../../../assets/launcher.html");
const DEFAULT_QUICK_SETTINGS_HTML: &str = include_str!("../../../assets/quick_settings.html");
const DEFAULT_WINDOW_DEC_HTML: &str = include_str!("../../../assets/window_decoration.html");
const DEFAULT_CATPPUCCIN_CSS: &str = include_str!("../../../assets/css/theme-catppuccin.css");
const DEFAULT_CYBERPUNK_CSS: &str = include_str!("../../../assets/css/theme-cyberpunk.css");

#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowCacheKey {
    width: u32,
    height: u32,
    is_focused: bool,
    is_floating: bool,
    is_fullscreen: bool,
    is_urgent: bool,
    title: String,
    client_serial: u64,
    terminal_rev: u64,
}

/// Live HTML+CSS surface instance managed by `LuminaDaemon`.
pub struct LiveSurface {
    pub name: String,
    pub html_source: String,
    pub dom: DomDocument,
    pub styles: HashMap<NodeId, ComputedStyle>,
    pub layouts: HashMap<NodeId, LayoutBox>,
    pub viewport: Size,
    pub visible: bool,
    pub layer: String,
    pub dirty: bool,
    pub last_frame: Option<FrameBuffer>,
}

/// Orchestrates `LuminaWmCore`, `WaylandCompositorHost`, `StateStore`, `SystemProviderManager`,
/// `DomDocument` surfaces, `CssStyleEngine`, `TaffyLayoutEngine`, and `SkiaRenderer`.
pub struct LuminaDaemon {
    pub wm: LuminaWmCore,
    pub compositor: WaylandCompositorHost,
    pub wayland_server: Option<RealWaylandServer>,
    pub store: StateStore,
    pub sys_providers: SystemProviderManager,
    pub style_engine: CssStyleEngine,
    pub layout_engine: TaffyLayoutEngine,
    pub renderer: SkiaRenderer,
    pub surfaces: BTreeMap<String, LiveSurface>,
    pub widgets: BTreeMap<String, WidgetStatus>,
    pub css_variables: BTreeMap<String, String>,
    pub custom_css_sheets: Vec<String>,
    pub current_theme: String,
    pub assets_dir: Option<PathBuf>,
    pub reload_count: u64,
    pub terminal_lines: Vec<String>,
    pub terminal_input: String,
    pub terminal_rev: u64,
    pub output_width: u32,
    pub output_height: u32,
    cached_wallpaper: Option<(String, FrameBuffer)>,
    window_frame_cache: HashMap<u64, (WindowCacheKey, FrameBuffer)>,
}

impl Default for LuminaDaemon {
    fn default() -> Self {
        Self::new()
    }
}

impl LuminaDaemon {
    pub fn new() -> Self {
        let mut wm = LuminaWmCore::new();
        let mut compositor = WaylandCompositorHost::default();
        let mut store = StateStore::new();
        let mut sys_providers = SystemProviderManager::new();
        let mut style_engine = CssStyleEngine::new();
        let layout_engine = TaffyLayoutEngine::default();
        let renderer = SkiaRenderer::new();

        // Load default Catppuccin theme
        let _ = style_engine.load_stylesheet(DEFAULT_CATPPUCCIN_CSS);

        // Register default HTML/CSS layer-shell topbar surface
        compositor.register_layer_surface(&mut wm, SurfaceMetadata::default(), None);

        // Seed default windows for headless/CLI testing (cleared in interactive nested mode via `clear_startup_windows`)
        wm.on_window_opened(WindowDescriptor {
            id: 101,
            title: "Lumina Terminal — zsh (cargo test --workspace)".to_string(),
            app_id: "org.lumina.terminal".to_string(),
            icon: "terminal".to_string(),
            workspace_id: 1,
            is_focused: true,
            is_floating: false,
            is_fullscreen: false,
            is_maximized: false,
            is_minimized: false,
            is_urgent: false,
            rect: Rect::default(),
        });
        wm.on_window_opened(WindowDescriptor {
            id: 102,
            title: "Mozilla Firefox — Lumina Wayland HTML+CSS Architecture".to_string(),
            app_id: "org.mozilla.firefox".to_string(),
            icon: "firefox".to_string(),
            workspace_id: 1,
            is_focused: false,
            is_floating: false,
            is_fullscreen: false,
            is_maximized: false,
            is_minimized: false,
            is_urgent: false,
            rect: Rect::default(),
        });

        // Initialize system providers into StateStore
        sys_providers.init_store(&mut store);
        store.set(
            "sys.display.resolution",
            StateValue::String("1920x1080".to_string()),
        );
        sys_providers.notif.push_notification(
            "Lumina WM",
            "Pure-Rust HTML/CSS Compositor Ready",
            "Rendered with Taffy + Tiny-Skia + Smithay (0% WebKit/Electron).",
            "normal",
        );
        let notif_patches = sys_providers.notif.poll_updates();
        store.apply_patches(notif_patches);

        let mut css_variables = BTreeMap::new();
        css_variables.insert("--bg-surface".to_string(), "rgba(30, 30, 46, 0.92)".to_string());
        css_variables.insert("--accent".to_string(), "#89b4fa".to_string());
        css_variables.insert("--text-main".to_string(), "#cdd6f4".to_string());
        css_variables.insert("--wm-gap-inner".to_string(), "8px".to_string());
        css_variables.insert("--wm-gap-outer".to_string(), "12px".to_string());
        css_variables.insert("--wm-border-radius".to_string(), "10px".to_string());

        let mut daemon = Self {
            wm,
            compositor,
            wayland_server: None,
            store,
            sys_providers,
            style_engine,
            layout_engine,
            renderer,
            surfaces: BTreeMap::new(),
            widgets: BTreeMap::new(),
            css_variables,
            custom_css_sheets: Vec::new(),
            current_theme: "catppuccin".to_string(),
            assets_dir: None,
            reload_count: 0,
            terminal_lines: Vec::new(),
            terminal_input: String::new(),
            terminal_rev: 1,
            output_width: 1920,
            output_height: 1080,
            cached_wallpaper: None,
            window_frame_cache: HashMap::new(),
        };

        // Sync initial WM state into store
        let wm_patches = daemon.wm.export_state_patches();
        daemon.apply_state_patches(wm_patches);

        // Load built-in declarative HTML surfaces
        daemon.load_builtin_surfaces();
        daemon.recompute_all_surfaces();

        daemon
    }

    /// Removes all pre-seeded windows so an interactive session starts with a clean desktop.
    pub fn clear_startup_windows(&mut self) {
        self.window_frame_cache.clear();
        let patches = self.wm.clear_all_windows();
        self.apply_state_patches(patches);
    }

    /// Starts the embedded `RealWaylandServer` so real Wayland client applications can connect
    /// and render their `wl_shm` surfaces inside Lumina WM.
    pub fn enable_wayland_server(&mut self) -> Result<String, String> {
        let mut server = RealWaylandServer::new()?;
        server.set_output_mode(self.output_width, self.output_height, 1.0);
        let sock = server.socket_name().to_string();
        self.wayland_server = Some(server);
        Ok(sock)
    }

    /// Dynamically resizes the active display output and updates all shell surfaces,
    /// widgets, WM usable area, and embedded Wayland output geometry 1:1.
    pub fn resize_output(&mut self, width: u32, height: u32, scale: f32) {
        let w = width.max(640);
        let h = height.max(480);
        if self.output_width == w && self.output_height == h {
            return;
        }
        self.output_width = w;
        self.output_height = h;
        self.cached_wallpaper = None;
        self.window_frame_cache.clear();

        self.wm.add_output(OutputDescriptor {
            name: "eDP-1".to_string(),
            make: "Lumina".to_string(),
            model: "Virtual Wayland Display".to_string(),
            rect: Rect::new(0.0, 0.0, w as f32, h as f32),
            usable_rect: Rect::new(0.0, 44.0, w as f32, (h as f32 - 44.0).max(100.0)),
            scale: scale.max(1.0),
            refresh_hz: 60,
        });

        for name in ["panel", "topbar"] {
            if let Some(surf) = self.surfaces.get_mut(name) {
                surf.viewport = Size::new(w as f32, 44.0);
                surf.dirty = true;
                surf.last_frame = None;
            }
            if let Some(widget) = self.widgets.get_mut(name) {
                widget.width = w;
                widget.height = 44;
            }
        }

        if let Some(widget) = self.widgets.get_mut("dock") {
            widget.width = 680;
            widget.height = 58;
        }
        for name in ["quick_settings", "control-center"] {
            if let Some(widget) = self.widgets.get_mut(name) {
                widget.width = 380;
                widget.height = 520;
            }
        }
        if let Some(widget) = self.widgets.get_mut("launcher") {
            widget.width = 640;
            widget.height = 440;
        }

        if let Some(ref mut wl) = self.wayland_server {
            wl.set_output_mode(w, h, scale);
        }

        self.store.set(
            "sys.display.resolution",
            StateValue::String(format!("{w}x{h}")),
        );

        let patches = self.wm.export_state_patches();
        self.apply_state_patches(patches);
        self.recompute_all_surfaces();
    }

    /// Polls connected Wayland client applications, handling new windows, title changes, frame commits, and closes.
    pub fn poll_wayland_clients(&mut self) -> bool {
        let Some(ref mut wl) = self.wayland_server else {
            return false;
        };
        let events = wl.poll();
        if events.is_empty() {
            return false;
        }
        let mut patches = Vec::new();
        for ev in events {
            match ev {
                WaylandCompositorEvent::WindowOpened {
                    window_id,
                    title,
                    app_id,
                } => {
                    patches.extend(self.wm.on_window_opened(WindowDescriptor {
                        id: window_id,
                        title,
                        app_id: app_id.clone(),
                        icon: app_id,
                        workspace_id: self.wm.active_workspace_id(),
                        is_focused: true,
                        is_floating: false,
                        is_fullscreen: false,
                        is_maximized: false,
                        is_minimized: false,
                        is_urgent: false,
                        rect: Rect::default(),
                    }));
                }
                WaylandCompositorEvent::WindowUpdated {
                    window_id,
                    title,
                    app_id,
                } => {
                    patches.extend(self.wm.update_window_metadata(window_id, title, app_id));
                }
                WaylandCompositorEvent::WindowClosed { window_id } => {
                    self.window_frame_cache.remove(&window_id);
                    patches.extend(self.wm.on_window_closed(window_id));
                }
                WaylandCompositorEvent::WindowFrameCommitted { .. } => {}
            }
        }
        if !patches.is_empty() {
            self.apply_state_patches(patches);
        }
        true
    }

    fn load_builtin_surfaces(&mut self) {
        let definitions = [
            ("panel", DEFAULT_TOPBAR_HTML, Size::new(1920.0, 44.0), true, "top"),
            ("topbar", DEFAULT_TOPBAR_HTML, Size::new(1920.0, 44.0), true, "top"),
            ("dock", DEFAULT_DOCK_HTML, Size::new(680.0, 58.0), true, "top"),
            ("launcher", DEFAULT_LAUNCHER_HTML, Size::new(640.0, 440.0), false, "overlay"),
            (
                "quick_settings",
                DEFAULT_QUICK_SETTINGS_HTML,
                Size::new(380.0, 520.0),
                false,
                "overlay",
            ),
            (
                "control-center",
                DEFAULT_QUICK_SETTINGS_HTML,
                Size::new(380.0, 520.0),
                false,
                "overlay",
            ),
            (
                "window_decoration",
                DEFAULT_WINDOW_DEC_HTML,
                Size::new(800.0, 32.0),
                true,
                "top",
            ),
        ];

        for (name, html, viewport, visible, layer) in definitions {
            let mut dom = DomDocument::parse(html);
            dom.sync_reactive_state(&self.store);
            let node_count = dom.all_node_ids().len();
            self.surfaces.insert(
                name.to_string(),
                LiveSurface {
                    name: name.to_string(),
                    html_source: html.to_string(),
                    dom,
                    styles: HashMap::new(),
                    layouts: HashMap::new(),
                    viewport,
                    visible,
                    layer: layer.to_string(),
                    dirty: true,
                    last_frame: None,
                },
            );
            self.widgets.insert(
                name.to_string(),
                WidgetStatus {
                    name: name.to_string(),
                    layer: layer.to_string(),
                    visible,
                    width: viewport.width as u32,
                    height: viewport.height as u32,
                    node_count,
                },
            );
        }

        // Also register "overview" and "notifications" widget entries
        for (name, layer, visible, w, h) in [
            ("overview", "overlay", false, 1600, 900),
            ("notifications", "top", true, 360, 280),
            ("osd", "overlay", false, 280, 72),
        ] {
            self.widgets.entry(name.to_string()).or_insert(WidgetStatus {
                name: name.to_string(),
                layer: layer.to_string(),
                visible,
                width: w,
                height: h,
                node_count: 8,
            });
        }
    }

    /// Marks all HTML surfaces dirty and recomputes reactive DOM, CSS cascade, and Taffy layout.
    pub fn recompute_all_surfaces(&mut self) {
        for (name, surf) in &mut self.surfaces {
            surf.dom.sync_reactive_state(&self.store);
            surf.styles = self.style_engine.compute_styles(&surf.dom);
            surf.layouts = self
                .layout_engine
                .compute_layout(&surf.dom, &surf.styles, surf.viewport);
            surf.dirty = true;
            if let Some(w) = self.widgets.get_mut(name) {
                w.node_count = surf.dom.all_node_ids().len();
                w.visible = surf.visible;
            }
        }
    }

    /// Renders a specific HTML/CSS surface into an RGBA8 `FrameBuffer`, utilizing surface frame caching
    /// when `surf.dirty == false` for sub-millisecond compositing.
    pub fn render_surface(&mut self, name: &str) -> Option<FrameBuffer> {
        let surf = self.surfaces.get_mut(name)?;
        if !surf.dirty {
            if let Some(ref cached) = surf.last_frame {
                return Some(cached.clone());
            }
        }
        surf.dom.sync_reactive_state(&self.store);
        surf.styles = self.style_engine.compute_styles(&surf.dom);
        surf.layouts =
            self.layout_engine
                .compute_layout(&surf.dom, &surf.styles, surf.viewport);
        let fb = self
            .renderer
            .render_surface(&surf.dom, &surf.styles, &surf.layouts, surf.viewport);
        surf.last_frame = Some(fb.clone());
        surf.dirty = false;
        Some(fb)
    }

    /// Updates `:hover` state on a named HTML surface and returns `true` if the hovered element changed.
    pub fn update_hover(&mut self, surface_name: &str, x: f32, y: f32) -> bool {
        let Some(surf) = self.surfaces.get_mut(surface_name) else {
            return false;
        };
        let hit = if x >= 0.0
            && y >= 0.0
            && x <= surf.viewport.width
            && y <= surf.viewport.height
        {
            self.layout_engine
                .hit_test(&surf.dom, &surf.layouts, Point::new(x, y))
        } else {
            None
        };
        let changed = surf.dom.update_hover_chain(hit);
        if changed {
            surf.styles = self.style_engine.compute_styles(&surf.dom);
            surf.layouts =
                self.layout_engine
                    .compute_layout(&surf.dom, &surf.styles, surf.viewport);
            surf.dirty = true;
        }
        changed
    }

    /// Dispatches a pointer click at `(x, y)` on a named HTML surface, performing
    /// Taffy hit-testing, DOM event bubbling, and executing any triggered `ActionCommand`s!
    pub fn click_surface(&mut self, surface_name: &str, x: f32, y: f32) -> Result<Vec<String>, String> {
        self.dispatch_surface_event(surface_name, x, y, DomEventKind::Click)
    }

    /// Dispatches a scroll event (`ScrollUp` or `ScrollDown`) at `(x, y)` on a named HTML surface.
    pub fn scroll_surface(
        &mut self,
        surface_name: &str,
        x: f32,
        y: f32,
        scroll_up: bool,
    ) -> Result<Vec<String>, String> {
        let kind = if scroll_up {
            DomEventKind::ScrollUp
        } else {
            DomEventKind::ScrollDown
        };
        self.dispatch_surface_event(surface_name, x, y, kind)
    }

    fn dispatch_surface_event(
        &mut self,
        surface_name: &str,
        x: f32,
        y: f32,
        event_kind: DomEventKind,
    ) -> Result<Vec<String>, String> {
        let actions = {
            let surf = self
                .surfaces
                .get(surface_name)
                .ok_or_else(|| format!("Surface '{surface_name}' not found"))?;
            let Some(node_id) =
                self.layout_engine
                    .hit_test(&surf.dom, &surf.layouts, Point::new(x, y))
            else {
                return Ok(Vec::new());
            };
            surf.dom
                .dispatch_dom_event(node_id, event_kind, &self.store)
        };
        let mut results = Vec::new();
        for cmd in actions {
            results.push(self.dispatch_action(cmd)?);
        }
        Ok(results)
    }

    /// Appends typed text to the interactive Terminal window buffer.
    pub fn terminal_push_str(&mut self, s: &str) {
        let mut changed = false;
        for ch in s.chars() {
            if !ch.is_control() {
                self.terminal_input.push(ch);
                changed = true;
            }
        }
        if changed {
            self.terminal_rev = self.terminal_rev.wrapping_add(1);
        }
    }

    /// Deletes the last character in the interactive Terminal window buffer.
    pub fn terminal_backspace(&mut self) {
        if self.terminal_input.pop().is_some() {
            self.terminal_rev = self.terminal_rev.wrapping_add(1);
        }
    }

    /// Executes the current `terminal_input` command inside the focused Terminal window.
    pub fn terminal_execute(&mut self) {
        self.terminal_rev = self.terminal_rev.wrapping_add(1);
        let cmd = self.terminal_input.trim().to_string();
        self.terminal_input.clear();
        self.terminal_lines.push(format!("kaz@lumina-wm ➜ ~ $ {cmd}"));
        if cmd.is_empty() {
            return;
        }
        if cmd == "clear" {
            self.terminal_lines.clear();
            return;
        }
        if cmd == "help" {
            self.terminal_lines.push(
                "Built-in commands: clear, help, status, workspaces, windows, or any shell command (ls, uname -a, date)".to_string(),
            );
            return;
        }
        // Run non-blocking shell command with short output limit
        if let Ok(output) = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stdout.lines().chain(stderr.lines()).take(10) {
                self.terminal_lines.push(line.to_string());
            }
        } else {
            self.terminal_lines
                .push(format!("zsh: command failed: {cmd}"));
        }
        while self.terminal_lines.len() > 14 {
            self.terminal_lines.remove(0);
        }
    }

    fn get_or_build_wallpaper(&mut self, width: u32, height: u32) -> FrameBuffer {
        if let Some((ref theme, ref fb)) = self.cached_wallpaper {
            if theme == &self.current_theme && fb.width == width && fb.height == height {
                return fb.clone();
            }
        }
        let mut desktop = FrameBuffer::new(width, height);
        let is_cyber = self.current_theme.contains("cyber");
        for y in 0..height {
            let t = y as f32 / height as f32;
            let (r, g, b) = if is_cyber {
                (
                    (10.0 + t * 18.0) as u8,
                    (12.0 + t * 10.0) as u8,
                    (28.0 + t * 36.0) as u8,
                )
            } else {
                (
                    (17.0 + t * 13.0) as u8,
                    (17.0 + t * 13.0) as u8,
                    (27.0 + t * 22.0) as u8,
                )
            };
            let row_start = (y * width * 4) as usize;
            for x in 0..width as usize {
                let idx = row_start + x * 4;
                desktop.pixels[idx] = r;
                desktop.pixels[idx + 1] = g;
                desktop.pixels[idx + 2] = b;
                desktop.pixels[idx + 3] = 255;
            }
        }
        self.cached_wallpaper = Some((self.current_theme.clone(), desktop.clone()));
        desktop
    }

    /// Composites a complete desktop screenshot at native output resolution (Wallpaper + Tiled/Floating Windows
    /// with CSS Server-Side Decorations & Application UIs + Topbar + Dock + Active Overlays).
    pub fn render_desktop_frame(&mut self) -> FrameBuffer {
        let width = self.output_width.max(640);
        let height = self.output_height.max(480);

        // 1. Copy cached desktop background gradient
        let mut desktop = self.get_or_build_wallpaper(width, height);

        // 2. Render tiled/floating windows on active workspace with Server-Side Decorations + App Client UI
        let usable = self
            .wm
            .outputs()
            .first()
            .map(|o| o.usable_rect)
            .unwrap_or(Rect::new(
                0.0,
                44.0,
                width as f32,
                (height as f32 - 44.0).max(100.0),
            ));
        let placements = self.wm.compute_window_placements(usable);
        let windows_map: HashMap<u64, WindowDescriptor> = self
            .wm
            .windows()
            .into_iter()
            .map(|w| (w.id, w))
            .collect();

        for placement in &placements {
            let win_desc = windows_map.get(&placement.window_id);
            let title = win_desc
                .map(|w| w.title.as_str())
                .unwrap_or("Wayland Window");
            let app_id = win_desc
                .map(|w| w.app_id.as_str())
                .unwrap_or("org.lumina.app");
            let is_urgent = win_desc.map(|w| w.is_urgent).unwrap_or(false);

            let content_w = placement.content_rect.width.round().max(100.0) as u32;
            let content_h = placement.content_rect.height.round().max(100.0) as u32;
            if let Some(ref mut wl) = self.wayland_server {
                wl.configure_window(
                    placement.window_id,
                    content_w,
                    content_h,
                    placement.is_focused,
                );
            }

            let client_serial = self
                .wayland_server
                .as_ref()
                .and_then(|wl| wl.window_frame(placement.window_id))
                .map(|(_, serial)| serial)
                .unwrap_or(0);

            let cache_key = WindowCacheKey {
                width: placement.frame_rect.width.round().max(1.0) as u32,
                height: placement.frame_rect.height.round().max(1.0) as u32,
                is_focused: placement.is_focused,
                is_floating: placement.is_floating,
                is_fullscreen: placement.is_fullscreen,
                is_urgent,
                title: title.to_string(),
                client_serial,
                terminal_rev: if app_id.contains("term") {
                    self.terminal_rev
                } else {
                    0
                },
            };

            let cache_hit = self
                .window_frame_cache
                .get(&placement.window_id)
                .map(|(k, _)| k == &cache_key)
                .unwrap_or(false);

            if !cache_hit {
                let client_fb = self
                    .wayland_server
                    .as_ref()
                    .and_then(|wl| wl.window_frame(placement.window_id))
                    .map(|(fb, _)| fb);

                let win_fb = renderer::render_window_with_client_buffer(
                    self.renderer.font_manager(),
                    placement,
                    app_id,
                    title,
                    is_urgent,
                    client_fb.as_deref(),
                    &self.terminal_lines,
                    &self.terminal_input,
                    None,
                );
                self.window_frame_cache
                    .insert(placement.window_id, (cache_key, win_fb));
            }

            if let Some((_, ref win_fb)) = self.window_frame_cache.get(&placement.window_id) {
                blit_framebuffer(
                    &mut desktop,
                    win_fb,
                    placement.frame_rect.x.round() as i32,
                    placement.frame_rect.y.round() as i32,
                );
            }
        }

        // 3. Render Topbar (`topbar` at (0, 0))
        if let Some(topbar_fb) = self.render_surface("topbar") {
            blit_framebuffer(&mut desktop, &topbar_fb, 0, 0);
        }

        // 4. Render Bottom Dock (`dock` centered at bottom)
        let dock_visible = self
            .surfaces
            .get("dock")
            .map(|s| s.visible)
            .unwrap_or(true);
        if dock_visible {
            if let Some(dock_fb) = self.render_surface("dock") {
                let dx = ((width as i32) - (dock_fb.width as i32)) / 2;
                let dy = (height as i32) - (dock_fb.height as i32) - 10;
                blit_framebuffer(&mut desktop, &dock_fb, dx, dy);
            }
        }

        // 5. Render Quick Settings / Control Center if visible (top-right overlay)
        let qs_visible = self
            .widgets
            .get("quick_settings")
            .map(|w| w.visible)
            .unwrap_or(false)
            || self
                .widgets
                .get("control-center")
                .map(|w| w.visible)
                .unwrap_or(false);
        if qs_visible {
            if let Some(qs_fb) = self.render_surface("quick_settings") {
                let qx = (width as i32) - (qs_fb.width as i32) - 16;
                let qy = 54;
                blit_framebuffer(&mut desktop, &qs_fb, qx, qy);
            }
        }

        // 6. Render App Launcher modal if visible (centered overlay)
        let launcher_visible = self
            .widgets
            .get("launcher")
            .map(|w| w.visible)
            .unwrap_or(false);
        if launcher_visible {
            if let Some(launcher_fb) = self.render_surface("launcher") {
                let lx = ((width as i32) - (launcher_fb.width as i32)) / 2;
                let ly = ((height as i32) - (launcher_fb.height as i32)) / 2;
                blit_framebuffer(&mut desktop, &launcher_fb, lx, ly);
            }
        }

        desktop
    }

    pub fn apply_state_patches(&mut self, patches: Vec<StatePatch>) {
        if patches.is_empty() {
            return;
        }
        self.store.apply_patches(patches);
        // Keep convenient aliases synced
        if let Some(vol) = self.store.get("sys.audio.percent") {
            self.store.set("audio.volume_pct", vol);
        }
        if let Some(muted) = self.store.get("sys.audio.muted") {
            self.store.set("audio.muted", muted);
        }
        self.recompute_all_surfaces();
    }

    pub fn set_state_path(&mut self, path: &str, value: StateValue) {
        self.store.set(path, value.clone());
        if path == "audio.volume_pct" {
            self.store.set("sys.audio.percent", value);
        } else if path == "sys.audio.percent" {
            self.store.set("audio.volume_pct", value);
        } else if path == "audio.muted" {
            self.store.set("sys.audio.muted", value);
        } else if path == "sys.audio.muted" {
            self.store.set("audio.muted", value);
        }
        self.recompute_all_surfaces();
    }

    pub fn get_state_path(&self, path: Option<&str>) -> StateValue {
        match path {
            None | Some("") => self.store.snapshot(),
            Some(p) => self.store.get(p).unwrap_or(StateValue::Null),
        }
    }

    pub fn set_theme(&mut self, theme_name: &str) -> Result<(), String> {
        let lower = theme_name.to_ascii_lowercase();
        let css = if lower.contains("cyber") {
            DEFAULT_CYBERPUNK_CSS
        } else {
            DEFAULT_CATPPUCCIN_CSS
        };
        self.style_engine.load_stylesheet(css)?;
        for sheet in &self.custom_css_sheets {
            let _ = self.style_engine.append_stylesheet(sheet);
        }
        for (k, v) in &self.css_variables {
            self.style_engine.set_custom_property(k, v);
        }
        self.current_theme = theme_name.to_string();
        self.cached_wallpaper = None;
        self.window_frame_cache.clear();
        self.store
            .set("shell.theme", StateValue::String(theme_name.to_string()));
        self.recompute_all_surfaces();
        Ok(())
    }

    fn set_widget_visibility(&mut self, name: &str, visible: Option<bool>) -> Result<bool, String> {
        let aliases: Vec<&str> = match name {
            "panel" | "topbar" => vec!["panel", "topbar"],
            "control-center" | "quick_settings" => vec!["control-center", "quick_settings"],
            other => vec![other],
        };
        let mut found = false;
        let mut final_vis = false;
        for &alias in &aliases {
            if let Some(w) = self.widgets.get_mut(alias) {
                let next = visible.unwrap_or(!w.visible);
                w.visible = next;
                final_vis = next;
                found = true;
            }
            if let Some(s) = self.surfaces.get_mut(alias) {
                s.visible = final_vis;
                s.dirty = true;
            }
        }
        if found {
            Ok(final_vis)
        } else {
            Err(format!("Widget '{name}' not found"))
        }
    }

    pub fn dispatch_action(&mut self, action: ActionCommand) -> Result<String, String> {
        match action {
            ActionCommand::Wm(wm_act) => {
                if let WmAction::CloseWindow { id } = &wm_act {
                    if let Some(target_id) = id.or_else(|| self.wm.focused_window_id()) {
                        self.window_frame_cache.remove(&target_id);
                        if let Some(ref mut wl) = self.wayland_server {
                            wl.close_window(target_id);
                        }
                    }
                }
                let patches = self.wm.dispatch_wm_action(&wm_act)?;
                if let Some(w) = self.widgets.get_mut("overview") {
                    w.visible = self.wm.overview_open();
                }
                self.apply_state_patches(patches);
                Ok(format!("Dispatched WM action: {wm_act:?}"))
            }
            ActionCommand::Shell(shell_act) => match shell_act {
                ShellAction::OpenWidget { name, .. } => {
                    self.set_widget_visibility(&name, Some(true))?;
                    Ok(format!("Opened widget '{name}'"))
                }
                ShellAction::CloseWidget { name } => {
                    self.set_widget_visibility(&name, Some(false))?;
                    Ok(format!("Closed widget '{name}'"))
                }
                ShellAction::ToggleWidget { name, .. } => {
                    let vis = self.set_widget_visibility(&name, None)?;
                    Ok(format!("Toggled widget '{name}' (visible={vis})"))
                }
                ShellAction::ReloadCss { surface } => {
                    self.reload_from_disk(ReloadTarget::Css);
                    Ok(format!("Reloaded CSS (surface={surface:?})"))
                }
                ShellAction::ReloadHtml { surface } => {
                    self.reload_from_disk(ReloadTarget::Html);
                    Ok(format!("Reloaded HTML (surface={surface:?})"))
                }
                ShellAction::SetTheme { name } => {
                    self.set_theme(&name)?;
                    Ok(format!("Set theme to '{name}'"))
                }
            },
            ActionCommand::Sys(SysAction::SetResolution { width, height }) => {
                let _ = crate::output_config::request_parent_wayland_mode(
                    width as i32,
                    height as i32,
                );
                self.resize_output(width, height, 1.0);
                Ok(format!("Set display resolution to {width}x{height}"))
            }
            ActionCommand::Sys(_) | ActionCommand::Notif(_) => {
                let patches = self.sys_providers.handle_action(&action)?;
                self.apply_state_patches(patches);
                Ok(format!("Dispatched system action: {action:?}"))
            }
            ActionCommand::Apps(ref apps_act) => {
                let launched_id = match apps_act {
                    wm_proto::AppsAction::Launch { app_id } => Some(app_id.clone()),
                    wm_proto::AppsAction::LaunchSelected => {
                        let idx = self
                            .store
                            .get("apps.selected_idx")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)
                            .max(0) as usize;
                        self.store
                            .get("apps.filtered")
                            .and_then(|v| match v {
                                StateValue::List(list) => list.get(idx).cloned(),
                                _ => None,
                            })
                            .and_then(|item| match item {
                                StateValue::Map(m) => m
                                    .get("id")
                                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                                _ => None,
                            })
                    }
                    _ => None,
                };
                let patches = self.sys_providers.handle_action(&action)?;
                self.apply_state_patches(patches);
                if let Some(app_id) = launched_id {
                    self.spawn_app_window(&app_id);
                }
                Ok(format!("Dispatched apps action: {apps_act:?}"))
            }
            ActionCommand::State(state_act) => {
                self.store.apply_state_action(&state_act);
                self.recompute_all_surfaces();
                Ok(format!("Applied state action: {state_act:?}"))
            }
            ActionCommand::Exec { command, args } => {
                let full_cmd = if args.is_empty() {
                    command.clone()
                } else {
                    format!("{command} {}", args.join(" "))
                };
                self.spawn_app_window(&full_cmd);
                Ok(format!("Exec command '{command}' with args {args:?}"))
            }
        }
    }

    fn spawn_app_window(&mut self, app_id: &str) {
        let lower = app_id.to_ascii_lowercase();

        // Only use the built-in simulated terminal when explicitly requested via `org.lumina.terminal`
        // or when running in headless mode without an embedded Wayland server.
        let is_builtin_terminal = lower == "org.lumina.terminal" || self.wayland_server.is_none();

        if !is_builtin_terminal && self.wayland_server.is_some() {
            let exec_cmd = if (lower == "alacritty"
                || lower == "org.alacritty.alacritty"
                || lower == "terminal"
                || lower == "org.gnome.console"
                || lower == "kgx")
                && Path::new("/usr/bin/kgx").exists()
            {
                "kgx".to_string()
            } else {
                self.sys_providers
                    .apps
                    .find_app(app_id)
                    .map(|entry| entry.exec.clone())
                    .unwrap_or_else(|| app_id.to_string())
            };

            if let Some(ref mut wl) = self.wayland_server {
                if wl.spawn_client(&exec_cmd).is_ok() {
                    return;
                }
            }
        }

        let (title, icon, effective_app_id) = if is_builtin_terminal
            || lower.contains("term")
            || lower.contains("alacritty")
            || lower.contains("kitty")
            || lower.contains("foot")
        {
            (
                "Lumina Terminal — zsh (~/scratch/lumina-wm)".to_string(),
                "terminal".to_string(),
                "org.lumina.terminal".to_string(),
            )
        } else if lower.contains("firefox")
            || lower.contains("chrome")
            || lower.contains("browser")
            || lower.contains("web")
        {
            (
                "Mozilla Firefox — Lumina Wayland HTML+CSS Architecture".to_string(),
                "firefox".to_string(),
                app_id.to_string(),
            )
        } else if lower.contains("nautilus")
            || lower.contains("files")
            || lower.contains("thunar")
            || lower.contains("dolphin")
        {
            (
                "Files — ~/scratch/lumina-wm".to_string(),
                "files".to_string(),
                app_id.to_string(),
            )
        } else if lower.contains("code")
            || lower.contains("zed")
            || lower.contains("nvim")
            || lower.contains("editor")
        {
            (
                "Zed Editor — crates/wm-cli/src/daemon.rs".to_string(),
                "code".to_string(),
                app_id.to_string(),
            )
        } else {
            (app_id.to_string(), app_id.to_string(), app_id.to_string())
        };

        let next_id = self
            .wm
            .windows()
            .iter()
            .map(|w| w.id)
            .max()
            .unwrap_or(200)
            + 1;
        let wm_patches = self.wm.on_window_opened(WindowDescriptor {
            id: next_id,
            title,
            app_id: effective_app_id,
            icon,
            workspace_id: self.wm.active_workspace_id(),
            is_focused: true,
            is_floating: false,
            is_fullscreen: false,
            is_maximized: false,
            is_minimized: false,
            is_urgent: false,
            rect: Rect::default(),
        });
        self.apply_state_patches(wm_patches);
    }

    pub fn reload_from_disk(&mut self, kind: ReloadTarget) {
        self.reload_count += 1;
        if let Some(dir) = self.assets_dir.clone() {
            if matches!(kind, ReloadTarget::Css | ReloadTarget::All) {
                let css_file = if self.current_theme.contains("cyber") {
                    dir.join("css/theme-cyberpunk.css")
                } else {
                    dir.join("css/theme-catppuccin.css")
                };
                if let Ok(css_text) = std::fs::read_to_string(&css_file) {
                    let _ = self.style_engine.load_stylesheet(&css_text);
                    for sheet in &self.custom_css_sheets {
                        let _ = self.style_engine.append_stylesheet(sheet);
                    }
                    for (k, v) in &self.css_variables {
                        self.style_engine.set_custom_property(k, v);
                    }
                }
            }
            if matches!(kind, ReloadTarget::Html | ReloadTarget::All) {
                for (surf_name, filename) in [
                    ("topbar", "topbar.html"),
                    ("panel", "topbar.html"),
                    ("dock", "dock.html"),
                    ("launcher", "launcher.html"),
                    ("quick_settings", "quick_settings.html"),
                    ("control-center", "quick_settings.html"),
                ] {
                    let path = dir.join(filename);
                    if let Ok(html_text) = std::fs::read_to_string(&path) {
                        if let Some(surf) = self.surfaces.get_mut(surf_name) {
                            surf.html_source = html_text.clone();
                            surf.dom = DomDocument::parse(&html_text);
                        }
                    }
                }
            }
        }
        self.recompute_all_surfaces();
    }

    /// Processes a single `IpcRequest` and returns an `IpcResponse`.
    pub fn handle_ipc_request(&mut self, req: IpcRequest) -> IpcResponse {
        let id = req.id;
        match req.command {
            IpcCommand::Ping => IpcResponse::success(
                id,
                IpcPayload::Pong {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
            ),
            IpcCommand::WmWorkspaces => {
                IpcResponse::success(id, IpcPayload::Workspaces(self.wm.workspaces()))
            }
            IpcCommand::WmWindows => {
                IpcResponse::success(id, IpcPayload::Windows(self.wm.windows()))
            }
            IpcCommand::WmOutputs => {
                IpcResponse::success(id, IpcPayload::Outputs(self.wm.outputs()))
            }
            IpcCommand::WmDispatch { action } => match self.dispatch_action(action) {
                Ok(msg) => IpcResponse::success(id, IpcPayload::Message(msg)),
                Err(err) => IpcResponse::err(id, err),
            },
            IpcCommand::DomQuery { surface, selector } => {
                let surf_name = surface.unwrap_or_else(|| "panel".to_string());
                let matched = self.query_dom_elements(&surf_name, &selector);
                IpcResponse::success(id, IpcPayload::DomElements(matched))
            }
            IpcCommand::DomTree { surface, max_depth } => {
                let surf_name = surface.unwrap_or_else(|| "panel".to_string());
                let tree = if let Some(surf) = self.surfaces.get(&surf_name) {
                    let root_id = surf.dom.root_id();
                    vec![surf.dom.export_element_info(
                        &surf_name,
                        root_id,
                        Some(&surf.layouts),
                        max_depth,
                    )]
                } else {
                    Vec::new()
                };
                IpcResponse::success(id, IpcPayload::DomElements(tree))
            }
            IpcCommand::DomInspectStyle { surface, selector } => {
                let surf_name = surface.unwrap_or_else(|| "panel".to_string());
                let mut reports = Vec::new();
                if let Some(surf) = self.surfaces.get(&surf_name) {
                    for node_id in self.style_engine.query_selector_all(&surf.dom, &selector) {
                        reports.push(self.style_engine.inspect_node_style(&surf.dom, node_id));
                    }
                }
                IpcResponse::success(id, IpcPayload::StyleInspection(reports))
            }
            IpcCommand::DomSetAttr {
                surface,
                selector,
                attr,
                value,
            } => {
                if let Some(surf) = self.surfaces.get_mut(&surface) {
                    let matched = self.style_engine.query_selector_all(&surf.dom, &selector);
                    for node_id in matched {
                        surf.dom.set_attr(node_id, &attr, value.clone());
                    }
                }
                self.recompute_all_surfaces();
                IpcResponse::success(
                    id,
                    IpcPayload::Message(format!(
                        "Updated attribute '{attr}'={value:?} on '{selector}' in surface '{surface}'"
                    )),
                )
            }
            IpcCommand::Reload { kind, surface } => {
                self.reload_from_disk(kind);
                IpcResponse::success(
                    id,
                    IpcPayload::Message(format!(
                        "Reloaded {kind:?} (surface={surface:?}, count={})",
                        self.reload_count
                    )),
                )
            }
            IpcCommand::CssApply { surface, css_text } => {
                let _ = self.style_engine.append_stylesheet(&css_text);
                self.custom_css_sheets.push(css_text.clone());
                self.recompute_all_surfaces();
                IpcResponse::success(
                    id,
                    IpcPayload::Message(format!(
                        "Applied live CSS ({} bytes) to surface {:?}",
                        css_text.len(),
                        surface.as_deref().unwrap_or("all")
                    )),
                )
            }
            IpcCommand::CssSetVar {
                surface,
                name,
                value,
            } => {
                let normalized = if name.starts_with("--") {
                    name.clone()
                } else {
                    format!("--{}", name.trim_start_matches('-'))
                };
                self.css_variables.insert(normalized.clone(), value.clone());
                self.style_engine.set_custom_property(&normalized, &value);
                self.wm.apply_css_vars(&self.css_variables);
                let patches = self.wm.export_state_patches();
                self.apply_state_patches(patches);
                IpcResponse::success(
                    id,
                    IpcPayload::Message(format!(
                        "Set CSS variable {normalized}={value} on surface {:?}",
                        surface.as_deref().unwrap_or("all")
                    )),
                )
            }
            IpcCommand::WidgetList => {
                let list: Vec<WidgetStatus> = self.widgets.values().cloned().collect();
                IpcResponse::success(id, IpcPayload::Widgets(list))
            }
            IpcCommand::WidgetOpen { name, .. } => match self.set_widget_visibility(&name, Some(true)) {
                Ok(_) => {
                    let list: Vec<WidgetStatus> = self.widgets.values().cloned().collect();
                    IpcResponse::success(id, IpcPayload::Widgets(list))
                }
                Err(e) => IpcResponse::err(id, e),
            },
            IpcCommand::WidgetClose { name } => match self.set_widget_visibility(&name, Some(false)) {
                Ok(_) => {
                    let list: Vec<WidgetStatus> = self.widgets.values().cloned().collect();
                    IpcResponse::success(id, IpcPayload::Widgets(list))
                }
                Err(e) => IpcResponse::err(id, e),
            },
            IpcCommand::WidgetToggle { name, .. } => match self.set_widget_visibility(&name, None) {
                Ok(_) => {
                    let list: Vec<WidgetStatus> = self.widgets.values().cloned().collect();
                    IpcResponse::success(id, IpcPayload::Widgets(list))
                }
                Err(e) => IpcResponse::err(id, e),
            },
            IpcCommand::StateGet { path } => {
                let val = self.get_state_path(path.as_deref());
                IpcResponse::success(id, IpcPayload::State(val))
            }
            IpcCommand::StateSet { path, value } => {
                self.set_state_path(&path, value.clone());
                IpcResponse::success(id, IpcPayload::State(value))
            }
            IpcCommand::Subscribe { topics } => IpcResponse::success(
                id,
                IpcPayload::Message(format!("Subscribed to topics: {topics:?}")),
            ),
        }
    }

    /// Spawns a background IPC server wired to a shared `LuminaDaemon` instance.
    pub fn spawn_ipc_server(
        daemon: Arc<Mutex<Self>>,
        socket_path: impl Into<PathBuf>,
    ) -> Result<IpcServerHandle, IpcError> {
        IpcServer::spawn(socket_path, move |req| {
            let mut guard = daemon.lock().expect("daemon mutex poisoned");
            guard.handle_ipc_request(req)
        })
    }

    /// Polls `AssetWatcher` and triggers live CSS/HTML hot-reload if any files changed.
    pub fn poll_hot_reload(&mut self, watcher: &mut AssetWatcher) -> Vec<ReloadTarget> {
        let targets = watcher.poll_reload_targets();
        for &t in &targets {
            self.reload_from_disk(t);
        }
        targets
    }

    fn query_dom_elements(&self, surface: &str, selector: &str) -> Vec<DomElementInfo> {
        let Some(surf) = self.surfaces.get(surface) else {
            return Vec::new();
        };
        let matched_ids = self.style_engine.query_selector_all(&surf.dom, selector);
        matched_ids
            .into_iter()
            .map(|id| surf.dom.export_element_info(surface, id, Some(&surf.layouts), Some(0)))
            .collect()
    }

    /// Saves a full desktop composite frame to a `.bmp` or `.ppm` file.
    pub fn save_desktop_screenshot(&mut self, path: &Path) -> std::io::Result<FrameBuffer> {
        let fb = self.render_desktop_frame();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("ppm"))
            .unwrap_or(false)
        {
            renderer::save_ppm(&fb, path)?;
        } else {
            renderer::save_bmp(&fb, path)?;
        }
        Ok(fb)
    }
}

/// Alpha-blends `src` RGBA8 framebuffer onto `dst` at `(offset_x, offset_y)`.
fn blit_framebuffer(dst: &mut FrameBuffer, src: &FrameBuffer, offset_x: i32, offset_y: i32) {
    let dst_w = dst.width as i32;
    let dst_h = dst.height as i32;
    let src_w = src.width as i32;
    let src_h = src.height as i32;

    let sx_start = (-offset_x).max(0);
    let sy_start = (-offset_y).max(0);
    let sx_end = (dst_w - offset_x).min(src_w);
    let sy_end = (dst_h - offset_y).min(src_h);
    if sx_start >= sx_end || sy_start >= sy_end {
        return;
    }

    let dst_stride = (dst_w as usize) * 4;
    let src_stride = (src_w as usize) * 4;
    let span_bytes = ((sx_end - sx_start) as usize) * 4;

    for sy in sy_start..sy_end {
        let dy = (offset_y + sy) as usize;
        let s_row_start = (sy as usize) * src_stride + (sx_start as usize) * 4;
        let d_row_start = dy * dst_stride + ((offset_x + sx_start) as usize) * 4;

        let src_slice = &src.pixels[s_row_start..s_row_start + span_bytes];
        let dst_slice = &mut dst.pixels[d_row_start..d_row_start + span_bytes];

        for (sp, dp) in src_slice.chunks_exact(4).zip(dst_slice.chunks_exact_mut(4)) {
            let sa = sp[3] as u32;
            if sa == 0 {
                continue;
            }
            if sa == 255 {
                dp.copy_from_slice(sp);
            } else {
                let inv_a = 255 - sa;
                dp[0] = ((sp[0] as u32) + ((dp[0] as u32) * inv_a) / 255).min(255) as u8;
                dp[1] = ((sp[1] as u32) + ((dp[1] as u32) * inv_a) / 255).min(255) as u8;
                dp[2] = ((sp[2] as u32) + ((dp[2] as u32) * inv_a) / 255).min(255) as u8;
                dp[3] = 255;
            }
        }
    }
}
