use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use ipc_server::{default_socket_path, AssetWatcher, IpcServer};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};
use wm_proto::{
    ActionCommand, AppsAction, Point, Rect, ShellAction, WindowManagerEngine, WmAction,
};
use crate::daemon::LuminaDaemon;

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
    last_frame_time: Instant,
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
            last_frame_time: Instant::now(),
            width: 1600,
            height: 900,
        }
    }

    pub fn run(mut self) -> Result<(), Box<dyn std::error::Error>> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop.run_app(&mut self)?;
        Ok(())
    }

    fn handle_mouse_click(&mut self, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }

        let Some(daemon) = self.daemon.as_mut() else { return; };
        let Point { x, y } = self.cursor_pos;

        // 1. Check if Launcher modal is open (centered 640x440)
        let launcher_visible = daemon
            .widgets
            .get("launcher")
            .map(|w| w.visible)
            .unwrap_or(false);
        if launcher_visible {
            let lx = (self.width as f32 - 640.0) / 2.0;
            let ly = (self.height as f32 - 440.0) / 2.0;
            let modal_rect = Rect::new(lx, ly, 640.0, 440.0);
            if modal_rect.contains(self.cursor_pos) {
                let _ = daemon.click_surface("launcher", x - lx, y - ly);
                return;
            } else {
                // Click outside closes launcher
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "launcher".to_string(),
                }));
                return;
            }
        }

        // 2. Check if Quick Settings modal is open (top-right 380x520)
        let qs_visible = daemon
            .widgets
            .get("quick_settings")
            .map(|w| w.visible)
            .unwrap_or(false);
        if qs_visible {
            let qx = self.width as f32 - 380.0 - 16.0;
            let qy = 54.0;
            let modal_rect = Rect::new(qx, qy, 380.0, 520.0);
            if modal_rect.contains(self.cursor_pos) {
                let _ = daemon.click_surface("quick_settings", x - qx, y - qy);
                return;
            } else {
                // Click outside closes quick settings
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "quick_settings".to_string(),
                }));
                return;
            }
        }

        // 3. Topbar: y < 44
        if y < 44.0 {
            let _ = daemon.click_surface("topbar", x, y);
            return;
        }

        // 4. Bottom Dock: centered at bottom
        let dock_visible = daemon
            .surfaces
            .get("dock")
            .map(|s| s.visible)
            .unwrap_or(true);
        if dock_visible {
            let dock_w = 680.0;
            let dock_h = 58.0;
            let dx = (self.width as f32 - dock_w) / 2.0;
            let dy = self.height as f32 - dock_h - 10.0;
            let dock_rect = Rect::new(dx, dy, dock_w, dock_h);
            if dock_rect.contains(self.cursor_pos) {
                let _ = daemon.click_surface("dock", x - dx, y - dy);
                return;
            }
        }

        // 5. Window Frame / Titlebar buttons
        let usable = Rect::new(0.0, 44.0, self.width as f32, (self.height - 44) as f32);
        let placements = daemon.wm.compute_window_placements(usable);

        for p in placements.iter().rev() {
            if p.frame_rect.contains(self.cursor_pos) {
                // Check titlebar controls: titlebar is top 32px of frame
                if y >= p.frame_rect.y && y <= p.frame_rect.y + 32.0 {
                    let rx = x - p.frame_rect.x;
                    // Button positions in titlebar:
                    // Float: [10..30]
                    // Maximize: [36..56]
                    // Close: [62..82]
                    if rx >= 10.0 && rx <= 30.0 {
                        let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::ToggleFloating {
                            id: Some(p.window_id),
                        }));
                        return;
                    } else if rx >= 36.0 && rx <= 56.0 {
                        let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::ToggleMaximize {
                            id: Some(p.window_id),
                        }));
                        return;
                    } else if rx >= 62.0 && rx <= 82.0 {
                        let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::CloseWindow {
                            id: Some(p.window_id),
                        }));
                        return;
                    }
                }
                // Focus window
                let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWindow {
                    id: p.window_id,
                }));
                return;
            }
        }
    }

    fn handle_key_input(&mut self, key_event: KeyEvent) {
        if key_event.state != ElementState::Pressed {
            return;
        }

        let Some(daemon) = self.daemon.as_mut() else { return; };

        match &key_event.logical_key {
            Key::Named(NamedKey::Super) => {
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::ToggleWidget {
                    name: "launcher".to_string(),
                    args: Default::default(),
                }));
            }
            Key::Named(NamedKey::Escape) => {
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "launcher".to_string(),
                }));
                let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                    name: "quick_settings".to_string(),
                }));
            }
            Key::Named(NamedKey::Tab) => {
                let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusDirection {
                    dir: wm_proto::Direction::Right,
                }));
            }
            Key::Named(NamedKey::Enter) => {
                let launcher_open = daemon
                    .widgets
                    .get("launcher")
                    .map(|w| w.visible)
                    .unwrap_or(false);
                if launcher_open {
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::LaunchSelected));
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "launcher".to_string(),
                    }));
                }
            }
            Key::Character(c) => {
                let launcher_open = daemon
                    .widgets
                    .get("launcher")
                    .map(|w| w.visible)
                    .unwrap_or(false);
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
                }
            }
            Key::Named(NamedKey::Backspace) => {
                let launcher_open = daemon
                    .widgets
                    .get("launcher")
                    .map(|w| w.visible)
                    .unwrap_or(false);
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
                }
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
                    "Nested Lumina WM started!\n- IPC Socket: {:?}\n- To control via CLI: ./target/debug/luminactl --socket {:?} <command>",
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
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Point::new(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    self.handle_mouse_click(button);
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key_input(event);
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Poll IPC requests
                if let (Some(daemon), Some(ipc)) = (self.daemon.as_mut(), self.ipc_server.as_mut()) {
                    let _ = ipc.poll_once(|req| daemon.handle_ipc_request(req));
                }

                // Poll file hot reload
                if let (Some(daemon), Some(watcher)) = (self.daemon.as_mut(), self.watcher.as_mut()) {
                    let _ = daemon.poll_hot_reload(watcher);
                }

                // Tick compositor & WM
                if let Some(daemon) = self.daemon.as_mut() {
                    let patches = daemon.compositor.tick(&mut daemon.wm);
                    daemon.apply_state_patches(patches);
                }

                // Render composite frame and blit to softbuffer
                let (width, height) = (self.width, self.height);
                if let (Some(daemon), Some(surface)) = (self.daemon.as_mut(), self.surface.as_mut()) {
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Redraw at ~60 FPS or on new updates
        let now = Instant::now();
        if now.duration_since(self.last_frame_time).as_millis() >= 16 {
            self.last_frame_time = now;
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }
}
