use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ipc_server::{default_socket_path, AssetWatcher, IpcServer};
use renderer::{hit_test_titlebar, TitlebarHit};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};
use wm_proto::{
    ActionCommand, AppsAction, Point, Rect, ShellAction, WindowManagerEngine, WmAction,
    WorkspaceTarget,
};
use crate::daemon::LuminaDaemon;

const DESKTOP_W: f32 = 1920.0;
const DESKTOP_H: f32 = 1080.0;

#[derive(Clone, Copy, Debug)]
struct DragState {
    start_desktop_pt: Point,
}

pub struct NestedApp {
    socket_path: PathBuf,
    assets_path: PathBuf,
    theme: String,
    initial_actions: Vec<String>,
    daemon: Option<LuminaDaemon>,
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    ipc_server: Option<IpcServer>,
    watcher: Option<AssetWatcher>,
    cursor_pos: Point,
    cursor_dirty: bool,
    drag_state: Option<DragState>,
    pressed_client_win: Option<u64>,
    last_sys_poll: Instant,
    last_redraw: Instant,
    needs_redraw: bool,
    width: u32,
    height: u32,
}

impl NestedApp {
    pub fn new(
        socket: Option<PathBuf>,
        assets: PathBuf,
        theme: String,
        initial_actions: Vec<String>,
    ) -> Self {
        let socket_path = socket.unwrap_or_else(default_socket_path);
        let now = Instant::now();
        Self {
            socket_path,
            assets_path: assets,
            theme,
            initial_actions,
            daemon: None,
            window: None,
            context: None,
            surface: None,
            ipc_server: None,
            watcher: None,
            cursor_pos: Point::new(0.0, 0.0),
            cursor_dirty: false,
            drag_state: None,
            pressed_client_win: None,
            last_sys_poll: now,
            last_redraw: now,
            needs_redraw: true,
            width: 1600,
            height: 900,
        }
    }

    pub fn run(mut self) -> Result<(), Box<dyn std::error::Error>> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
        ));
        event_loop.run_app(&mut self)?;
        Ok(())
    }

    /// Maps window physical coordinates `(cursor_pos)` to the `1920x1080` logical desktop coordinate space.
    fn desktop_cursor(&self) -> Point {
        let sx = DESKTOP_W / (self.width.max(1) as f32);
        let sy = DESKTOP_H / (self.height.max(1) as f32);
        Point::new(self.cursor_pos.x * sx, self.cursor_pos.y * sy)
    }

    fn request_redraw(&mut self) {
        self.needs_redraw = true;
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Coalesces high-frequency Wayland mouse motion events so layout/hover/drag updates
    /// happen at most once per rendered frame instead of on every 1000Hz pointer event.
    fn process_pending_cursor_move(&mut self) -> bool {
        if !self.cursor_dirty {
            return false;
        }
        self.cursor_dirty = false;
        let desktop_pt = self.desktop_cursor();

        // Fast O(1) interactive window move: no DOM/CSS/Taffy recomputation during drag!
        if let Some(drag) = self.drag_state {
            let dx = desktop_pt.x - drag.start_desktop_pt.x;
            let dy = desktop_pt.y - drag.start_desktop_pt.y;
            if let Some(daemon) = self.daemon.as_mut() {
                return daemon.wm.move_interactive_grab_fast(dx, dy);
            }
            return false;
        }

        let Some(daemon) = self.daemon.as_mut() else {
            return false;
        };

        let mut hover_changed = false;

        // Update hover state on visible surfaces in desktop coordinates
        let launcher_visible = daemon
            .widgets
            .get("launcher")
            .map(|w| w.visible)
            .unwrap_or(false);
        if launcher_visible {
            let lx = (DESKTOP_W - 640.0) / 2.0;
            let ly = (DESKTOP_H - 440.0) / 2.0;
            hover_changed |=
                daemon.update_hover("launcher", desktop_pt.x - lx, desktop_pt.y - ly);
        }

        let qs_visible = daemon
            .widgets
            .get("quick_settings")
            .map(|w| w.visible)
            .unwrap_or(false);
        if qs_visible {
            let qx = DESKTOP_W - 380.0 - 16.0;
            let qy = 54.0;
            hover_changed |=
                daemon.update_hover("quick_settings", desktop_pt.x - qx, desktop_pt.y - qy);
        }

        hover_changed |= daemon.update_hover("topbar", desktop_pt.x, desktop_pt.y);

        let dock_visible = daemon
            .surfaces
            .get("dock")
            .map(|s| s.visible)
            .unwrap_or(true);
        if dock_visible {
            let dx = (DESKTOP_W - 680.0) / 2.0;
            let dy = DESKTOP_H - 58.0 - 10.0;
            hover_changed |= daemon.update_hover("dock", desktop_pt.x - dx, desktop_pt.y - dy);
        }

        // Forward pointer motion to the topmost Wayland client window if cursor is inside its content_rect
        if !launcher_visible && !qs_visible && daemon.wayland_server.is_some() {
            let usable = daemon
                .wm
                .outputs()
                .first()
                .map(|o| o.usable_rect)
                .unwrap_or(Rect::new(0.0, 44.0, DESKTOP_W, DESKTOP_H - 44.0));
            let placements = daemon.wm.compute_window_placements(usable);
            for p in placements.iter().rev() {
                if p.content_rect.contains(desktop_pt) {
                    if let Some(ref mut wl) = daemon.wayland_server {
                        let local_x = desktop_pt.x - p.content_rect.x;
                        let local_y = desktop_pt.y - p.content_rect.y;
                        wl.send_pointer_motion(p.window_id, local_x, local_y);
                    }
                    break;
                }
            }
        }

        hover_changed
    }

    fn handle_mouse_click(&mut self, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }

        let desktop_pt = self.desktop_cursor();
        let Point { x, y } = desktop_pt;
        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };

        // 1. Check if Launcher modal is open (centered 640x440 on 1920x1080)
        let launcher_visible = daemon
            .widgets
            .get("launcher")
            .map(|w| w.visible)
            .unwrap_or(false);
        if launcher_visible {
            let lx = (DESKTOP_W - 640.0) / 2.0;
            let ly = (DESKTOP_H - 440.0) / 2.0;
            let modal_rect = Rect::new(lx, ly, 640.0, 440.0);
            if modal_rect.contains(desktop_pt) {
                let _ = daemon.click_surface("launcher", x - lx, y - ly);
            } else {
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "launcher".to_string(),
                }));
            }
            self.request_redraw();
            return;
        }

        // 2. Check if Quick Settings modal is open (top-right 380x520 on 1920x1080)
        let qs_visible = daemon
            .widgets
            .get("quick_settings")
            .map(|w| w.visible)
            .unwrap_or(false);
        if qs_visible {
            let qx = DESKTOP_W - 380.0 - 16.0;
            let qy = 54.0;
            let modal_rect = Rect::new(qx, qy, 380.0, 520.0);
            if modal_rect.contains(desktop_pt) {
                let _ = daemon.click_surface("quick_settings", x - qx, y - qy);
            } else {
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "quick_settings".to_string(),
                }));
            }
            self.request_redraw();
            return;
        }

        // 3. Topbar: y < 44
        if y < 44.0 {
            let _ = daemon.click_surface("topbar", x, y);
            self.request_redraw();
            return;
        }

        // 4. Bottom Dock: centered at bottom (680x58)
        let dock_visible = daemon
            .surfaces
            .get("dock")
            .map(|s| s.visible)
            .unwrap_or(true);
        if dock_visible {
            let dock_w = 680.0;
            let dock_h = 58.0;
            let dx = (DESKTOP_W - dock_w) / 2.0;
            let dy = DESKTOP_H - dock_h - 10.0;
            let dock_rect = Rect::new(dx, dy, dock_w, dock_h);
            if dock_rect.contains(desktop_pt) {
                let _ = daemon.click_surface("dock", x - dx, y - dy);
                self.request_redraw();
                return;
            }
        }

        // 5. Window Frame / Server-Side Titlebar buttons & Client Area
        let usable = daemon
            .wm
            .outputs()
            .first()
            .map(|o| o.usable_rect)
            .unwrap_or(Rect::new(0.0, 44.0, DESKTOP_W, DESKTOP_H - 44.0));
        let placements = daemon.wm.compute_window_placements(usable);

        for p in placements.iter().rev() {
            if p.frame_rect.contains(desktop_pt) {
                // Focus clicked window first
                if !p.is_focused {
                    let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWindow {
                        id: p.window_id,
                    }));
                }

                // Check server-side titlebar hit (Float, Maximize, Close, or DragArea)
                if let Some(hit) = hit_test_titlebar(p, desktop_pt) {
                    match hit {
                        TitlebarHit::Float => {
                            let _ = daemon.dispatch_action(ActionCommand::Wm(
                                WmAction::ToggleFloating {
                                    id: Some(p.window_id),
                                },
                            ));
                        }
                        TitlebarHit::Maximize => {
                            let _ = daemon.dispatch_action(ActionCommand::Wm(
                                WmAction::ToggleMaximize {
                                    id: Some(p.window_id),
                                },
                            ));
                        }
                        TitlebarHit::Close => {
                            let _ = daemon.dispatch_action(ActionCommand::Wm(
                                WmAction::CloseWindow {
                                    id: Some(p.window_id),
                                },
                            ));
                        }
                        TitlebarHit::DragArea => {
                            if p.is_floating {
                                let _ = daemon.wm.dispatch_wm_action(
                                    &WmAction::BeginInteractiveMove {
                                        win_id: p.window_id,
                                    },
                                );
                                self.drag_state = Some(DragState {
                                    start_desktop_pt: desktop_pt,
                                });
                            }
                        }
                    }
                } else if p.content_rect.contains(desktop_pt) {
                    // Forward pointer button press into the real Wayland client surface
                    if let Some(ref mut wl) = daemon.wayland_server {
                        let local_x = desktop_pt.x - p.content_rect.x;
                        let local_y = desktop_pt.y - p.content_rect.y;
                        wl.send_pointer_button(p.window_id, local_x, local_y, true);
                        self.pressed_client_win = Some(p.window_id);
                    }
                }
                self.request_redraw();
                return;
            }
        }
    }

    fn handle_mouse_scroll(&mut self, delta: MouseScrollDelta) {
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => y,
            MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
        };
        if dy.abs() < 0.01 {
            return;
        }
        let scroll_up = dy > 0.0;
        let desktop_pt = self.desktop_cursor();

        if let Some(daemon) = self.daemon.as_mut() {
            if desktop_pt.y < 44.0 {
                let _ = daemon.scroll_surface("topbar", desktop_pt.x, desktop_pt.y, scroll_up);
                self.request_redraw();
            }
        }
    }

    fn handle_key_input(&mut self, key_event: KeyEvent) {
        if key_event.state != ElementState::Pressed {
            return;
        }

        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };

        let launcher_open = daemon
            .widgets
            .get("launcher")
            .map(|w| w.visible)
            .unwrap_or(false);

        match &key_event.logical_key {
            Key::Named(NamedKey::Super) => {
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::ToggleWidget {
                    name: "launcher".to_string(),
                    args: Default::default(),
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::Escape) => {
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "launcher".to_string(),
                }));
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "quick_settings".to_string(),
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::Tab) => {
                let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusDirection {
                    dir: wm_proto::Direction::Right,
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) if launcher_open => {
                let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SelectRelative {
                    delta: 1,
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowUp) if launcher_open => {
                let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SelectRelative {
                    delta: -1,
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::F1) => {
                let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                    target: WorkspaceTarget::Index(1),
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::F2) => {
                let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                    target: WorkspaceTarget::Index(2),
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::F3) => {
                let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                    target: WorkspaceTarget::Index(3),
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::F4) => {
                let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                    target: WorkspaceTarget::Index(4),
                }));
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                if launcher_open {
                    let _ =
                        daemon.dispatch_action(ActionCommand::Apps(AppsAction::LaunchSelected));
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "launcher".to_string(),
                    }));
                } else {
                    daemon.terminal_execute();
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Space) => {
                if launcher_open {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.push(' ');
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SetQuery {
                        query,
                    }));
                } else {
                    daemon.terminal_push_str(" ");
                }
                self.request_redraw();
            }
            Key::Character(c) => {
                if launcher_open {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.push_str(c);
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SetQuery {
                        query,
                    }));
                } else {
                    daemon.terminal_push_str(c);
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::Backspace) => {
                if launcher_open {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.pop();
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SetQuery {
                        query,
                    }));
                } else {
                    daemon.terminal_backspace();
                }
                self.request_redraw();
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for NestedApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window_attrs = WindowAttributes::default()
            .with_title("Lumina WM — Pure-Rust HTML/CSS Wayland Compositor (Nested)")
            .with_inner_size(LogicalSize::new(self.width, self.height));

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Failed to create nested window: {e}");
                return;
            }
        };

        let context = match Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to create softbuffer context: {e}");
                return;
            }
        };

        let surface = match Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to create softbuffer surface: {e}");
                return;
            }
        };

        // Initialize daemon
        let mut daemon = LuminaDaemon::new();
        daemon.assets_dir = Some(self.assets_path.clone());
        // Start interactive nested session with a clean desktop (no pre-opened fake windows)
        daemon.clear_startup_windows();
        match daemon.enable_wayland_server() {
            Ok(sock_name) => {
                println!(
                    "Embedded Wayland Compositor active on WAYLAND_DISPLAY={sock_name}"
                );
            }
            Err(e) => {
                eprintln!("Warning: could not bind embedded Wayland server: {e}");
            }
        }
        if let Err(e) = daemon.set_theme(&self.theme) {
            eprintln!("Warning: failed to load theme '{}': {e}", self.theme);
        }

        for dsl in &self.initial_actions {
            if let Ok(cmds) = ActionCommand::parse_dsl_list(dsl) {
                for cmd in cmds {
                    let _ = daemon.dispatch_action(cmd);
                }
            }
        }

        // Bind IPC server
        let ipc_server = match IpcServer::bind(&self.socket_path) {
            Ok(s) => {
                println!(
                    "Nested Lumina WM started!\n- IPC Socket: {:?}\n- To control via CLI: ./target/release/luminactl --socket {:?} <command>",
                    s.socket_path(),
                    s.socket_path()
                );
                Some(s)
            }
            Err(e) => {
                eprintln!("Warning: could not bind IPC socket at {:?}: {e}", self.socket_path);
                None
            }
        };

        let watcher = AssetWatcher::new(&self.assets_path);

        window.request_redraw();
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        self.daemon = Some(daemon);
        self.ipc_server = ipc_server;
        self.watcher = Some(watcher);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.width = size.width.max(1);
                self.height = size.height.max(1);
                if let Some(surface) = self.surface.as_mut() {
                    if let (Some(w), Some(h)) = (
                        NonZeroU32::new(self.width),
                        NonZeroU32::new(self.height),
                    ) {
                        let _ = surface.resize(w, h);
                    }
                }
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Point::new(position.x as f32, position.y as f32);
                self.cursor_dirty = true;
                // Throttle redraw requests during high-frequency mouse motion to ~60 FPS
                if self.last_redraw.elapsed() >= Duration::from_millis(14) {
                    if self.drag_state.is_some() || self.process_pending_cursor_move() {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => {
                    self.handle_mouse_click(button);
                }
                ElementState::Released => {
                    if button == MouseButton::Left {
                        if self.drag_state.take().is_some() {
                            if let Some(daemon) = self.daemon.as_mut() {
                                daemon.wm.end_interactive_grab();
                                let patches = daemon.wm.export_state_patches();
                                daemon.apply_state_patches(patches);
                            }
                            self.request_redraw();
                        }
                        if let Some(win_id) = self.pressed_client_win.take() {
                            let desktop_pt = self.desktop_cursor();
                            if let Some(daemon) = self.daemon.as_mut() {
                                let usable = daemon
                                    .wm
                                    .outputs()
                                    .first()
                                    .map(|o| o.usable_rect)
                                    .unwrap_or(Rect::new(0.0, 44.0, DESKTOP_W, DESKTOP_H - 44.0));
                                let placements = daemon.wm.compute_window_placements(usable);
                                let (local_x, local_y) = placements
                                    .iter()
                                    .find(|p| p.window_id == win_id)
                                    .map(|p| {
                                        (
                                            desktop_pt.x - p.content_rect.x,
                                            desktop_pt.y - p.content_rect.y,
                                        )
                                    })
                                    .unwrap_or((0.0, 0.0));
                                if let Some(ref mut wl) = daemon.wayland_server {
                                    wl.send_pointer_button(win_id, local_x, local_y, false);
                                }
                            }
                            self.request_redraw();
                        }
                    }
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                self.handle_mouse_scroll(delta);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key_input(event);
            }
            WindowEvent::RedrawRequested => {
                self.needs_redraw = false;
                self.last_redraw = Instant::now();
                let _ = self.process_pending_cursor_move();

                // Render composite frame and blit to softbuffer
                let (width, height) = (self.width, self.height);
                if let (Some(daemon), Some(surface)) =
                    (self.daemon.as_mut(), self.surface.as_mut())
                {
                    let _ = daemon.poll_wayland_clients();
                    let frame = daemon.render_desktop_frame();

                    if let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
                        let _ = surface.resize(w, h);
                    }

                    if let Ok(mut buffer) = surface.buffer_mut() {
                        let frame_w = frame.width as usize;
                        let frame_h = frame.height as usize;
                        let dst_w = width as usize;
                        let dst_h = height as usize;

                        for y in 0..dst_h {
                            let src_y = (y * frame_h) / dst_h;
                            let src_row = src_y * frame_w * 4;
                            let dst_row = y * dst_w;

                            for x in 0..dst_w {
                                let src_x = (x * frame_w) / dst_w;
                                let s_idx = src_row + src_x * 4;
                                let r = frame.pixels[s_idx] as u32;
                                let g = frame.pixels[s_idx + 1] as u32;
                                let b = frame.pixels[s_idx + 2] as u32;
                                buffer[dst_row + x] = (r << 16) | (g << 8) | b;
                            }
                        }

                        let _ = buffer.present();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut dirty = false;

        // 0. Process any remaining coalesced cursor movement
        if self.cursor_dirty && self.last_redraw.elapsed() >= Duration::from_millis(14) {
            if self.process_pending_cursor_move() {
                dirty = true;
            }
        }

        // 1. Poll embedded Wayland client applications (real apps rendering into Lumina WM)
        if let Some(daemon) = self.daemon.as_mut() {
            if daemon.poll_wayland_clients() {
                dirty = true;
            }
        }

        // 2. Poll IPC requests without blocking
        if let (Some(daemon), Some(ipc)) = (self.daemon.as_mut(), self.ipc_server.as_mut()) {
            if let Ok(count) = ipc.poll_once(|req| daemon.handle_ipc_request(req)) {
                if count > 0 {
                    dirty = true;
                }
            }
        }

        // 3. Poll file hot reload
        if let (Some(daemon), Some(watcher)) = (self.daemon.as_mut(), self.watcher.as_mut()) {
            let reloaded = daemon.poll_hot_reload(watcher);
            if !reloaded.is_empty() {
                dirty = true;
            }
        }

        // 4. Poll system metrics once every 1.5s so idle CPU usage stays near 0%
        let now = Instant::now();
        if now.duration_since(self.last_sys_poll) >= Duration::from_millis(1500) {
            self.last_sys_poll = now;
            if let Some(daemon) = self.daemon.as_mut() {
                let patches = daemon.sys_providers.poll_patches();
                if !patches.is_empty() {
                    daemon.apply_state_patches(patches);
                    dirty = true;
                }
            }
        }

        if dirty {
            self.request_redraw();
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
        ));
    }
}
