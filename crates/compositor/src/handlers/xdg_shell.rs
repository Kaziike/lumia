use smithay::{
    delegate_xdg_shell,
    desktop::{PopupKind, Window},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_output, wl_seat, wl_surface::WlSurface},
    },
    utils::{Rectangle, Serial},
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};
use tracing::info;

use crate::state::LuminaState;

impl XdgShellHandler for LuminaState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let win_id = self.next_window_id;
        self.next_window_id += 1;

        let (title, app_id) = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|d| {
                    let attrs = d.lock().unwrap();
                    (
                        attrs.title.clone().unwrap_or_default(),
                        attrs.app_id.clone().unwrap_or_default(),
                    )
                })
                .unwrap_or_default()
        });

        let window = Window::new_wayland_window(surface.clone());
        self.windows.insert(win_id, window);

        info!(
            win_id,
            title = %title,
            app_id = %app_id,
            "New xdg_toplevel window registered in Smithay compositor"
        );

        self.shell.on_client_window_opened(win_id, title, app_id);
        self.sync_from_shell();

        if !self.shell.any_modal_open() {
            let focused = self.shell.focused_window_id();
            self.focus_window(focused);
        }
        self.needs_redraw = true;
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(win_id) = self.window_id_for_toplevel(&surface) {
            if let Some(window) = self.windows.remove(&win_id) {
                self.space.unmap_elem(&window);
            }
            self.buffer_cache.remove_window(win_id);
            self.shell.on_client_window_closed(win_id);
            let focused = if self.shell.any_modal_open() {
                None
            } else {
                self.shell.focused_window_id()
            };
            self.focus_window(focused);
            self.sync_from_shell();
            self.needs_redraw = true;
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.constrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::from(surface));
        self.needs_redraw = true;
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.constrain_popup(&surface);
        surface.send_repositioned(token);
        self.needs_redraw = true;
    }

    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Window movement is managed by LuminaShell's InputRouter (Super+LMB or SSD titlebar drag)
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        // Window resizing is managed by LuminaShell's InputRouter (Super+RMB or SSD border drag)
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        surface.send_configure();
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        surface.send_configure();
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}

impl LuminaState {
    fn constrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = smithay::desktop::find_popup_root_surface(&PopupKind::Xdg(popup.clone()))
        else {
            return;
        };
        let (out_w, out_h) = self.shell.output_size();
        let mut target = Rectangle::from_size((out_w as i32, out_h as i32).into());
        if let Some(win_id) = self.window_id_for_surface(&root) {
            if let Some(window) = self.windows.get(&win_id) {
                if let Some(loc) = self.space.element_location(window) {
                    target.loc -= loc;
                }
            }
        }
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    #[allow(dead_code)]
    pub fn find_window_by_surface(&self, surface: &WlSurface) -> Option<&Window> {
        self.window_id_for_surface(surface)
            .and_then(|id| self.windows.get(&id))
    }
}

delegate_xdg_shell!(LuminaState);
