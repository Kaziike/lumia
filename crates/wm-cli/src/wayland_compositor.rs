use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use memmap2::MmapOptions;
use wayland_protocols::wp::viewporter::server::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::{self, WpViewporter},
};
use wayland_protocols::xdg::decoration::zv1::server::{
    zxdg_decoration_manager_v1::{self, ZxdgDecorationManagerV1},
    zxdg_toplevel_decoration_v1::{self, ZxdgToplevelDecorationV1},
};
use wayland_protocols::xdg::shell::server::{
    xdg_popup::{self, XdgPopup},
    xdg_positioner::{self, XdgPositioner},
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason, ObjectId},
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_callback::{self, WlCallback},
        wl_compositor::{self, WlCompositor},
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::{self, WlDataDeviceManager},
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_keyboard::{self, WlKeyboard},
        wl_output::{self, WlOutput},
        wl_pointer::{self, WlPointer},
        wl_region::{self, WlRegion},
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
        wl_subcompositor::{self, WlSubcompositor},
        wl_subsurface::{self, WlSubsurface},
        wl_surface::{self, WlSurface},
        wl_touch::{self, WlTouch},
    },
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource, WEnum,
};
use wm_proto::FrameBuffer;

/// Events emitted by the embedded `RealWaylandServer` to `LuminaDaemon`.
#[derive(Clone, Debug)]
pub enum WaylandCompositorEvent {
    WindowOpened {
        window_id: u64,
        title: String,
        app_id: String,
    },
    WindowUpdated {
        window_id: u64,
        title: String,
        app_id: String,
    },
    WindowClosed {
        window_id: u64,
    },
    WindowFrameCommitted {
        window_id: u64,
    },
}

struct LuminaClientData;
impl ClientData for LuminaClientData {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[derive(Clone)]
pub struct SurfaceUserData;

#[derive(Clone)]
pub struct XdgSurfaceUserData {
    pub wl_surface_id: ObjectId,
}

#[derive(Clone)]
pub struct XdgToplevelUserData {
    pub wl_surface_id: ObjectId,
    pub window_id: u64,
}

#[derive(Clone)]
pub struct ShmBufferUserData {
    pub pool_id: ObjectId,
    pub offset: usize,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: u32,
}

struct ShmPoolEntry {
    file: File,
    size: usize,
}

struct ShmBufferEntry {
    pool_id: ObjectId,
    offset: usize,
    width: u32,
    height: u32,
    stride: usize,
    format: u32,
    resource: WlBuffer,
}

struct SurfaceEntry {
    wl_surface: WlSurface,
    pending_buffer: Option<ObjectId>,
    pending_callbacks: Vec<WlCallback>,
    committed_frame: Option<Arc<FrameBuffer>>,
    frame_serial: u64,
    window_id: Option<u64>,
    window_geometry: Option<(i32, i32, i32, i32)>,
}

struct ToplevelEntry {
    #[allow(dead_code)]
    window_id: u64,
    wl_surface_id: ObjectId,
    xdg_surface: XdgSurface,
    xdg_toplevel: XdgToplevel,
    title: String,
    app_id: String,
    configured_size: (u32, u32),
    initial_configure_sent: bool,
}

pub struct WaylandState {
    start_time: Instant,
    next_serial: u32,
    next_window_id: u64,
    shm_pools: HashMap<ObjectId, ShmPoolEntry>,
    shm_buffers: HashMap<ObjectId, ShmBufferEntry>,
    surfaces: HashMap<ObjectId, SurfaceEntry>,
    toplevels: HashMap<u64, ToplevelEntry>,
    pointers: Vec<WlPointer>,
    keyboards: Vec<WlKeyboard>,
    pub outputs: Vec<WlOutput>,
    pub output_width: i32,
    pub output_height: i32,
    pub output_scale: i32,
    focused_pointer_surface: Option<ObjectId>,
    focused_keyboard_surface: Option<ObjectId>,
    pub mods_depressed: u32,
    pub mods_latched: u32,
    pub mods_locked: u32,
    pub mods_group: u32,
    keymap_file: Option<(File, u32)>,
    pub events: Vec<WaylandCompositorEvent>,
}

impl WaylandState {
    fn new() -> Self {
        let keymap_file = create_full_xkb_keymap_file();
        Self {
            start_time: Instant::now(),
            next_serial: 1,
            next_window_id: 500,
            shm_pools: HashMap::new(),
            shm_buffers: HashMap::new(),
            surfaces: HashMap::new(),
            toplevels: HashMap::new(),
            pointers: Vec::new(),
            keyboards: Vec::new(),
            outputs: Vec::new(),
            output_width: 1920,
            output_height: 1080,
            output_scale: 1,
            focused_pointer_surface: None,
            focused_keyboard_surface: None,
            mods_depressed: 0,
            mods_latched: 0,
            mods_locked: 0,
            mods_group: 0,
            keymap_file,
            events: Vec::new(),
        }
    }

    fn next_serial(&mut self) -> u32 {
        let s = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        s
    }

    fn elapsed_ms(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    fn focus_surface_keyboard(&mut self, target_surf_id: Option<ObjectId>) {
        if self.focused_keyboard_surface == target_surf_id {
            return;
        }

        self.keyboards.retain(|kb| kb.is_alive());

        if let Some(old_surf_id) = self.focused_keyboard_surface.take() {
            if let Some(old_surf) = self.surfaces.get(&old_surf_id) {
                let old_wl_surf = old_surf.wl_surface.clone();
                let old_client = old_wl_surf.client();
                let leave_serial = self.next_serial();
                for kb in &self.keyboards {
                    if kb.client() == old_client {
                        kb.leave(leave_serial, &old_wl_surf);
                    }
                }
            }
        }

        if let Some(new_surf_id) = target_surf_id {
            if let Some(new_surf) = self.surfaces.get(&new_surf_id) {
                let new_wl_surf = new_surf.wl_surface.clone();
                let new_client = new_wl_surf.client();
                let enter_serial = self.next_serial();
                for kb in &self.keyboards {
                    if kb.client() == new_client {
                        kb.enter(enter_serial, &new_wl_surf, Vec::new());
                        kb.modifiers(
                            enter_serial,
                            self.mods_depressed,
                            self.mods_latched,
                            self.mods_locked,
                            self.mods_group,
                        );
                    }
                }
                self.focused_keyboard_surface = Some(new_surf_id);
            }
        }
    }
}

/// Real embedded Wayland server hosting client applications inside Lumina WM.
pub struct RealWaylandServer {
    display: Display<WaylandState>,
    socket: ListeningSocket,
    socket_name: String,
    pub state: WaylandState,
    children: Vec<Child>,
}

impl RealWaylandServer {
    pub fn new() -> Result<Self, String> {
        let display: Display<WaylandState> =
            Display::new().map_err(|e| format!("Failed to create Wayland Display: {e}"))?;
        let dh = display.handle();

        // Register standard Wayland globals
        dh.create_global::<WaylandState, WlCompositor, ()>(5, ());
        dh.create_global::<WaylandState, WlSubcompositor, ()>(1, ());
        dh.create_global::<WaylandState, WlShm, ()>(1, ());
        dh.create_global::<WaylandState, WlOutput, ()>(4, ());
        dh.create_global::<WaylandState, WlSeat, ()>(7, ());
        dh.create_global::<WaylandState, WlDataDeviceManager, ()>(3, ());
        dh.create_global::<WaylandState, XdgWmBase, ()>(5, ());
        dh.create_global::<WaylandState, ZxdgDecorationManagerV1, ()>(1, ());
        dh.create_global::<WaylandState, WpViewporter, ()>(1, ());

        let socket = ListeningSocket::bind_auto("wayland-lumina", 0..32)
            .map_err(|e| format!("Failed to bind Wayland listening socket: {e}"))?;
        let socket_name = socket
            .socket_name()
            .and_then(|s| s.to_str())
            .unwrap_or("wayland-lumina-0")
            .to_string();

        Ok(Self {
            display,
            socket,
            socket_name,
            state: WaylandState::new(),
            children: Vec::new(),
        })
    }

    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }

    /// Updates the Wayland `wl_output` resolution and scale factor, broadcasting changes to connected clients.
    pub fn set_output_mode(&mut self, width: u32, height: u32, scale: f32) {
        let w = (width as i32).max(320);
        let h = (height as i32).max(200);
        let s = scale.round().max(1.0) as i32;
        if self.state.output_width == w
            && self.state.output_height == h
            && self.state.output_scale == s
        {
            return;
        }
        self.state.output_width = w;
        self.state.output_height = h;
        self.state.output_scale = s;
        self.state.outputs.retain(|o| o.is_alive());
        for output in &self.state.outputs {
            output.mode(
                wl_output::Mode::Current | wl_output::Mode::Preferred,
                w,
                h,
                60000,
            );
            if output.version() >= 2 {
                output.scale(s);
                output.done();
            }
        }
        let _ = self.display.flush_clients();
    }

    /// Spawns a real Linux Wayland client application connected to this compositor's `WAYLAND_DISPLAY`.
    pub fn spawn_client(&mut self, exec_cmd: &str) -> Result<(), String> {
        let trimmed = exec_cmd.trim();
        if trimmed.is_empty() {
            return Err("Empty exec command".to_string());
        }

        let mut cmd_str = if (trimmed.eq_ignore_ascii_case("alacritty")
            || trimmed.eq_ignore_ascii_case("terminal"))
            && !Path::new("/usr/bin/alacritty").exists()
        {
            "kgx".to_string()
        } else if trimmed.eq_ignore_ascii_case("antigravity")
            && Path::new("/home/kass/.local/share/Antigravity-x64/antigravity").exists()
        {
            "/home/kass/.local/share/Antigravity-x64/antigravity".to_string()
        } else {
            trimmed.to_string()
        };

        let lower = cmd_str.to_ascii_lowercase();
        let is_electron = lower.contains("antigravity")
            || lower.contains("code")
            || lower.contains("chromium")
            || lower.contains("electron")
            || lower.contains("brave");

        if is_electron {
            for flag in [
                "--ozone-platform=wayland",
                "--enable-features=UseOzonePlatform",
                "--disable-gpu",
                "--disable-gpu-compositing",
                "--disable-gpu-sandbox",
                "--no-sandbox",
            ] {
                if !cmd_str.contains(flag) {
                    cmd_str.push(' ');
                    cmd_str.push_str(flag);
                }
            }
            if lower.contains("antigravity") && !cmd_str.contains("--user-data-dir") {
                cmd_str.push_str(" --user-data-dir=/tmp/lumina-antigravity-session");
            }
        }

        let in_cage = std::env::var("LUMINA_IN_CAGE").is_ok();
        let has_dbus_run_session = Path::new("/usr/bin/dbus-run-session").exists();

        let mut command = if !in_cage && has_dbus_run_session {
            let mut c = Command::new("/usr/bin/dbus-run-session");
            c.arg("--").arg("sh").arg("-c").arg(&cmd_str);
            c.env_remove("DBUS_SESSION_BUS_ADDRESS");
            c
        } else if in_cage
            && std::env::var("DBUS_SESSION_BUS_ADDRESS").is_err()
            && has_dbus_run_session
        {
            let mut c = Command::new("/usr/bin/dbus-run-session");
            c.arg("--").arg("sh").arg("-c").arg(&cmd_str);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&cmd_str);
            if !in_cage && !is_electron {
                c.env_remove("DBUS_SESSION_BUS_ADDRESS");
            }
            c
        };

        let child = command
            .env_remove("DISPLAY")
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("XDG_SESSION_TYPE", "wayland")
            .env("GDK_BACKEND", "wayland")
            // Force GTK4 / Libadwaita and Mesa / Electron to render into standard wl_shm buffers
            // so our software compositor can map and composite client surfaces cleanly.
            .env("GSK_RENDERER", "cairo")
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .env("GALLIUM_DRIVER", "llvmpipe")
            .env("QT_QPA_PLATFORM", "wayland")
            .env("SDL_VIDEODRIVER", "wayland")
            .env("MOZ_ENABLE_WAYLAND", "1")
            .env("ELECTRON_OZONE_PLATFORM_HINT", "wayland")
            .env("OZONE_PLATFORM", "wayland")
            .env("NO_AT_BRIDGE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn '{cmd_str}': {e}"))?;

        self.children.push(child);
        Ok(())
    }

    /// Accepts new client connections, dispatches incoming Wayland protocol requests, and flushes events.
    pub fn poll(&mut self) -> Vec<WaylandCompositorEvent> {
        while let Ok(Some(stream)) = self.socket.accept() {
            let _ = self
                .display
                .handle()
                .insert_client(stream, Arc::new(LuminaClientData));
        }

        let _ = self.display.dispatch_clients(&mut self.state);
        let _ = self.display.flush_clients();

        // Reap any exited child processes
        self.children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));

        std::mem::take(&mut self.state.events)
    }

    /// Returns the latest committed `FrameBuffer` and its `frame_serial` for a Wayland client window.
    pub fn window_frame(&self, window_id: u64) -> Option<(Arc<FrameBuffer>, u64)> {
        let tl = self.state.toplevels.get(&window_id)?;
        let surf = self.state.surfaces.get(&tl.wl_surface_id)?;
        let fb = surf.committed_frame.as_ref()?.clone();
        Some((fb, surf.frame_serial))
    }

    /// Sends an `xdg_toplevel.configure(width, height, states)` event if the window size changed.
    pub fn configure_window(&mut self, window_id: u64, width: u32, height: u32, is_focused: bool) {
        let target_w = width.max(120);
        let target_h = height.max(80);
        let serial = self.state.next_serial();

        let Some(tl) = self.state.toplevels.get_mut(&window_id) else {
            return;
        };
        if tl.configured_size == (target_w, target_h) && tl.initial_configure_sent {
            return;
        }
        tl.configured_size = (target_w, target_h);
        tl.initial_configure_sent = true;

        let states = encode_toplevel_states(is_focused);
        tl.xdg_toplevel
            .configure(target_w as i32, target_h as i32, states);
        tl.xdg_surface.configure(serial);
        let _ = self.display.flush_clients();
    }

    /// Requests a Wayland client window to close gracefully via `xdg_toplevel.close()`.
    pub fn close_window(&mut self, window_id: u64) {
        if let Some(tl) = self.state.toplevels.get(&window_id) {
            tl.xdg_toplevel.close();
            let _ = self.display.flush_clients();
        }
    }

    /// Transfers Wayland `wl_keyboard` focus (`leave` old surface, `enter` + `modifiers` new surface).
    pub fn focus_window_keyboard(&mut self, window_id: Option<u64>) {
        let target_surf_id = window_id
            .and_then(|wid| self.state.toplevels.get(&wid))
            .map(|tl| tl.wl_surface_id.clone());
        self.state.focus_surface_keyboard(target_surf_id);
        let _ = self.display.flush_clients();
    }

    /// Sends a raw Linux evdev key press/release event to the specified Wayland client window.
    pub fn send_key(&mut self, window_id: u64, evdev_keycode: u32, pressed: bool) {
        self.focus_window_keyboard(Some(window_id));
        let Some(tl) = self.state.toplevels.get(&window_id) else {
            return;
        };
        let surf_id = tl.wl_surface_id.clone();
        let Some(surf) = self.state.surfaces.get(&surf_id) else {
            return;
        };
        let target_client = surf.wl_surface.client();
        let serial = self.state.next_serial();
        let time = self.state.elapsed_ms();
        let key_state = if pressed {
            wl_keyboard::KeyState::Pressed
        } else {
            wl_keyboard::KeyState::Released
        };

        self.state.keyboards.retain(|kb| kb.is_alive());
        for kb in &self.state.keyboards {
            if kb.client() == target_client {
                kb.key(serial, time, evdev_keycode, key_state);
            }
        }
        let _ = self.display.flush_clients();
    }

    /// Updates XKB modifier bitmasks (`Shift=1`, `Lock=2`, `Ctrl=4`, `Alt=8`, `Super=64`) and notifies the focused client.
    pub fn send_modifiers(
        &mut self,
        mods_depressed: u32,
        mods_latched: u32,
        mods_locked: u32,
        group: u32,
    ) {
        self.state.mods_depressed = mods_depressed;
        self.state.mods_latched = mods_latched;
        self.state.mods_locked = mods_locked;
        self.state.mods_group = group;

        let Some(ref surf_id) = self.state.focused_keyboard_surface else {
            return;
        };
        let Some(surf) = self.state.surfaces.get(surf_id) else {
            return;
        };
        let target_client = surf.wl_surface.client();
        let serial = self.state.next_serial();

        self.state.keyboards.retain(|kb| kb.is_alive());
        for kb in &self.state.keyboards {
            if kb.client() == target_client {
                kb.modifiers(serial, mods_depressed, mods_latched, mods_locked, group);
            }
        }
        let _ = self.display.flush_clients();
    }

    /// Forwards pointer motion (`(local_x, local_y)` inside the window's content area) to the Wayland client.
    pub fn send_pointer_motion(&mut self, window_id: u64, local_x: f32, local_y: f32) {
        let Some(tl) = self.state.toplevels.get(&window_id) else {
            return;
        };
        let surf_id = tl.wl_surface_id.clone();
        let Some(surf) = self.state.surfaces.get(&surf_id) else {
            return;
        };
        let wl_surface = surf.wl_surface.clone();
        let (gx, gy) = surf
            .window_geometry
            .map(|(x, y, _, _)| (x as f64, y as f64))
            .unwrap_or((0.0, 0.0));
        let sx = local_x as f64 + gx;
        let sy = local_y as f64 + gy;

        let need_enter = self.state.focused_pointer_surface.as_ref() != Some(&surf_id);
        let serial = if need_enter {
            self.state.next_serial()
        } else {
            0
        };
        let time = self.state.elapsed_ms();

        self.state.pointers.retain(|ptr| ptr.is_alive());
        let target_client = wl_surface.client();
        for ptr in &self.state.pointers {
            if ptr.client() == target_client {
                if need_enter {
                    ptr.enter(serial, &wl_surface, sx, sy);
                }
                ptr.motion(time, sx, sy);
                if ptr.version() >= 5 {
                    ptr.frame();
                }
            }
        }
        self.state.focused_pointer_surface = Some(surf_id);
        let _ = self.display.flush_clients();
    }

    /// Forwards left mouse button press/release (`0x110`) inside the window's content area to the Wayland client.
    pub fn send_pointer_button(
        &mut self,
        window_id: u64,
        local_x: f32,
        local_y: f32,
        pressed: bool,
    ) {
        self.send_pointer_button_code(window_id, local_x, local_y, 0x110, pressed);
    }

    /// Forwards any mouse button press/release (`0x110` Left, `0x111` Right, `0x112` Middle) to the Wayland client.
    pub fn send_pointer_button_code(
        &mut self,
        window_id: u64,
        local_x: f32,
        local_y: f32,
        button_code: u32,
        pressed: bool,
    ) {
        self.send_pointer_motion(window_id, local_x, local_y);
        let Some(tl) = self.state.toplevels.get(&window_id) else {
            return;
        };
        let surf_id = tl.wl_surface_id.clone();
        let Some(surf) = self.state.surfaces.get(&surf_id) else {
            return;
        };
        let wl_surface = surf.wl_surface.clone();
        let serial = self.state.next_serial();
        let time = self.state.elapsed_ms();
        let btn_state = if pressed {
            wl_pointer::ButtonState::Pressed
        } else {
            wl_pointer::ButtonState::Released
        };

        self.state.pointers.retain(|ptr| ptr.is_alive());
        let target_client = wl_surface.client();
        for ptr in &self.state.pointers {
            if ptr.client() == target_client {
                ptr.button(serial, time, button_code, btn_state);
                if ptr.version() >= 5 {
                    ptr.frame();
                }
            }
        }

        // Also give keyboard focus on click
        if pressed {
            self.state.focus_surface_keyboard(Some(surf_id));
        }

        let _ = self.display.flush_clients();
    }

    /// Forwards horizontal (`dx`) and vertical (`dy`) scroll wheel / touchpad axis events to the Wayland client.
    pub fn send_pointer_axis(&mut self, window_id: u64, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let Some(tl) = self.state.toplevels.get(&window_id) else {
            return;
        };
        let surf_id = tl.wl_surface_id.clone();
        let Some(surf) = self.state.surfaces.get(&surf_id) else {
            return;
        };
        let target_client = surf.wl_surface.client();
        let time = self.state.elapsed_ms();

        self.state.pointers.retain(|ptr| ptr.is_alive());
        for ptr in &self.state.pointers {
            if ptr.client() == target_client {
                if dy != 0.0 {
                    ptr.axis(time, wl_pointer::Axis::VerticalScroll, dy as f64);
                }
                if dx != 0.0 {
                    ptr.axis(time, wl_pointer::Axis::HorizontalScroll, dx as f64);
                }
                if ptr.version() >= 5 {
                    ptr.frame();
                }
            }
        }
        let _ = self.display.flush_clients();
    }
}

/// Encodes `xdg_toplevel::State` values (`Maximized = 1`, `Activated = 4`, `TiledLeft..Bottom = 5..8`)
/// as a byte array of native-endian `u32`s so GTK/Qt clients disable oversized CSD shadow margins.
fn encode_toplevel_states(is_focused: bool) -> Vec<u8> {
    let mut states: Vec<u32> = vec![
        1, // Maximized (tells GTK4/Libadwaita to use flat edges without transparent shadow padding)
        5, // TiledLeft
        6, // TiledRight
        7, // TiledTop
        8, // TiledBottom
    ];
    if is_focused {
        states.push(4); // Activated
    }
    let mut bytes = Vec::with_capacity(states.len() * 4);
    for s in states {
        bytes.extend_from_slice(&s.to_ne_bytes());
    }
    bytes
}

fn try_load_system_xkb_keymap() -> Option<Vec<u8>> {
    unsafe {
        let lib_name = std::ffi::CString::new("libxkbcommon.so.0").ok()?;
        let handle = libc::dlopen(lib_name.as_ptr(), libc::RTLD_LAZY);
        if handle.is_null() {
            return None;
        }

        type XkbContextNew = unsafe extern "C" fn(u32) -> *mut libc::c_void;
        type XkbContextUnref = unsafe extern "C" fn(*mut libc::c_void);
        type XkbKeymapNewFromNames =
            unsafe extern "C" fn(*mut libc::c_void, *const libc::c_void, u32) -> *mut libc::c_void;
        type XkbKeymapGetAsString =
            unsafe extern "C" fn(*mut libc::c_void, u32) -> *mut libc::c_char;
        type XkbKeymapUnref = unsafe extern "C" fn(*mut libc::c_void);

        let sym = |name: &[u8]| -> Option<*mut libc::c_void> {
            let ptr = libc::dlsym(handle, name.as_ptr() as *const libc::c_char);
            if ptr.is_null() {
                None
            } else {
                Some(ptr)
            }
        };

        let result = (|| {
            let ctx_new: XkbContextNew = std::mem::transmute(sym(b"xkb_context_new\0")?);
            let ctx_unref: XkbContextUnref = std::mem::transmute(sym(b"xkb_context_unref\0")?);
            let km_new: XkbKeymapNewFromNames =
                std::mem::transmute(sym(b"xkb_keymap_new_from_names\0")?);
            let km_str: XkbKeymapGetAsString =
                std::mem::transmute(sym(b"xkb_keymap_get_as_string\0")?);
            let km_unref: XkbKeymapUnref = std::mem::transmute(sym(b"xkb_keymap_unref\0")?);

            let ctx = ctx_new(0);
            if ctx.is_null() {
                return None;
            }
            let km = km_new(ctx, std::ptr::null(), 0);
            if km.is_null() {
                ctx_unref(ctx);
                return None;
            }
            // XKB_KEYMAP_FORMAT_TEXT_V1 = 1
            let c_str = km_str(km, 1);
            let bytes = if !c_str.is_null() {
                let slice = std::ffi::CStr::from_ptr(c_str)
                    .to_bytes_with_nul()
                    .to_vec();
                libc::free(c_str as *mut libc::c_void);
                Some(slice)
            } else {
                None
            };
            km_unref(km);
            ctx_unref(ctx);
            bytes
        })();

        libc::dlclose(handle);
        result
    }
}

const FALLBACK_FULL_XKB_KEYMAP: &[u8] = b"xkb_keymap {\n\
xkb_keycodes \"evdev_full\" {\n\
    minimum = 8;\n\
    maximum = 255;\n\
    <ESC> = 9;\n\
    <AE01> = 10; <AE02> = 11; <AE03> = 12; <AE04> = 13; <AE05> = 14; <AE06> = 15;\n\
    <AE07> = 16; <AE08> = 17; <AE09> = 18; <AE10> = 19; <AE11> = 20; <AE12> = 21;\n\
    <BKSP> = 22;\n\
    <TAB> = 23;\n\
    <AD01> = 24; <AD02> = 25; <AD03> = 26; <AD04> = 27; <AD05> = 28; <AD06> = 29;\n\
    <AD07> = 30; <AD08> = 31; <AD09> = 32; <AD10> = 33; <AD11> = 34; <AD12> = 35;\n\
    <RTRN> = 36;\n\
    <LCTL> = 37;\n\
    <AC01> = 38; <AC02> = 39; <AC03> = 40; <AC04> = 41; <AC05> = 42; <AC06> = 43;\n\
    <AC07> = 44; <AC08> = 45; <AC09> = 46; <AC10> = 47; <AC11> = 48;\n\
    <TLDE> = 49;\n\
    <LFSH> = 50;\n\
    <BKSL> = 51;\n\
    <AB01> = 52; <AB02> = 53; <AB03> = 54; <AB04> = 55; <AB05> = 56; <AB06> = 57;\n\
    <AB07> = 58; <AB08> = 59; <AB09> = 60; <AB10> = 61;\n\
    <RTSH> = 62;\n\
    <KPMU> = 63;\n\
    <LALT> = 64;\n\
    <SPCE> = 65;\n\
    <CAPS> = 66;\n\
    <FK01> = 67; <FK02> = 68; <FK03> = 69; <FK04> = 70; <FK05> = 71;\n\
    <FK06> = 72; <FK07> = 73; <FK08> = 74; <FK09> = 75; <FK10> = 76;\n\
    <FK11> = 95; <FK12> = 96;\n\
    <RCTL> = 105;\n\
    <RALT> = 108;\n\
    <HOME> = 110; <UP> = 111; <PGUP> = 112; <LEFT> = 113; <RGHT> = 114;\n\
    <END> = 115; <DOWN> = 116; <PGDN> = 117; <INS> = 118; <DELE> = 119;\n\
    <LWIN> = 133; <RWIN> = 134;\n\
};\n\
xkb_types \"complete\" {\n\
    virtual_modifiers NumLock,Alt,Super;\n\
    type \"ONE_LEVEL\" {\n\
        modifiers = none;\n\
        level_name[Level1] = \"Any\";\n\
    };\n\
    type \"TWO_LEVEL\" {\n\
        modifiers = Shift;\n\
        map[Shift] = Level2;\n\
        level_name[Level1] = \"Base\";\n\
        level_name[Level2] = \"Shift\";\n\
    };\n\
    type \"ALPHABETIC\" {\n\
        modifiers = Shift+Lock;\n\
        map[Shift] = Level2;\n\
        map[Lock] = Level2;\n\
        level_name[Level1] = \"Base\";\n\
        level_name[Level2] = \"Caps\";\n\
    };\n\
};\n\
xkb_compatibility \"complete\" {\n\
    virtual_modifiers NumLock,Alt,Super;\n\
    interpret Any + AnyOf(all) { action = NoAction(); };\n\
};\n\
xkb_symbols \"pc_us\" {\n\
    name[group1]=\"English (US)\";\n\
    key <ESC>  { [ Escape ] };\n\
    key <TLDE> { [ grave, asciitilde ] };\n\
    key <AE01> { [ 1, exclam ] };\n\
    key <AE02> { [ 2, at ] };\n\
    key <AE03> { [ 3, numbersign ] };\n\
    key <AE04> { [ 4, dollar ] };\n\
    key <AE05> { [ 5, percent ] };\n\
    key <AE06> { [ 6, asciicircum ] };\n\
    key <AE07> { [ 7, ampersand ] };\n\
    key <AE08> { [ 8, asterisk ] };\n\
    key <AE09> { [ 9, parenleft ] };\n\
    key <AE10> { [ 0, parenright ] };\n\
    key <AE11> { [ minus, underscore ] };\n\
    key <AE12> { [ equal, plus ] };\n\
    key <BKSP> { [ BackSpace, BackSpace ] };\n\
    key <TAB>  { [ Tab, ISO_Left_Tab ] };\n\
    key <AD01> { [ q, Q ] };\n\
    key <AD02> { [ w, W ] };\n\
    key <AD03> { [ e, E ] };\n\
    key <AD04> { [ r, R ] };\n\
    key <AD05> { [ t, T ] };\n\
    key <AD06> { [ y, Y ] };\n\
    key <AD07> { [ u, U ] };\n\
    key <AD08> { [ i, I ] };\n\
    key <AD09> { [ o, O ] };\n\
    key <AD10> { [ p, P ] };\n\
    key <AD11> { [ bracketleft, braceleft ] };\n\
    key <AD12> { [ bracketright, braceright ] };\n\
    key <RTRN> { [ Return ] };\n\
    key <LCTL> { [ Control_L ] };\n\
    key <AC01> { [ a, A ] };\n\
    key <AC02> { [ s, S ] };\n\
    key <AC03> { [ d, D ] };\n\
    key <AC04> { [ f, F ] };\n\
    key <AC05> { [ g, G ] };\n\
    key <AC06> { [ h, H ] };\n\
    key <AC07> { [ j, J ] };\n\
    key <AC08> { [ k, K ] };\n\
    key <AC09> { [ l, L ] };\n\
    key <AC10> { [ semicolon, colon ] };\n\
    key <AC11> { [ apostrophe, quotedbl ] };\n\
    key <LFSH> { [ Shift_L ] };\n\
    key <BKSL> { [ backslash, bar ] };\n\
    key <AB01> { [ z, Z ] };\n\
    key <AB02> { [ x, X ] };\n\
    key <AB03> { [ c, C ] };\n\
    key <AB04> { [ v, V ] };\n\
    key <AB05> { [ b, B ] };\n\
    key <AB06> { [ n, N ] };\n\
    key <AB07> { [ m, M ] };\n\
    key <AB08> { [ comma, less ] };\n\
    key <AB09> { [ period, greater ] };\n\
    key <AB10> { [ slash, question ] };\n\
    key <RTSH> { [ Shift_R ] };\n\
    key <KPMU> { [ KP_Multiply ] };\n\
    key <LALT> { [ Alt_L, Meta_L ] };\n\
    key <SPCE> { [ space ] };\n\
    key <CAPS> { [ Caps_Lock ] };\n\
    key <FK01> { [ F1 ] };\n\
    key <FK02> { [ F2 ] };\n\
    key <FK03> { [ F3 ] };\n\
    key <FK04> { [ F4 ] };\n\
    key <FK05> { [ F5 ] };\n\
    key <FK06> { [ F6 ] };\n\
    key <FK07> { [ F7 ] };\n\
    key <FK08> { [ F8 ] };\n\
    key <FK09> { [ F9 ] };\n\
    key <FK10> { [ F10 ] };\n\
    key <FK11> { [ F11 ] };\n\
    key <FK12> { [ F12 ] };\n\
    key <RCTL> { [ Control_R ] };\n\
    key <RALT> { [ Alt_R, Meta_R ] };\n\
    key <HOME> { [ Home ] };\n\
    key <UP>   { [ Up ] };\n\
    key <PGUP> { [ Prior ] };\n\
    key <LEFT> { [ Left ] };\n\
    key <RGHT> { [ Right ] };\n\
    key <END>  { [ End ] };\n\
    key <DOWN> { [ Down ] };\n\
    key <PGDN> { [ Next ] };\n\
    key <INS>  { [ Insert ] };\n\
    key <DELE> { [ Delete ] };\n\
    key <LWIN> { [ Super_L ] };\n\
    key <RWIN> { [ Super_R ] };\n\
    modifier_map Shift   { <LFSH>, <RTSH> };\n\
    modifier_map Lock    { <CAPS> };\n\
    modifier_map Control { <LCTL>, <RCTL> };\n\
    modifier_map Mod1    { <LALT>, <RALT> };\n\
    modifier_map Mod4    { <LWIN>, <RWIN> };\n\
};\n\
};\0";

fn create_full_xkb_keymap_file() -> Option<(File, u32)> {
    let keymap_bytes =
        try_load_system_xkb_keymap().unwrap_or_else(|| FALLBACK_FULL_XKB_KEYMAP.to_vec());
    let path = std::env::temp_dir().join(format!("lumina-xkb-keymap-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    let _ = std::fs::remove_file(&path);
    file.write_all(&keymap_bytes).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    Some((file, keymap_bytes.len() as u32))
}

// ============================================================================
// Wayland Protocol GlobalDispatch & Dispatch Implementations
// ============================================================================

impl GlobalDispatch<WlCompositor, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlCompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WlCompositor, ()> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &WlCompositor,
        request: wl_compositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                let surface = data_init.init(id, SurfaceUserData);
                let surf_id = surface.id();
                state.surfaces.insert(
                    surf_id,
                    SurfaceEntry {
                        wl_surface: surface,
                        pending_buffer: None,
                        pending_callbacks: Vec::new(),
                        committed_frame: None,
                        frame_serial: 0,
                        window_id: None,
                        window_geometry: None,
                    },
                );
            }
            wl_compositor::Request::CreateRegion { id } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegion, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlRegion,
        _request: wl_region::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WlSurface, SurfaceUserData> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WlSurface,
        request: wl_surface::Request,
        _data: &SurfaceUserData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let surf_id = resource.id();
        match request {
            wl_surface::Request::Attach { buffer, .. } => {
                if let Some(surf) = state.surfaces.get_mut(&surf_id) {
                    surf.pending_buffer = buffer.map(|b| b.id());
                }
            }
            wl_surface::Request::Frame { callback } => {
                let cb = data_init.init(callback, ());
                if let Some(surf) = state.surfaces.get_mut(&surf_id) {
                    surf.pending_callbacks.push(cb);
                }
            }
            wl_surface::Request::Commit => {
                let elapsed = state.elapsed_ms();
                let (pending_buf_id, callbacks, win_id, geom) = {
                    let Some(surf) = state.surfaces.get_mut(&surf_id) else {
                        return;
                    };
                    (
                        surf.pending_buffer.take(),
                        std::mem::take(&mut surf.pending_callbacks),
                        surf.window_id,
                        surf.window_geometry,
                    )
                };

                // Fire frame callbacks so the client can keep animating
                for cb in callbacks {
                    cb.done(elapsed);
                }

                // If a shared-memory buffer was attached, read its pixels and release the buffer
                if let Some(buf_id) = pending_buf_id {
                    if let Some(extracted) = extract_shm_framebuffer(state, &buf_id, geom) {
                        if let Some(surf) = state.surfaces.get_mut(&surf_id) {
                            surf.frame_serial += 1;
                            surf.committed_frame = Some(Arc::new(extracted));
                        }
                        if let Some(w_id) = win_id {
                            state
                                .events
                                .push(WaylandCompositorEvent::WindowFrameCommitted {
                                    window_id: w_id,
                                });
                        }
                    }
                    if let Some(buf_entry) = state.shm_buffers.get(&buf_id) {
                        buf_entry.resource.release();
                    }
                }

                // If this surface is a toplevel and hasn't received its initial configure, send it now
                if let Some(w_id) = win_id {
                    let need_init_cfg = state
                        .toplevels
                        .get(&w_id)
                        .map(|t| !t.initial_configure_sent)
                        .unwrap_or(false);
                    if need_init_cfg {
                        let serial = state.next_serial();
                        if let Some(tl) = state.toplevels.get_mut(&w_id) {
                            tl.initial_configure_sent = true;
                            let (w, h) = tl.configured_size;
                            tl.xdg_toplevel
                                .configure(w as i32, h as i32, encode_toplevel_states(true));
                            tl.xdg_surface.configure(serial);
                        }
                    }
                }
            }
            wl_surface::Request::Destroy => {
                state.surfaces.remove(&surf_id);
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &WlSurface,
        _data: &SurfaceUserData,
    ) {
        state.surfaces.remove(&resource.id());
    }
}

fn extract_shm_framebuffer(
    state: &WaylandState,
    buf_id: &ObjectId,
    geom: Option<(i32, i32, i32, i32)>,
) -> Option<FrameBuffer> {
    let buf = state.shm_buffers.get(buf_id)?;
    let pool = state.shm_pools.get(&buf.pool_id)?;

    if buf.width == 0 || buf.height == 0 || buf.stride < (buf.width as usize * 4) {
        return None;
    }
    let required_end = buf
        .offset
        .checked_add(buf.stride.checked_mul(buf.height as usize)?)?;
    let file_len = pool.file.metadata().map(|m| m.len() as usize).unwrap_or(pool.size);
    let map_len = file_len.max(pool.size);
    if required_end > map_len {
        return None;
    }

    let mmap = unsafe { MmapOptions::new().len(required_end).map(&pool.file).ok()? };
    let src_data = &mmap[buf.offset..required_end];

    let is_opaque = buf.format == 1; // WL_SHM_FORMAT_XRGB8888 == 1, ARGB8888 == 0

    // Determine crop sub-rectangle if the client specified `xdg_surface.set_window_geometry`
    let (crop_x, crop_y, crop_w, crop_h) = if let Some((gx, gy, gw, gh)) = geom {
        let cx = gx.max(0) as u32;
        let cy = gy.max(0) as u32;
        let cw = (gw.max(1) as u32).min(buf.width.saturating_sub(cx));
        let ch = (gh.max(1) as u32).min(buf.height.saturating_sub(cy));
        if cw > 0 && ch > 0 {
            (cx, cy, cw, ch)
        } else {
            (0, 0, buf.width, buf.height)
        }
    } else {
        (0, 0, buf.width, buf.height)
    };

    let mut fb = FrameBuffer::new(crop_w, crop_h);
    let dst_stride = (crop_w as usize) * 4;

    for row in 0..(crop_h as usize) {
        let src_row_start = (crop_y as usize + row) * buf.stride + (crop_x as usize) * 4;
        let src_row = &src_data[src_row_start..src_row_start + dst_stride];
        let dst_row_start = row * dst_stride;
        let dst_row = &mut fb.pixels[dst_row_start..dst_row_start + dst_stride];

        for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
            let b = src_px[0];
            let g = src_px[1];
            let r = src_px[2];
            let a = if is_opaque { 255 } else { src_px[3] };
            dst_px[0] = r;
            dst_px[1] = g;
            dst_px[2] = b;
            dst_px[3] = a;
        }
    }

    Some(fb)
}

impl Dispatch<WlCallback, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlCallback,
        _request: wl_callback::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

// ============================================================================
// WlSubcompositor & WlSubsurface
// ============================================================================

impl GlobalDispatch<WlSubcompositor, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlSubcompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WlSubcompositor, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlSubcompositor,
        request: wl_subcompositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_subcompositor::Request::GetSubsurface { id, .. } = request {
            data_init.init(id, ());
        }
    }
}

impl Dispatch<WlSubsurface, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlSubsurface,
        _request: wl_subsurface::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

// ============================================================================
// WlShm, WlShmPool & WlBuffer
// ============================================================================

impl GlobalDispatch<WlShm, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlShm>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let shm = data_init.init(resource, ());
        shm.format(wl_shm::Format::Argb8888);
        shm.format(wl_shm::Format::Xrgb8888);
    }
}

impl Dispatch<WlShm, ()> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &WlShm,
        request: wl_shm::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, size } = request {
            let pool = data_init.init(id, ());
            state.shm_pools.insert(
                pool.id(),
                ShmPoolEntry {
                    file: File::from(fd),
                    size: size.max(0) as usize,
                },
            );
        }
    }
}

impl Dispatch<WlShmPool, ()> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WlShmPool,
        request: wl_shm_pool::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let pool_id = resource.id();
        match request {
            wl_shm_pool::Request::CreateBuffer {
                id,
                offset,
                width,
                height,
                stride,
                format,
            } => {
                let raw_fmt = match format {
                    WEnum::Value(wl_shm::Format::Argb8888) => 0,
                    WEnum::Value(wl_shm::Format::Xrgb8888) => 1,
                    WEnum::Value(other) => other as u32,
                    WEnum::Unknown(u) => u,
                };
                let udata = ShmBufferUserData {
                    pool_id: pool_id.clone(),
                    offset: offset.max(0) as usize,
                    width: width.max(0) as u32,
                    height: height.max(0) as u32,
                    stride: stride.max(0) as usize,
                    format: raw_fmt,
                };
                let buffer = data_init.init(id, udata.clone());
                state.shm_buffers.insert(
                    buffer.id(),
                    ShmBufferEntry {
                        pool_id,
                        offset: udata.offset,
                        width: udata.width,
                        height: udata.height,
                        stride: udata.stride,
                        format: udata.format,
                        resource: buffer,
                    },
                );
            }
            wl_shm_pool::Request::Resize { size } => {
                if let Some(pool) = state.shm_pools.get_mut(&pool_id) {
                    pool.size = size.max(0) as usize;
                }
            }
            wl_shm_pool::Request::Destroy => {
                state.shm_pools.remove(&pool_id);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, ShmBufferUserData> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WlBuffer,
        request: wl_buffer::Request,
        _data: &ShmBufferUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_buffer::Request::Destroy = request {
            state.shm_buffers.remove(&resource.id());
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &WlBuffer,
        _data: &ShmBufferUserData,
    ) {
        state.shm_buffers.remove(&resource.id());
    }
}

// ============================================================================
// WlOutput, WlSeat, WlPointer, WlKeyboard, WlTouch, WlDataDeviceManager
// ============================================================================

impl GlobalDispatch<WlOutput, ()> for WaylandState {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlOutput>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let output = data_init.init(resource, ());
        output.geometry(
            0,
            0,
            340,
            190,
            wl_output::Subpixel::Unknown,
            "Lumina".to_string(),
            "VirtualOutput-1".to_string(),
            wl_output::Transform::Normal,
        );
        output.mode(
            wl_output::Mode::Current | wl_output::Mode::Preferred,
            state.output_width,
            state.output_height,
            60000,
        );
        if output.version() >= 2 {
            output.scale(state.output_scale);
            output.done();
        }
        state.outputs.push(output);
    }
}

impl Dispatch<WlOutput, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlOutput,
        _request: wl_output::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlSeat, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlSeat>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let seat = data_init.init(resource, ());
        seat.capabilities(wl_seat::Capability::Pointer | wl_seat::Capability::Keyboard);
        if seat.version() >= 2 {
            seat.name("seat0".to_string());
        }
    }
}

impl Dispatch<WlSeat, ()> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &WlSeat,
        request: wl_seat::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_seat::Request::GetPointer { id } => {
                let ptr = data_init.init(id, ());
                state.pointers.push(ptr);
            }
            wl_seat::Request::GetKeyboard { id } => {
                let kb = data_init.init(id, ());
                if let Some((ref file, size)) = state.keymap_file {
                    kb.keymap(wl_keyboard::KeymapFormat::XkbV1, file.as_fd(), size);
                }
                if kb.version() >= 4 {
                    kb.repeat_info(25, 400);
                }
                // If the currently focused keyboard surface belongs to this client, send enter immediately
                let focused_wl_surf = state
                    .focused_keyboard_surface
                    .as_ref()
                    .and_then(|sid| state.surfaces.get(sid))
                    .map(|s| s.wl_surface.clone());
                if let Some(wl_surf) = focused_wl_surf {
                    if wl_surf.client() == kb.client() {
                        let serial = state.next_serial();
                        kb.enter(serial, &wl_surf, Vec::new());
                        kb.modifiers(
                            serial,
                            state.mods_depressed,
                            state.mods_latched,
                            state.mods_locked,
                            state.mods_group,
                        );
                    }
                }
                state.keyboards.push(kb);
            }
            wl_seat::Request::GetTouch { id } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlPointer,
        _request: wl_pointer::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WlKeyboard, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlKeyboard,
        _request: wl_keyboard::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WlTouch, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlTouch,
        _request: wl_touch::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlDataDeviceManager, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlDataDeviceManager>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WlDataDeviceManager, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                data_init.init(id, ());
            }
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlDataSource,
        _request: wl_data_source::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlDataDevice,
        _request: wl_data_device::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WlDataOffer, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlDataOffer,
        _request: wl_data_offer::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

// ============================================================================
// XdgWmBase, XdgSurface, XdgToplevel, XdgPopup, XdgPositioner, ZxdgDecoration
// ============================================================================

impl GlobalDispatch<XdgWmBase, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XdgWmBase>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<XdgWmBase, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &XdgWmBase,
        request: xdg_wm_base::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_wm_base::Request::CreatePositioner { id } => {
                data_init.init(id, ());
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                data_init.init(
                    id,
                    XdgSurfaceUserData {
                        wl_surface_id: surface.id(),
                    },
                );
            }
            xdg_wm_base::Request::Pong { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<XdgPositioner, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &XdgPositioner,
        _request: xdg_positioner::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<XdgSurface, XdgSurfaceUserData> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &XdgSurface,
        request: xdg_surface::Request,
        data: &XdgSurfaceUserData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_surface::Request::GetToplevel { id } => {
                let win_id = state.next_window_id;
                state.next_window_id += 1;

                let toplevel = data_init.init(
                    id,
                    XdgToplevelUserData {
                        wl_surface_id: data.wl_surface_id.clone(),
                        window_id: win_id,
                    },
                );

                if let Some(surf) = state.surfaces.get_mut(&data.wl_surface_id) {
                    surf.window_id = Some(win_id);
                }

                let default_title = format!("Wayland App #{win_id}");
                let default_app_id = "wayland.client".to_string();

                state.toplevels.insert(
                    win_id,
                    ToplevelEntry {
                        window_id: win_id,
                        wl_surface_id: data.wl_surface_id.clone(),
                        xdg_surface: resource.clone(),
                        xdg_toplevel: toplevel.clone(),
                        title: default_title.clone(),
                        app_id: default_app_id.clone(),
                        configured_size: (900, 620),
                        initial_configure_sent: true,
                    },
                );

                // Send initial configure immediately so client starts drawing
                let serial = state.next_serial();
                toplevel.configure(900, 620, encode_toplevel_states(true));
                resource.configure(serial);

                // Automatically focus the newly created toplevel's surface for keyboard input
                state.focus_surface_keyboard(Some(data.wl_surface_id.clone()));

                state.events.push(WaylandCompositorEvent::WindowOpened {
                    window_id: win_id,
                    title: default_title,
                    app_id: default_app_id,
                });
            }
            xdg_surface::Request::GetPopup { id, .. } => {
                let popup = data_init.init(id, ());
                let serial = state.next_serial();
                popup.configure(0, 0, 240, 200);
                resource.configure(serial);
            }
            xdg_surface::Request::SetWindowGeometry {
                x,
                y,
                width,
                height,
            } => {
                if let Some(surf) = state.surfaces.get_mut(&data.wl_surface_id) {
                    surf.window_geometry = Some((x, y, width, height));
                }
            }
            xdg_surface::Request::AckConfigure { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<XdgPopup, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &XdgPopup,
        _request: xdg_popup::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<XdgToplevel, XdgToplevelUserData> for WaylandState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &XdgToplevel,
        request: xdg_toplevel::Request,
        data: &XdgToplevelUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let win_id = data.window_id;
        match request {
            xdg_toplevel::Request::SetTitle { title } => {
                if let Some(tl) = state.toplevels.get_mut(&win_id) {
                    if !title.trim().is_empty() {
                        tl.title = title;
                    }
                    state.events.push(WaylandCompositorEvent::WindowUpdated {
                        window_id: win_id,
                        title: tl.title.clone(),
                        app_id: tl.app_id.clone(),
                    });
                }
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                if let Some(tl) = state.toplevels.get_mut(&win_id) {
                    if !app_id.trim().is_empty() {
                        tl.app_id = app_id.clone();
                        if tl.title.starts_with("Wayland App #") {
                            tl.title = app_id;
                        }
                    }
                    state.events.push(WaylandCompositorEvent::WindowUpdated {
                        window_id: win_id,
                        title: tl.title.clone(),
                        app_id: tl.app_id.clone(),
                    });
                }
            }
            xdg_toplevel::Request::Destroy => {
                if state.toplevels.remove(&win_id).is_some() {
                    state
                        .events
                        .push(WaylandCompositorEvent::WindowClosed { window_id: win_id });
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &XdgToplevel,
        data: &XdgToplevelUserData,
    ) {
        if state.toplevels.remove(&data.window_id).is_some() {
            state.events.push(WaylandCompositorEvent::WindowClosed {
                window_id: data.window_id,
            });
        }
    }
}

impl GlobalDispatch<ZxdgDecorationManagerV1, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZxdgDecorationManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZxdgDecorationManagerV1, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id, .. } = request {
            let dec = data_init.init(id, ());
            dec.configure(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        resource: &ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Request::SetMode { .. }
        | zxdg_toplevel_decoration_v1::Request::UnsetMode = request
        {
            resource.configure(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        }
    }
}

// ============================================================================
// WpViewporter & WpViewport
// ============================================================================

impl GlobalDispatch<WpViewporter, ()> for WaylandState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WpViewporter>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WpViewporter, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WpViewporter,
        request: wp_viewporter::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_viewporter::Request::GetViewport { id, .. } = request {
            data_init.init(id, ());
        }
    }
}

impl Dispatch<WpViewport, ()> for WaylandState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WpViewport,
        _request: wp_viewport::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

