pub mod backend;
pub mod handlers;
pub mod input;
pub mod render;
pub mod shell_bridge;
pub mod sources;
pub mod state;

pub use backend::winit::run_winit_compositor;
pub use shell_bridge::{
    ManagedWindowPlacement, ShellBufferCache, ShellGrabKind, ShellHost, ShellInputOutcome,
    ShellKeyInput, ShellKeySymbol, ShellLayerRenderItem, ShellModifiers, ShellPointerButton,
};
pub use smithay::reexports::wayland_server::Display as WaylandDisplay;
pub use state::{ClientState, LuminaState};

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_server::AssetWatcher;
    use smithay::reexports::wayland_server::Display;
    use wm_core::LayerTier;
    use wm_proto::{
        FrameBuffer, IpcPayload, IpcRequest, IpcResponse, Point, Rect, WindowPlacement,
    };

    struct DummyShellHost {
        width: u32,
        height: u32,
        opened: Vec<(u64, String, String)>,
    }

    impl ShellHost for DummyShellHost {
        fn output_size(&self) -> (u32, u32) {
            (self.width, self.height)
        }
        fn usable_rect(&self) -> Rect {
            Rect {
                x: 0.0,
                y: 44.0,
                width: self.width as f32,
                height: (self.height - 44) as f32,
            }
        }
        fn any_modal_open(&self) -> bool {
            false
        }
        fn resize_output(&mut self, width: u32, height: u32, _scale: f32) {
            self.width = width;
            self.height = height;
        }
        fn on_client_window_opened(&mut self, window_id: u64, title: String, app_id: String) {
            self.opened.push((window_id, title, app_id));
        }
        fn on_client_window_updated(&mut self, _window_id: u64, _title: String, _app_id: String) {}
        fn on_client_window_closed(&mut self, _window_id: u64) {}
        fn focused_window_id(&self) -> Option<u64> {
            self.opened.last().map(|(id, _, _)| *id)
        }
        fn compute_window_placements(&self) -> Vec<ManagedWindowPlacement> {
            self.opened
                .iter()
                .map(|(id, title, app_id)| ManagedWindowPlacement {
                    placement: WindowPlacement {
                        window_id: *id,
                        frame_rect: Rect {
                            x: 12.0,
                            y: 56.0,
                            width: 800.0,
                            height: 600.0,
                        },
                        content_rect: Rect {
                            x: 14.0,
                            y: 88.0,
                            width: 796.0,
                            height: 566.0,
                        },
                        titlebar_rect: None,
                        border_width: 2.0,
                        border_radius: 10.0,
                        is_focused: true,
                        is_floating: false,
                        is_fullscreen: false,
                        z_order: 0,
                    },
                    title: title.clone(),
                    app_id: app_id.clone(),
                    is_urgent: false,
                    is_maximized: false,
                })
                .collect()
        }
        fn wallpaper_with_serial(&mut self) -> (FrameBuffer, u64) {
            (FrameBuffer::new(16, 16), 1)
        }
        fn visible_shell_layers(&mut self) -> Vec<ShellLayerRenderItem> {
            vec![ShellLayerRenderItem {
                name: "top-bar".to_string(),
                tier: LayerTier::Top,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: self.width as f32,
                    height: 44.0,
                },
                frame: FrameBuffer::new(self.width, 44),
                serial: 1,
            }]
        }
        fn render_window_decoration(
            &mut self,
            _info: &ManagedWindowPlacement,
        ) -> (FrameBuffer, u64) {
            (FrameBuffer::new(800, 600), 1)
        }
        fn on_pointer_motion(&mut self, _pt: Point) -> ShellInputOutcome {
            ShellInputOutcome::default()
        }
        fn on_pointer_button(
            &mut self,
            _pt: Point,
            _button: ShellPointerButton,
            _pressed: bool,
        ) -> ShellInputOutcome {
            ShellInputOutcome::default()
        }
        fn on_pointer_axis(&mut self, _pt: Point, _dx: f32, _dy: f32) -> ShellInputOutcome {
            ShellInputOutcome::default()
        }
        fn on_key_input(&mut self, _key: ShellKeyInput) -> ShellInputOutcome {
            ShellInputOutcome::default()
        }
        fn set_modifiers(&mut self, _mods: ShellModifiers) {}
        fn handle_ipc_request(&mut self, req: IpcRequest) -> IpcResponse {
            IpcResponse::success(req.id, IpcPayload::Message("ok".to_string()))
        }
        fn poll_hot_reload(&mut self, _watcher: &mut AssetWatcher) -> bool {
            false
        }
        fn poll_sys_providers(&mut self) -> bool {
            false
        }
        fn drain_pending_spawn_commands(&mut self) -> Vec<String> {
            Vec::new()
        }
        fn drain_pending_close_windows(&mut self) -> Vec<u64> {
            Vec::new()
        }
        fn is_exit_requested(&self) -> bool {
            false
        }
    }

    #[test]
    fn test_smithay_lumina_state_initialization() {
        let display: Display<LuminaState> = Display::new().expect("create Wayland Display");
        let host = Box::new(DummyShellHost {
            width: 1920,
            height: 1080,
            opened: Vec::new(),
        });
        let mut state = LuminaState::new(&display, "wayland-test-0".to_string(), host);
        assert_eq!(state.shell.output_size(), (1920, 1080));
        state.sync_from_shell();
        assert!(!state.should_exit);
    }
}
