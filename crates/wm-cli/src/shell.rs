pub use crate::daemon::{LiveSurface, LuminaDaemon, LuminaShell};
pub use crate::input_router::{
    ClientInputForward, InputOutcome, InputRouter, InteractiveGrabKind, KeyInput, KeyPattern,
    KeySymbol, KeybindEntry, KeybindTable, Modifiers, PointerButton,
};
pub use crate::shell_layout::{ModalHit, ShellLayerPlacement, ShellLayout};

use compositor::{
    ManagedWindowPlacement, ShellGrabKind, ShellHost, ShellInputOutcome, ShellKeyInput,
    ShellKeySymbol, ShellLayerRenderItem, ShellModifiers, ShellPointerButton,
};
use ipc_server::AssetWatcher;
use wm_proto::{FrameBuffer, IpcRequest, IpcResponse, Point, Rect, WindowManagerEngine};

/// Adapter combining `LuminaShell` (`LuminaDaemon`) and `InputRouter` to implement
/// `compositor::ShellHost` for Smithay backends (`winit`, and future `drm/udev`).
pub struct LuminaShellAdapter {
    pub shell: LuminaShell,
    pub router: InputRouter,
}

impl LuminaShellAdapter {
    pub fn new(mut shell: LuminaShell) -> Self {
        shell.external_compositor_active = true;
        let router = InputRouter::new();
        Self { shell, router }
    }
}

impl ShellHost for LuminaShellAdapter {
    fn output_size(&self) -> (u32, u32) {
        (
            self.shell.output_width.max(640),
            self.shell.output_height.max(480),
        )
    }

    fn usable_rect(&self) -> Rect {
        self.shell.shell_layout().usable_rect()
    }

    fn any_modal_open(&self) -> bool {
        self.shell.shell_layout().any_modal_open()
    }

    fn resize_output(&mut self, width: u32, height: u32, scale: f32) {
        self.shell.resize_output(width, height, scale);
    }

    fn on_client_window_opened(&mut self, window_id: u64, title: String, app_id: String) {
        self.shell
            .on_client_window_opened(window_id, title, app_id);
    }

    fn on_client_window_updated(&mut self, window_id: u64, title: String, app_id: String) {
        self.shell
            .on_client_window_updated(window_id, title, app_id);
    }

    fn on_client_window_closed(&mut self, window_id: u64) {
        self.shell.on_client_window_closed(window_id);
    }

    fn focused_window_id(&self) -> Option<u64> {
        self.shell.wm.focused_window_id()
    }

    fn compute_window_placements(&self) -> Vec<ManagedWindowPlacement> {
        let usable = self.usable_rect();
        let placements = self.shell.wm.compute_window_placements(usable);
        let windows = self.shell.wm.windows();
        placements
            .into_iter()
            .map(|placement| {
                let (title, app_id, is_urgent, is_maximized) = windows
                    .iter()
                    .find(|w| w.id == placement.window_id)
                    .map(|w| {
                        (
                            w.title.clone(),
                            w.app_id.clone(),
                            w.is_urgent,
                            w.is_maximized,
                        )
                    })
                    .unwrap_or_else(|| {
                        (
                            format!("Window {}", placement.window_id),
                            "app".to_string(),
                            false,
                            false,
                        )
                    });
                ManagedWindowPlacement {
                    placement,
                    title,
                    app_id,
                    is_urgent,
                    is_maximized,
                }
            })
            .collect()
    }

    fn wallpaper_with_serial(&mut self) -> (FrameBuffer, u64) {
        self.shell.wallpaper_with_serial()
    }

    fn visible_shell_layers(&mut self) -> Vec<ShellLayerRenderItem> {
        let layout = self.shell.shell_layout();
        let layers: Vec<ShellLayerPlacement> = layout.layers_bottom_to_top().to_vec();
        let mut items = Vec::with_capacity(layers.len());
        for lp in layers {
            if !lp.visible {
                continue;
            }
            if let Some((fb, serial)) = self.shell.render_surface_with_serial(&lp.name) {
                items.push(ShellLayerRenderItem {
                    name: lp.name,
                    tier: lp.tier,
                    rect: lp.rect,
                    frame: fb,
                    serial,
                });
            }
        }
        items
    }

    fn render_window_decoration(&mut self, info: &ManagedWindowPlacement) -> (FrameBuffer, u64) {
        self.shell.render_window_decoration_frame(
            &info.placement,
            &info.app_id,
            &info.title,
            info.is_urgent,
        )
    }

    fn on_pointer_motion(&mut self, pt: Point) -> ShellInputOutcome {
        let o = self.router.on_pointer_motion(&mut self.shell, pt);
        convert_outcome(o, self.shell.wm.focused_window_id())
    }

    fn on_pointer_button(
        &mut self,
        pt: Point,
        button: ShellPointerButton,
        pressed: bool,
    ) -> ShellInputOutcome {
        let btn = match button {
            ShellPointerButton::Left => PointerButton::Left,
            ShellPointerButton::Right => PointerButton::Right,
            ShellPointerButton::Middle => PointerButton::Middle,
            ShellPointerButton::Other(code) => PointerButton::Other(code),
        };
        let o = self
            .router
            .on_pointer_button(&mut self.shell, pt, btn, pressed);
        convert_outcome(o, self.shell.wm.focused_window_id())
    }

    fn on_pointer_axis(&mut self, pt: Point, dx: f32, dy: f32) -> ShellInputOutcome {
        let o = self.router.on_pointer_axis(&mut self.shell, pt, dx, dy);
        convert_outcome(o, self.shell.wm.focused_window_id())
    }

    fn on_key_input(&mut self, key: ShellKeyInput) -> ShellInputOutcome {
        let input = KeyInput {
            sym: match key.sym {
                ShellKeySymbol::Enter => KeySymbol::Enter,
                ShellKeySymbol::Escape => KeySymbol::Escape,
                ShellKeySymbol::Space => KeySymbol::Space,
                ShellKeySymbol::Tab => KeySymbol::Tab,
                ShellKeySymbol::Backspace => KeySymbol::Backspace,
                ShellKeySymbol::Super => KeySymbol::Super,
                ShellKeySymbol::ArrowLeft => KeySymbol::ArrowLeft,
                ShellKeySymbol::ArrowRight => KeySymbol::ArrowRight,
                ShellKeySymbol::ArrowUp => KeySymbol::ArrowUp,
                ShellKeySymbol::ArrowDown => KeySymbol::ArrowDown,
                ShellKeySymbol::F(n) => KeySymbol::F(n),
                ShellKeySymbol::Digit(d) => KeySymbol::Digit(d),
                ShellKeySymbol::Char(s) => KeySymbol::Char(s),
                ShellKeySymbol::Other => KeySymbol::Other,
            },
            digit_code: key.digit_code,
            evdev_code: key.evdev_code,
            pressed: key.pressed,
            repeat: key.repeat,
            mods: convert_mods(key.mods),
        };
        let o = self.router.on_key_input(&mut self.shell, input);
        convert_outcome(o, self.shell.wm.focused_window_id())
    }

    fn set_modifiers(&mut self, mods: ShellModifiers) {
        self.router.set_modifiers(convert_mods(mods));
    }

    fn handle_ipc_request(&mut self, req: IpcRequest) -> IpcResponse {
        self.shell.handle_ipc_request(req)
    }

    fn poll_hot_reload(&mut self, watcher: &mut AssetWatcher) -> bool {
        !self.shell.poll_hot_reload(watcher).is_empty()
    }

    fn poll_sys_providers(&mut self) -> bool {
        let patches = self.shell.sys_providers.poll_patches();
        if !patches.is_empty() {
            self.shell.apply_state_patches(patches);
            true
        } else {
            false
        }
    }

    fn drain_pending_spawn_commands(&mut self) -> Vec<String> {
        self.shell.drain_pending_spawn_commands()
    }

    fn drain_pending_close_windows(&mut self) -> Vec<u64> {
        self.shell.drain_pending_close_windows()
    }

    fn is_exit_requested(&self) -> bool {
        self.shell
            .get_state_path(Some("sys.session.exit_requested"))
            .as_bool()
    }
}

fn convert_mods(m: ShellModifiers) -> Modifiers {
    Modifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        logo: m.logo,
    }
}

fn convert_outcome(o: InputOutcome, focused_window: Option<u64>) -> ShellInputOutcome {
    ShellInputOutcome {
        consumed_by_shell: o.consumed_by_shell,
        redraw: o.redraw,
        exit_requested: o.exit_requested,
        focused_window,
        started_grab: o.started_grab.map(|g| match g {
            InteractiveGrabKind::Move {
                window_id,
                start_pt,
            } => ShellGrabKind::Move {
                window_id,
                start_pt,
            },
            InteractiveGrabKind::Resize {
                window_id,
                edge,
                start_pt,
            } => ShellGrabKind::Resize {
                window_id,
                edge,
                start_pt,
            },
        }),
        ended_grab: o.ended_grab,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compositor::{LuminaState, WaylandDisplay};

    #[test]
    fn test_lumina_shell_adapter_with_smithay_state() {
        let mut daemon = LuminaDaemon::new();
        daemon.clear_startup_windows();
        let adapter = Box::new(LuminaShellAdapter::new(daemon));

        let display: WaylandDisplay<LuminaState> = WaylandDisplay::new().expect("Wayland Display");
        let mut state = LuminaState::new(&display, "wayland-lumina-test".to_string(), adapter);

        // Verify shell layout usable_rect (topbar 44px exclusive zone)
        let usable = state.shell.usable_rect();
        assert_eq!(usable.y, 44.0);
        assert_eq!(usable.width, 1920.0);
        assert_eq!(usable.height, 1036.0);

        // Simulate a Wayland client window opening through ShellHost
        state
            .shell
            .on_client_window_opened(101, "Foot Terminal".to_string(), "foot".to_string());
        state.sync_from_shell();

        let placements = state.shell.compute_window_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].placement.window_id, 101);
        assert_eq!(placements[0].title, "Foot Terminal");

        // Verify visible HTML/CSS shell layers (topbar)
        let layers = state.shell.visible_shell_layers();
        assert!(!layers.is_empty());
        assert!(layers.iter().any(|l| l.name == "topbar"));

        // Verify Alt+Space keybind via ShellHost opens launcher modal
        let outcome = state.shell.on_key_input(ShellKeyInput {
            sym: ShellKeySymbol::Space,
            digit_code: None,
            evdev_code: None,
            pressed: true,
            repeat: false,
            mods: ShellModifiers {
                alt: true,
                ..Default::default()
            },
        });
        assert!(outcome.consumed_by_shell);
        assert!(state.shell.any_modal_open());
    }
}
