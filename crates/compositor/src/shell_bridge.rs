use std::collections::HashMap;

use ipc_server::AssetWatcher;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                Kind,
            },
            ImportMem, Renderer,
        },
    },
    utils::{Physical, Point as SmithayPoint, Size as SmithaySize, Transform},
};
use wm_core::LayerTier;
use wm_proto::{FrameBuffer, IpcRequest, IpcResponse, Point, Rect, ResizeEdge, WindowPlacement};

/// Metadata and placement for a window managed by `LuminaShell`.
#[derive(Clone, Debug)]
pub struct ManagedWindowPlacement {
    pub placement: WindowPlacement,
    pub title: String,
    pub app_id: String,
    pub is_urgent: bool,
    pub is_maximized: bool,
}

/// A rendered HTML/CSS shell layer frame ready for GPU upload.
#[derive(Clone, Debug)]
pub struct ShellLayerRenderItem {
    pub name: String,
    pub tier: LayerTier,
    pub rect: Rect,
    pub frame: FrameBuffer,
    pub serial: u64,
}

/// Backend-agnostic pointer button passed to `ShellHost`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellPointerButton {
    Left,
    Right,
    Middle,
    Other(u32),
}

/// Backend-agnostic modifier state passed to `ShellHost`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

/// Backend-agnostic logical key symbol passed to `ShellHost`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellKeySymbol {
    Enter,
    Escape,
    Space,
    Tab,
    Backspace,
    Super,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    F(u8),
    Digit(u8),
    Char(String),
    Other,
}

/// Normalized keyboard event passed to `ShellHost`.
#[derive(Clone, Debug)]
pub struct ShellKeyInput {
    pub sym: ShellKeySymbol,
    pub digit_code: Option<u8>,
    pub evdev_code: Option<u32>,
    pub pressed: bool,
    pub repeat: bool,
    pub mods: ShellModifiers,
}

/// Interactive window move/resize grab descriptor initiated by `ShellHost`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShellGrabKind {
    Move {
        window_id: u64,
        start_pt: Point,
    },
    Resize {
        window_id: u64,
        edge: ResizeEdge,
        start_pt: Point,
    },
}

/// Outcome returned by `ShellHost` after routing an input event.
#[derive(Clone, Debug, Default)]
pub struct ShellInputOutcome {
    pub consumed_by_shell: bool,
    pub redraw: bool,
    pub exit_requested: bool,
    pub focused_window: Option<u64>,
    pub started_grab: Option<ShellGrabKind>,
    pub ended_grab: bool,
}

/// Bridge trait implemented by `LuminaShell` (`LuminaDaemon`) so `crates/compositor`
/// can drive the HTML/CSS shell and `wm-core` layout engine without circular crate dependencies.
pub trait ShellHost {
    fn output_size(&self) -> (u32, u32);
    fn usable_rect(&self) -> Rect;
    fn any_modal_open(&self) -> bool;
    fn resize_output(&mut self, width: u32, height: u32, scale: f32);

    fn on_client_window_opened(&mut self, window_id: u64, title: String, app_id: String);
    fn on_client_window_updated(&mut self, window_id: u64, title: String, app_id: String);
    fn on_client_window_closed(&mut self, window_id: u64);

    fn focused_window_id(&self) -> Option<u64>;
    fn compute_window_placements(&self) -> Vec<ManagedWindowPlacement>;

    fn wallpaper_with_serial(&mut self) -> (FrameBuffer, u64);
    fn visible_shell_layers(&mut self) -> Vec<ShellLayerRenderItem>;
    fn render_window_decoration(
        &mut self,
        info: &ManagedWindowPlacement,
    ) -> (FrameBuffer, u64);

    fn on_pointer_motion(&mut self, pt: Point) -> ShellInputOutcome;
    fn on_pointer_button(
        &mut self,
        pt: Point,
        button: ShellPointerButton,
        pressed: bool,
    ) -> ShellInputOutcome;
    fn on_pointer_axis(&mut self, pt: Point, dx: f32, dy: f32) -> ShellInputOutcome;
    fn on_key_input(&mut self, key: ShellKeyInput) -> ShellInputOutcome;
    fn set_modifiers(&mut self, mods: ShellModifiers);

    fn handle_ipc_request(&mut self, req: IpcRequest) -> IpcResponse;
    fn poll_hot_reload(&mut self, watcher: &mut AssetWatcher) -> bool;
    fn poll_sys_providers(&mut self) -> bool;

    fn drain_pending_spawn_commands(&mut self) -> Vec<String>;
    fn drain_pending_close_windows(&mut self) -> Vec<u64>;
    fn is_exit_requested(&self) -> bool;
}

#[derive(Clone)]
struct CachedMemoryBuffer {
    serial: u64,
    width: u32,
    height: u32,
    buffer: MemoryRenderBuffer,
}

/// Caches Smithay `MemoryRenderBuffer` GPU textures for HTML/CSS shell surfaces,
/// wallpaper, and Server-Side Window Decorations so CPU `FrameBuffer`s are only uploaded
/// when their `serial` or dimensions change.
#[derive(Default)]
pub struct ShellBufferCache {
    wallpaper: Option<CachedMemoryBuffer>,
    layers: HashMap<String, CachedMemoryBuffer>,
    decorations: HashMap<u64, CachedMemoryBuffer>,
}

impl ShellBufferCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remove_window(&mut self, window_id: u64) {
        self.decorations.remove(&window_id);
    }

    fn get_or_update_entry<'a>(
        slot: &'a mut Option<CachedMemoryBuffer>,
        fb: &FrameBuffer,
        serial: u64,
    ) -> &'a MemoryRenderBuffer {
        let w = fb.width.max(1);
        let h = fb.height.max(1);
        let needs_new = slot
            .as_ref()
            .map(|c| c.serial != serial || c.width != w || c.height != h)
            .unwrap_or(true);
        if needs_new {
            let mem_buf = MemoryRenderBuffer::from_slice(
                &fb.pixels,
                Fourcc::Abgr8888,
                (w as i32, h as i32),
                1,
                Transform::Normal,
                None,
            );
            *slot = Some(CachedMemoryBuffer {
                serial,
                width: w,
                height: h,
                buffer: mem_buf,
            });
        }
        &slot.as_ref().unwrap().buffer
    }

    pub fn wallpaper_element<R>(
        &mut self,
        renderer: &mut R,
        fb: &FrameBuffer,
        serial: u64,
    ) -> Result<MemoryRenderBufferRenderElement<R>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let buf = Self::get_or_update_entry(&mut self.wallpaper, fb, serial);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            SmithayPoint::<f64, Physical>::from((0.0, 0.0)),
            buf,
            Some(1.0),
            None,
            Some(SmithaySize::from((fb.width as i32, fb.height as i32))),
            Kind::Unspecified,
        )
    }

    pub fn layer_element<R>(
        &mut self,
        renderer: &mut R,
        item: &ShellLayerRenderItem,
        scale: f64,
    ) -> Result<MemoryRenderBufferRenderElement<R>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let w = item.frame.width.max(1);
        let h = item.frame.height.max(1);
        let entry = self.layers.entry(item.name.clone());
        let cached = entry
            .and_modify(|c| {
                if c.serial != item.serial || c.width != w || c.height != h {
                    c.buffer = MemoryRenderBuffer::from_slice(
                        &item.frame.pixels,
                        Fourcc::Abgr8888,
                        (w as i32, h as i32),
                        1,
                        Transform::Normal,
                        None,
                    );
                    c.serial = item.serial;
                    c.width = w;
                    c.height = h;
                }
            })
            .or_insert_with(|| CachedMemoryBuffer {
                serial: item.serial,
                width: w,
                height: h,
                buffer: MemoryRenderBuffer::from_slice(
                    &item.frame.pixels,
                    Fourcc::Abgr8888,
                    (w as i32, h as i32),
                    1,
                    Transform::Normal,
                    None,
                ),
            });

        let phys_loc = SmithayPoint::<f64, Physical>::from((
            (item.rect.x as f64) * scale,
            (item.rect.y as f64) * scale,
        ));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            phys_loc,
            &cached.buffer,
            Some(1.0),
            None,
            Some(SmithaySize::from((w as i32, h as i32))),
            Kind::Unspecified,
        )
    }

    pub fn decoration_element<R>(
        &mut self,
        renderer: &mut R,
        window_id: u64,
        frame_rect: Rect,
        fb: &FrameBuffer,
        serial: u64,
        scale: f64,
    ) -> Result<MemoryRenderBufferRenderElement<R>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let w = fb.width.max(1);
        let h = fb.height.max(1);
        let entry = self.decorations.entry(window_id);
        let cached = entry
            .and_modify(|c| {
                if c.serial != serial || c.width != w || c.height != h {
                    c.buffer = MemoryRenderBuffer::from_slice(
                        &fb.pixels,
                        Fourcc::Abgr8888,
                        (w as i32, h as i32),
                        1,
                        Transform::Normal,
                        None,
                    );
                    c.serial = serial;
                    c.width = w;
                    c.height = h;
                }
            })
            .or_insert_with(|| CachedMemoryBuffer {
                serial,
                width: w,
                height: h,
                buffer: MemoryRenderBuffer::from_slice(
                    &fb.pixels,
                    Fourcc::Abgr8888,
                    (w as i32, h as i32),
                    1,
                    Transform::Normal,
                    None,
                ),
            });

        let phys_loc = SmithayPoint::<f64, Physical>::from((
            (frame_rect.x as f64) * scale,
            (frame_rect.y as f64) * scale,
        ));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            phys_loc,
            &cached.buffer,
            Some(1.0),
            None,
            Some(SmithaySize::from((w as i32, h as i32))),
            Kind::Unspecified,
        )
    }
}
