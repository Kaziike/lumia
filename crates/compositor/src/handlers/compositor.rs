use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    desktop::{layer_map_for_output, PopupKind, WindowSurfaceType},
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        shell::{
            wlr_layer::LayerSurfaceData,
            xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData},
        },
        shm::{ShmHandler, ShmState},
    },
};

use crate::state::{ClientState, LuminaState};

impl CompositorHandler for LuminaState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("Wayland client missing ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }

            if let Some(win_id) = self.window_id_for_surface(&root) {
                if let Some(window) = self.windows.get(&win_id).cloned() {
                    window.on_commit();

                    if let Some(toplevel) = window.toplevel() {
                        let (initial_configure_sent, title, app_id) =
                            with_states(toplevel.wl_surface(), |states| {
                                let attrs = states
                                    .data_map
                                    .get::<XdgToplevelSurfaceData>()
                                    .unwrap()
                                    .lock()
                                    .unwrap();
                                (
                                    attrs.initial_configure_sent,
                                    attrs.title.clone().unwrap_or_default(),
                                    attrs.app_id.clone().unwrap_or_default(),
                                )
                            });

                        if !initial_configure_sent {
                            toplevel.send_configure();
                        } else {
                            self.shell.on_client_window_updated(win_id, title, app_id);
                        }
                    }
                }
            }
        }

        // Handle popup commits & initial configure
        self.popups.commit(surface);
        if let Some(popup) = self.popups.find_popup(surface) {
            match popup {
                PopupKind::Xdg(ref xdg) => {
                    let initial_configure_sent = with_states(surface, |states| {
                        states
                            .data_map
                            .get::<XdgPopupSurfaceData>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .initial_configure_sent
                    });
                    if !initial_configure_sent {
                        let _ = xdg.send_configure();
                    }
                }
                PopupKind::InputMethod(_) => {}
            }
        }

        // Handle external wlr-layer-shell surface commits
        let initial_layer_configure = {
            let mut map = layer_map_for_output(&self.output);
            if let Some(layer) = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned()
            {
                map.arrange();
                let initial_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<LayerSurfaceData>()
                        .map(|d| d.lock().unwrap().initial_configure_sent)
                        .unwrap_or(true)
                });
                if !initial_sent {
                    Some(layer)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(layer) = initial_layer_configure {
            layer.layer_surface().send_configure();
        }

        self.sync_from_shell();
        self.needs_redraw = true;
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        if let Some(win_id) = self.window_id_for_surface(surface) {
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
}

impl BufferHandler for LuminaState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for LuminaState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(LuminaState);
delegate_shm!(LuminaState);
