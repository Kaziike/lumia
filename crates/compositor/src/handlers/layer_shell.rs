use smithay::{
    delegate_layer_shell,
    desktop::{layer_map_for_output, LayerSurface as DesktopLayerSurface},
    reexports::wayland_server::protocol::wl_output::WlOutput,
    wayland::shell::wlr_layer::{
        Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState,
    },
};
use tracing::info;

use crate::state::LuminaState;

impl WlrLayerShellHandler for LuminaState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        _output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        info!(
            namespace = %namespace,
            ?layer,
            "New wlr_layer_shell surface connected"
        );
        let desktop_layer = DesktopLayerSurface::new(surface, namespace);
        let mut map = layer_map_for_output(&self.output);
        let _ = map.map_layer(&desktop_layer);
        drop(map);
        self.needs_redraw = true;
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let mut map = layer_map_for_output(&self.output);
        let found = map
            .layers()
            .find(|l| l.layer_surface() == &surface)
            .cloned();
        if let Some(layer) = found {
            map.unmap_layer(&layer);
        }
        drop(map);
        self.needs_redraw = true;
    }
}

delegate_layer_shell!(LuminaState);
