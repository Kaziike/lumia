use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ipc_server::{default_socket_path, AssetWatcher, IpcServer};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};
use wm_proto::{ActionCommand, Point};

use crate::daemon::LuminaDaemon;
use crate::input_router::{InputRouter, KeyInput, KeySymbol, Modifiers, PointerButton};

fn winit_button_to_pointer_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Right => Some(PointerButton::Right),
        MouseButton::Middle => Some(PointerButton::Middle),
        _ => None,
    }
}

fn winit_mods_to_router_mods(mods: ModifiersState) -> Modifiers {
    Modifiers {
        shift: mods.shift_key(),
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        logo: mods.super_key(),
    }
}

fn keycode_to_digit(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::Digit1 => Some(1),
        KeyCode::Digit2 => Some(2),
        KeyCode::Digit3 => Some(3),
        KeyCode::Digit4 => Some(4),
        KeyCode::Digit5 => Some(5),
        KeyCode::Digit6 => Some(6),
        KeyCode::Digit7 => Some(7),
        KeyCode::Digit8 => Some(8),
        KeyCode::Digit9 => Some(9),
        _ => None,
    }
}

fn winit_logical_key_to_symbol(key: &Key) -> KeySymbol {
    match key {
        Key::Named(NamedKey::Enter) => KeySymbol::Enter,
        Key::Named(NamedKey::Escape) => KeySymbol::Escape,
        Key::Named(NamedKey::Space) => KeySymbol::Space,
        Key::Named(NamedKey::Tab) => KeySymbol::Tab,
        Key::Named(NamedKey::Backspace) => KeySymbol::Backspace,
        Key::Named(NamedKey::Super) => KeySymbol::Super,
        Key::Named(NamedKey::ArrowLeft) => KeySymbol::ArrowLeft,
        Key::Named(NamedKey::ArrowRight) => KeySymbol::ArrowRight,
        Key::Named(NamedKey::ArrowUp) => KeySymbol::ArrowUp,
        Key::Named(NamedKey::ArrowDown) => KeySymbol::ArrowDown,
        Key::Named(NamedKey::F1) => KeySymbol::F(1),
        Key::Named(NamedKey::F2) => KeySymbol::F(2),
        Key::Named(NamedKey::F3) => KeySymbol::F(3),
        Key::Named(NamedKey::F4) => KeySymbol::F(4),
        Key::Named(NamedKey::F5) => KeySymbol::F(5),
        Key::Named(NamedKey::F6) => KeySymbol::F(6),
        Key::Named(NamedKey::F7) => KeySymbol::F(7),
        Key::Named(NamedKey::F8) => KeySymbol::F(8),
        Key::Named(NamedKey::F9) => KeySymbol::F(9),
        Key::Named(NamedKey::F10) => KeySymbol::F(10),
        Key::Named(NamedKey::F11) => KeySymbol::F(11),
        Key::Named(NamedKey::F12) => KeySymbol::F(12),
        Key::Character(c) => KeySymbol::Char(c.to_string()),
        _ => KeySymbol::Other,
    }
}

pub fn keycode_to_evdev(code: KeyCode) -> Option<u32> {
    match code {
        KeyCode::Escape => Some(1),
        KeyCode::Digit1 => Some(2),
        KeyCode::Digit2 => Some(3),
        KeyCode::Digit3 => Some(3 + 1),
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
    input_router: InputRouter,
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    ipc_server: Option<IpcServer>,
    watcher: Option<AssetWatcher>,
    cursor_pos: Point,
    cursor_dirty: bool,
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
            input_router: InputRouter::new(),
            window: None,
            context: None,
            surface: None,
            ipc_server: None,
            watcher: None,
            cursor_pos: Point::new(0.0, 0.0),
            cursor_dirty: false,
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

    /// Coalesces high-frequency Wayland mouse motion events through `InputRouter`.
    fn process_pending_cursor_move(&mut self) -> bool {
        if !self.cursor_dirty {
            return false;
        }
        self.cursor_dirty = false;
        let desktop_pt = self.desktop_cursor();
        let Some(daemon) = self.daemon.as_mut() else {
            return false;
        };
        let outcome = self.input_router.on_pointer_motion(daemon, desktop_pt);
        InputRouter::apply_forwards_to_legacy_server(daemon, &outcome.client_forwards);
        outcome.redraw
    }

    fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        let Some(ptr_btn) = winit_button_to_pointer_button(button) else {
            return;
        };
        let desktop_pt = self.desktop_cursor();
        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };
        let outcome = self
            .input_router
            .on_pointer_button(daemon, desktop_pt, ptr_btn, pressed);
        InputRouter::apply_forwards_to_legacy_server(daemon, &outcome.client_forwards);
        if outcome.redraw {
            self.request_redraw();
        }
    }

    fn handle_mouse_scroll(&mut self, delta: MouseScrollDelta) {
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * 15.0, y * 15.0),
            MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
        };
        let desktop_pt = self.desktop_cursor();
        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };
        let outcome = self.input_router.on_pointer_axis(daemon, desktop_pt, dx, dy);
        InputRouter::apply_forwards_to_legacy_server(daemon, &outcome.client_forwards);
        if outcome.redraw {
            self.request_redraw();
        }
    }

    fn handle_key_input(&mut self, key_event: KeyEvent) {
        let pressed = key_event.state == ElementState::Pressed;
        let (digit_code, evdev_code) = match key_event.physical_key {
            PhysicalKey::Code(code) => (keycode_to_digit(code), keycode_to_evdev(code)),
            _ => (None, None),
        };
        let key_input = KeyInput {
            sym: winit_logical_key_to_symbol(&key_event.logical_key),
            digit_code,
            evdev_code,
            pressed,
            repeat: key_event.repeat,
            mods: winit_mods_to_router_mods(self.modifiers),
        };
        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };
        let outcome = self.input_router.on_key_input(daemon, key_input);
        InputRouter::apply_forwards_to_legacy_server(daemon, &outcome.client_forwards);
        if outcome.redraw {
            self.request_redraw();
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
        daemon.clear_startup_windows();
        match daemon.enable_wayland_server() {
            Ok(sock_name) => {
                println!("Embedded Wayland Compositor active on WAYLAND_DISPLAY={sock_name}");
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
                eprintln!(
                    "Warning: could not bind IPC socket at {:?}: {e}",
                    self.socket_path
                );
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
                    if let (Some(w), Some(h)) =
                        (NonZeroU32::new(self.width), NonZeroU32::new(self.height))
                    {
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
                let router_mods = winit_mods_to_router_mods(self.modifiers);
                self.input_router.set_modifiers(router_mods);
                let depressed = router_mods.to_xkb_depressed_mask();
                if let Some(daemon) = self.daemon.as_mut() {
                    if let Some(ref mut wl) = daemon.wayland_server {
                        wl.send_modifiers(depressed, 0, 0, 0);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Point::new(position.x as f32, position.y as f32);
                self.cursor_dirty = true;
                if self.last_redraw.elapsed() >= Duration::from_millis(14) {
                    if self.input_router.active_grab().is_some()
                        || self.process_pending_cursor_move()
                    {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                self.handle_mouse_button(button, pressed);
                if self.daemon.as_ref().is_some_and(|d| {
                    d.get_state_path(Some("sys.session.exit_requested"))
                        .as_bool()
                }) {
                    event_loop.exit();
                    return;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.handle_mouse_scroll(delta);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key_input(event);
                if self.daemon.as_ref().is_some_and(|d| {
                    d.get_state_path(Some("sys.session.exit_requested"))
                        .as_bool()
                }) {
                    event_loop.exit();
                    return;
                }
            }
            WindowEvent::RedrawRequested => {
                self.needs_redraw = false;
                self.last_redraw = Instant::now();
                let _ = self.process_pending_cursor_move();

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

        if self.cursor_dirty && self.last_redraw.elapsed() >= Duration::from_millis(14) {
            if self.process_pending_cursor_move() {
                dirty = true;
            }
        }

        if let Some(daemon) = self.daemon.as_mut() {
            if daemon.poll_wayland_clients() {
                dirty = true;
            }
        }

        if let (Some(daemon), Some(ipc)) = (self.daemon.as_mut(), self.ipc_server.as_mut()) {
            if let Ok(count) = ipc.poll_once(|req| daemon.handle_ipc_request(req)) {
                if count > 0 {
                    dirty = true;
                }
            }
        }

        if let (Some(daemon), Some(watcher)) = (self.daemon.as_mut(), self.watcher.as_mut()) {
            let reloaded = daemon.poll_hot_reload(watcher);
            if !reloaded.is_empty() {
                dirty = true;
            }
        }

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

        if self.daemon.as_ref().is_some_and(|d| {
            d.get_state_path(Some("sys.session.exit_requested"))
                .as_bool()
        }) {
            event_loop.exit();
            return;
        }

        if dirty {
            self.request_redraw();
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
        ));
    }
}
