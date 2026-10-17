use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use memmap2::MmapOptions;
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
    focused_pointer_surface: Option<ObjectId>,
    focused_keyboard_surface: Option<ObjectId>,
    keymap_file: Option<(File, u32)>,
    pub events: Vec<WaylandCompositorEvent>,
}

impl WaylandState {
    fn new() -> Self {
        let keymap_file = create_minimal_xkb_keymap_file();
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
            focused_pointer_surface: None,
            focused_keyboard_surface: None,
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

    /// Spawns a real Linux Wayland client application connected to this compositor's `WAYLAND_DISPLAY`.
    pub fn spawn_client(&mut self, exec_cmd: &str) -> Result<(), String> {
        let trimmed = exec_cmd.trim();
        if trimmed.is_empty() {
            return Err("Empty exec command".to_string());
        }

        let child = Command::new("sh")
            .arg("-c")
            .arg(trimmed)
            // Detach from the host desktop's D-Bus session bus so single-instance GApplication apps
            // (e.g. Nautilus, GNOME Text Editor, Calculator, Decibels) open a real Wayland window
            // inside Lumina WM instead of forwarding activation to the host GNOME session.
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DISPLAY")
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("XDG_SESSION_TYPE", "wayland")
            .env("GDK_BACKEND", "wayland")
            // Force GTK4 / Libadwaita to render into standard wl_shm buffers so our software compositor
            // can map and composite client surfaces with zero GPU DRM/DMABUF dependencies.
            .env("GSK_RENDERER", "cairo")
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .env("QT_QPA_PLATFORM", "wayland")
            .env("SDL_VIDEODRIVER", "wayland")
            .env("MOZ_ENABLE_WAYLAND", "1")
            .env("NO_AT_BRIDGE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn '{trimmed}': {e}"))?;

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

    /// Forwards mouse button press/release inside the window's content area to the Wayland client.
    pub fn send_pointer_button(
        &mut self,
        window_id: u64,
        local_x: f32,
        local_y: f32,
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

        let target_client = wl_surface.client();
        for ptr in &self.state.pointers {
            if ptr.client() == target_client {
                // BTN_LEFT = 0x110 (272)
                ptr.button(serial, time, 0x110, btn_state);
                if ptr.version() >= 5 {
                    ptr.frame();
                }
            }
        }

        // Also give keyboard focus on click
        if pressed && self.state.focused_keyboard_surface.as_ref() != Some(&surf_id) {
            let kb_serial = self.state.next_serial();
            for kb in &self.state.keyboards {
                if kb.client() == target_client {
                    kb.enter(kb_serial, &wl_surface, Vec::new());
                    kb.modifiers(kb_serial, 0, 0, 0, 0);
                }
            }
            self.state.focused_keyboard_surface = Some(surf_id);
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

fn create_minimal_xkb_keymap_file() -> Option<(File, u32)> {
    const KEYMAP_STR: &[u8] = b"xkb_keymap {\n\
xkb_keycodes \"minimal\" { minimum = 8; maximum = 255; <ESC> = 9; <RTRN> = 36; <SPCE> = 65; <BKSP> = 22; };\n\
xkb_types \"minimal\" { virtual_modifiers NumLock; type \"ONE_LEVEL\" { modifiers = none; level_name[Level1] = \"Any\"; }; };\n\
xkb_compatibility \"minimal\" { interpret Any + AnyOf(all) { action = NoAction(); }; };\n\
xkb_symbols \"minimal\" { key <ESC> { [ Escape ] }; key <RTRN> { [ Return ] }; key <SPCE> { [ space ] }; key <BKSP> { [ BackSpace ] }; };\n\
};\0";
    let path = std::env::temp_dir().join(format!("lumina-xkb-keymap-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    let _ = std::fs::remove_file(&path);
    file.write_all(KEYMAP_STR).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    Some((file, KEYMAP_STR.len() as u32))
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
        _state: &mut Self,
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
            1920,
            1080,
            60000,
        );
        if output.version() >= 2 {
            output.scale(1);
            output.done();
        }
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
