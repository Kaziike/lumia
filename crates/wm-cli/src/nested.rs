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
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};
use wm_proto::{
    ActionCommand, AppsAction, Point, Rect, ShellAction, WindowManagerEngine, WmAction,
    WorkspaceTarget,
};
use crate::daemon::LuminaDaemon;

#[derive(Clone, Copy, Debug)]
struct DragState {
    start_desktop_pt: Point,
}

fn mouse_button_to_linux_code(button: MouseButton) -> Option<u32> {
    match button {
        MouseButton::Left => Some(0x110),
        MouseButton::Right => Some(0x111),
        MouseButton::Middle => Some(0x112),
        _ => None,
    }
}

fn keycode_to_evdev(code: KeyCode) -> Option<u32> {
    match code {
        KeyCode::Escape => Some(1),
        KeyCode::Digit1 => Some(2),
        KeyCode::Digit2 => Some(3),
        KeyCode::Digit3 => Some(4),
        KeyCode::Digit4 => Some(5),
        KeyCode::Digit5 => Some(6),
        KeyCode::Digit6 => Some(7),
        KeyCode::Digit7 => Some(8),
        KeyCode::Digit8 => Some(9),
        KeyCode::Digit9 => Some(10),
        KeyCode::Digit0 => Some(11),
        KeyCode::Minus => Some(12),
        KeyCode::Equal => Some(13),
        KeyCode::Backspace => Some(14),
        KeyCode::Tab => Some(15),
        KeyCode::KeyQ => Some(16),
        KeyCode::KeyW => Some(17),
        KeyCode::KeyE => Some(18),
        KeyCode::KeyR => Some(19),
        KeyCode::KeyT => Some(20),
        KeyCode::KeyY => Some(21),
        KeyCode::KeyU => Some(22),
        KeyCode::KeyI => Some(23),
        KeyCode::KeyO => Some(24),
        KeyCode::KeyP => Some(25),
        KeyCode::BracketLeft => Some(26),
        KeyCode::BracketRight => Some(27),
        KeyCode::Enter | KeyCode::NumpadEnter => Some(28),
        KeyCode::ControlLeft => Some(29),
        KeyCode::KeyA => Some(30),
        KeyCode::KeyS => Some(31),
        KeyCode::KeyD => Some(32),
        KeyCode::KeyF => Some(33),
        KeyCode::KeyG => Some(34),
        KeyCode::KeyH => Some(35),
        KeyCode::KeyJ => Some(36),
        KeyCode::KeyK => Some(37),
        KeyCode::KeyL => Some(38),
        KeyCode::Semicolon => Some(39),
        KeyCode::Quote => Some(40),
        KeyCode::Backquote => Some(41),
        KeyCode::ShiftLeft => Some(42),
        KeyCode::Backslash => Some(43),
        KeyCode::KeyZ => Some(44),
        KeyCode::KeyX => Some(45),
        KeyCode::KeyC => Some(46),
        KeyCode::KeyV => Some(47),
        KeyCode::KeyB => Some(48),
        KeyCode::KeyN => Some(49),
        KeyCode::KeyM => Some(50),
        KeyCode::Comma => Some(51),
        KeyCode::Period => Some(52),
        KeyCode::Slash => Some(53),
        KeyCode::ShiftRight => Some(54),
        KeyCode::NumpadMultiply => Some(55),
        KeyCode::AltLeft => Some(56),
        KeyCode::Space => Some(57),
        KeyCode::CapsLock => Some(58),
        KeyCode::F1 => Some(59),
        KeyCode::F2 => Some(60),
        KeyCode::F3 => Some(61),
        KeyCode::F4 => Some(62),
        KeyCode::F5 => Some(63),
        KeyCode::F6 => Some(64),
        KeyCode::F7 => Some(65),
        KeyCode::F8 => Some(66),
        KeyCode::F9 => Some(67),
        KeyCode::F10 => Some(68),
        KeyCode::F11 => Some(87),
        KeyCode::F12 => Some(88),
        KeyCode::ControlRight => Some(97),
        KeyCode::AltRight => Some(100),
        KeyCode::Home => Some(102),
        KeyCode::ArrowUp => Some(103),
        KeyCode::PageUp => Some(104),
        KeyCode::ArrowLeft => Some(105),
        KeyCode::ArrowRight => Some(106),
        KeyCode::End => Some(107),
        KeyCode::ArrowDown => Some(108),
        KeyCode::PageDown => Some(109),
        KeyCode::Insert => Some(110),
        KeyCode::Delete => Some(111),
        KeyCode::SuperLeft => Some(125),
        KeyCode::SuperRight => Some(126),
        _ => None,
    }
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
    pressed_client_win: Option<(u64, u32)>,
    modifiers: ModifiersState,
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
            modifiers: ModifiersState::default(),
            last_sys_poll: now,
            last_redraw: now,
            needs_redraw: true,
            width: 1920,
            height: 1080,
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

    fn desktop_size(&self) -> (f32, f32) {
        if let Some(ref d) = self.daemon {
            (
                d.output_width.max(640) as f32,
                d.output_height.max(480) as f32,
            )
        } else {
            (self.width.max(640) as f32, self.height.max(480) as f32)
        }
    }

    /// Maps window physical coordinates `(cursor_pos)` to the logical desktop coordinate space.
    fn desktop_cursor(&self) -> Point {
        let (dw, dh) = self.desktop_size();
        let sx = dw / (self.width.max(1) as f32);
        let sy = dh / (self.height.max(1) as f32);
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
        let (dw, dh) = self.desktop_size();

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
            let lx = (dw - 640.0) / 2.0;
            let ly = (dh - 440.0) / 2.0;
            hover_changed |=
                daemon.update_hover("launcher", desktop_pt.x - lx, desktop_pt.y - ly);
        }

        let qs_visible = daemon
            .widgets
            .get("quick_settings")
            .map(|w| w.visible)
            .unwrap_or(false);
        if qs_visible {
            let qx = dw - 380.0 - 16.0;
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
            let dx = (dw - 680.0) / 2.0;
            let dy = dh - 58.0 - 10.0;
            hover_changed |= daemon.update_hover("dock", desktop_pt.x - dx, desktop_pt.y - dy);
        }

        // Forward pointer motion to the topmost Wayland client window if cursor is inside its content_rect
        if !launcher_visible && !qs_visible && daemon.wayland_server.is_some() {
            let usable = daemon
                .wm
                .outputs()
                .first()
                .map(|o| o.usable_rect)
                .unwrap_or(Rect::new(0.0, 44.0, dw, (dh - 44.0).max(100.0)));
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
        let Some(btn_code) = mouse_button_to_linux_code(button) else {
            return;
        };

        let desktop_pt = self.desktop_cursor();
        let (dw, dh) = self.desktop_size();
        let Point { x, y } = desktop_pt;
        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };

        if button == MouseButton::Left {
            // 1. Check if Launcher modal is open (centered 640x440)
            let launcher_visible = daemon
                .widgets
                .get("launcher")
                .map(|w| w.visible)
                .unwrap_or(false);
            if launcher_visible {
                let lx = (dw - 640.0) / 2.0;
                let ly = (dh - 440.0) / 2.0;
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

            // 2. Check if Quick Settings modal is open (top-right 380x520)
            let qs_visible = daemon
                .widgets
                .get("quick_settings")
                .map(|w| w.visible)
                .unwrap_or(false);
            if qs_visible {
                let qx = dw - 380.0 - 16.0;
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
                let dx = (dw - dock_w) / 2.0;
                let dy = dh - dock_h - 10.0;
                let dock_rect = Rect::new(dx, dy, dock_w, dock_h);
                if dock_rect.contains(desktop_pt) {
                    let _ = daemon.click_surface("dock", x - dx, y - dy);
                    self.request_redraw();
                    return;
                }
            }
        }

        // 5. Window Frame / Server-Side Titlebar buttons & Client Area
        let usable = daemon
            .wm
            .outputs()
            .first()
            .map(|o| o.usable_rect)
            .unwrap_or(Rect::new(0.0, 44.0, dw, (dh - 44.0).max(100.0)));
        let placements = daemon.wm.compute_window_placements(usable);

        for p in placements.iter().rev() {
            if p.frame_rect.contains(desktop_pt) {
                // Focus clicked window first
                if !p.is_focused {
                    let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWindow {
                        id: p.window_id,
                    }));
                }
                if let Some(ref mut wl) = daemon.wayland_server {
                    wl.focus_window_keyboard(Some(p.window_id));
                }

                // Check server-side titlebar hit (Float, Maximize, Close, or DragArea)
                if button == MouseButton::Left {
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
                        self.request_redraw();
                        return;
                    }
                }

                if p.content_rect.contains(desktop_pt) {
                    // Forward pointer button press into the real Wayland client surface
                    if let Some(ref mut wl) = daemon.wayland_server {
                        let local_x = desktop_pt.x - p.content_rect.x;
                        let local_y = desktop_pt.y - p.content_rect.y;
                        wl.send_pointer_button_code(p.window_id, local_x, local_y, btn_code, true);
                        self.pressed_client_win = Some((p.window_id, btn_code));
                    }
                }
                self.request_redraw();
                return;
            }
        }
    }

    fn handle_mouse_scroll(&mut self, delta: MouseScrollDelta) {
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * 15.0, y * 15.0),
            MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
        };
        if dx.abs() < 0.01 && dy.abs() < 0.01 {
            return;
        }
        let scroll_up = dy > 0.0;
        let desktop_pt = self.desktop_cursor();
        let (dw, dh) = self.desktop_size();

        if let Some(daemon) = self.daemon.as_mut() {
            if desktop_pt.y < 44.0 {
                let _ = daemon.scroll_surface("topbar", desktop_pt.x, desktop_pt.y, scroll_up);
                self.request_redraw();
                return;
            }

            let usable = daemon
                .wm
                .outputs()
                .first()
                .map(|o| o.usable_rect)
                .unwrap_or(Rect::new(0.0, 44.0, dw, (dh - 44.0).max(100.0)));
            let placements = daemon.wm.compute_window_placements(usable);
            for p in placements.iter().rev() {
                if p.content_rect.contains(desktop_pt) {
                    if let Some(ref mut wl) = daemon.wayland_server {
                        // Wayland axis value: positive is down/right
                        wl.send_pointer_axis(p.window_id, -dx, -dy);
                    }
                    self.request_redraw();
                    break;
                }
            }
        }
    }

    fn handle_key_input(&mut self, key_event: KeyEvent) {
        let pressed = key_event.state == ElementState::Pressed;

        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };

        let launcher_open = daemon
            .widgets
            .get("launcher")
            .map(|w| w.visible)
            .unwrap_or(false);
        let qs_open = daemon
            .widgets
            .get("quick_settings")
            .map(|w| w.visible)
            .unwrap_or(false);

        // Global WM shortcuts on key press
        if pressed {
            match &key_event.logical_key {
                Key::Named(NamedKey::Super) => {
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::ToggleWidget {
                        name: "launcher".to_string(),
                        args: Default::default(),
                    }));
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::Escape) if launcher_open || qs_open => {
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "launcher".to_string(),
                    }));
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "quick_settings".to_string(),
                    }));
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::F1) => {
                    let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                        target: WorkspaceTarget::Index(1),
                    }));
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::F2) => {
                    let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                        target: WorkspaceTarget::Index(2),
                    }));
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::F3) => {
                    let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                        target: WorkspaceTarget::Index(3),
                    }));
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::F4) => {
                    let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWorkspace {
                        target: WorkspaceTarget::Index(4),
                    }));
                    self.request_redraw();
                    return;
                }
                _ => {}
            }
        }

        // If Launcher modal is open, route keyboard input exclusively to Launcher search/navigation
        if launcher_open {
            if !pressed {
                return;
            }
            match &key_event.logical_key {
                Key::Named(NamedKey::ArrowDown) => {
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SelectRelative {
                        delta: 1,
                    }));
                    self.request_redraw();
                }
                Key::Named(NamedKey::ArrowUp) => {
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SelectRelative {
                        delta: -1,
                    }));
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) => {
                    let _ =
                        daemon.dispatch_action(ActionCommand::Apps(AppsAction::LaunchSelected));
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "launcher".to_string(),
                    }));
                    self.request_redraw();
                }
                Key::Named(NamedKey::Space) => {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.push(' ');
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SetQuery {
                        query,
                    }));
                    self.request_redraw();
                }
                Key::Character(c) => {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.push_str(c);
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SetQuery {
                        query,
                    }));
                    self.request_redraw();
                }
                Key::Named(NamedKey::Backspace) => {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.pop();
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::SetQuery {
                        query,
                    }));
                    self.request_redraw();
                }
                _ => {}
            }
            return;
        }

        // Alt+Tab or Super+Tab cycles window focus
        if pressed
            && matches!(key_event.logical_key, Key::Named(NamedKey::Tab))
            && (self.modifiers.alt_key() || self.modifiers.super_key())
        {
            let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusDirection {
                dir: wm_proto::Direction::Right,
            }));
            self.request_redraw();
            return;
        }

        // Route input to the focused window (Wayland client or built-in simulated terminal)
        if let Some(focused_id) = daemon.wm.focused_window_id() {
            let is_builtin_term = daemon
                .wm
                .windows()
                .into_iter()
                .find(|w| w.id == focused_id)
                .map(|w| w.app_id == "org.lumina.terminal")
                .unwrap_or(false);

            if is_builtin_term {
                if !pressed {
                    return;
                }
                match &key_event.logical_key {
                    Key::Named(NamedKey::Enter) => {
                        daemon.terminal_execute();
                        self.request_redraw();
                    }
                    Key::Named(NamedKey::Space) => {
                        daemon.terminal_push_str(" ");
                        self.request_redraw();
                    }
                    Key::Character(c) => {
                        daemon.terminal_push_str(c);
                        self.request_redraw();
                    }
                    Key::Named(NamedKey::Backspace) => {
                        daemon.terminal_backspace();
                        self.request_redraw();
                    }
                    _ => {}
                }
                return;
            }

            // Real Wayland client window: forward evdev keycode press/release
            if !key_event.repeat {
                if let PhysicalKey::Code(code) = key_event.physical_key {
                    if let Some(evdev) = keycode_to_evdev(code) {
                        if let Some(ref mut wl) = daemon.wayland_server {
                            wl.send_key(focused_id, evdev, pressed);
                            self.request_redraw();
                        }
                    }
                }
            }
        }
    }
}

impl ApplicationHandler for NestedApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // Request 1920x1080@60Hz mode on parent Wayland compositor (Cage / wlroots DRM) if supported
        let _ = crate::output_config::request_parent_wayland_mode(1920, 1080);

        let in_cage = std::env::var("LUMINA_IN_CAGE").is_ok();
        let window_attrs = WindowAttributes::default()
            .with_title("Lumina WM — Pure-Rust HTML/CSS Wayland Compositor (Nested)")
            .with_inner_size(LogicalSize::new(self.width, self.height))
            .with_maximized(true)
            .with_decorations(!in_cage);

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Failed to create nested window: {e}");
                return;
            }
        };

        let actual_size = window.inner_size();
        if actual_size.width >= 640 && actual_size.height >= 480 {
            self.width = actual_size.width;
            self.height = actual_size.height;
        }
        let scale_factor = window.scale_factor() as f32;

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
        daemon.resize_output(self.width, self.height, scale_factor);

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
                self.width = size.width.max(640);
                self.height = size.height.max(480);
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor() as f32)
                    .unwrap_or(1.0);
                if let Some(daemon) = self.daemon.as_mut() {
                    daemon.resize_output(self.width, self.height, scale);
                }
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
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(daemon) = self.daemon.as_mut() {
                    daemon.resize_output(self.width, self.height, scale_factor as f32);
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                let mut depressed = 0u32;
                if self.modifiers.shift_key() {
                    depressed |= 1;
                }
                if self.modifiers.control_key() {
                    depressed |= 4;
                }
                if self.modifiers.alt_key() {
                    depressed |= 8;
                }
                if self.modifiers.super_key() {
                    depressed |= 64;
                }
                if let Some(daemon) = self.daemon.as_mut() {
                    if let Some(ref mut wl) = daemon.wayland_server {
                        wl.send_modifiers(depressed, 0, 0, 0);
                    }
                }
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
                    if button == MouseButton::Left && self.drag_state.take().is_some() {
                        if let Some(daemon) = self.daemon.as_mut() {
                            daemon.wm.end_interactive_grab();
                            let patches = daemon.wm.export_state_patches();
                            daemon.apply_state_patches(patches);
                        }
                        self.request_redraw();
                    }
                    if let Some((win_id, btn_code)) = self.pressed_client_win.take() {
                        let desktop_pt = self.desktop_cursor();
                        let (dw, dh) = self.desktop_size();
                        if let Some(daemon) = self.daemon.as_mut() {
                            let usable = daemon
                                .wm
                                .outputs()
                                .first()
                                .map(|o| o.usable_rect)
                                .unwrap_or(Rect::new(0.0, 44.0, dw, (dh - 44.0).max(100.0)));
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
                                wl.send_pointer_button_code(win_id, local_x, local_y, btn_code, false);
                            }
                        }
                        self.request_redraw();
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

                        if frame_w == dst_w && frame_h == dst_h {
                            for (dst_px, rgba) in
                                buffer.iter_mut().zip(frame.pixels.chunks_exact(4))
                            {
                                *dst_px = ((rgba[0] as u32) << 16)
                                    | ((rgba[1] as u32) << 8)
                                    | (rgba[2] as u32);
                            }
                        } else {
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
