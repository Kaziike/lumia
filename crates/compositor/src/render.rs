use std::time::Duration;

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            Kind,
        },
        ImportAll, ImportMem, Renderer,
    },
    desktop::{layer_map_for_output, PopupManager},
    utils::{Physical, Point, Scale},
    wayland::{seat::WaylandFocus, shell::wlr_layer::Layer as WlrLayer},
};
use tracing::warn;
use wm_core::LayerTier;

use crate::state::LuminaState;

render_elements! {
    pub LuminaRenderElement<R> where R: ImportAll + ImportMem;
    Surface = WaylandSurfaceRenderElement<R>,
    Memory = MemoryRenderBufferRenderElement<R>,
}

impl LuminaState {
    /// Collect all render elements for the output in **front-to-back** order
    /// (as required by `OutputDamageTracker::render_output`).
    pub fn collect_render_elements<R>(&mut self, renderer: &mut R) -> Vec<LuminaRenderElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let scale = self.output.current_scale().fractional_scale();
        let mut elements: Vec<LuminaRenderElement<R>> = Vec::with_capacity(32);

        let shell_layers = self.shell.visible_shell_layers();

        // 1. Overlay & Top HTML/CSS shell surfaces (front-to-back)
        for tier in [LayerTier::Overlay, LayerTier::Top] {
            for item in shell_layers.iter().rev().filter(|l| l.tier == tier) {
                match self.buffer_cache.layer_element(renderer, item, scale) {
                    Ok(elem) => elements.push(LuminaRenderElement::from(elem)),
                    Err(err) => warn!(layer = %item.name, "Failed to upload shell layer: {err}"),
                }
            }
        }

        // External wlr-layer-shell Overlay & Top surfaces
        self.collect_wlr_layer_elements(
            renderer,
            scale,
            &[WlrLayer::Overlay, WlrLayer::Top],
            &mut elements,
        );

        // 2. Managed client windows in front-to-back order
        // (`compute_window_placements` returns back-to-front, so we iterate `.rev()`)
        let placements = self.shell.compute_window_placements();
        for managed in placements.iter().rev() {
            let win_id = managed.placement.window_id;
            if let Some(window) = self.windows.get(&win_id).cloned() {
                if let Some(wl_surface) = window.wl_surface() {
                    let content = managed.placement.content_rect;
                    let content_phys = Point::<i32, Physical>::from((
                        (content.x as f64 * scale).round() as i32,
                        (content.y as f64 * scale).round() as i32,
                    ));
                    let win_geo_loc = window.geometry().loc.to_physical_precise_round(scale);
                    let surface_loc = content_phys - win_geo_loc;

                    // 2a. Window popups (in front of window content)
                    for (popup, popup_loc) in PopupManager::popups_for_surface(&wl_surface) {
                        let popup_offset = surface_loc
                            + popup_loc.to_physical_precise_round(scale)
                            - popup.geometry().loc.to_physical_precise_round(scale);
                        let popup_elems: Vec<LuminaRenderElement<R>> =
                            render_elements_from_surface_tree(
                                renderer,
                                popup.wl_surface(),
                                popup_offset,
                                Scale::from(scale),
                                1.0,
                                Kind::Unspecified,
                            );
                        elements.extend(popup_elems);
                    }

                    // 2b. Client Wayland surface tree at `content_rect`
                    let win_elems: Vec<LuminaRenderElement<R>> = render_elements_from_surface_tree(
                        renderer,
                        &wl_surface,
                        surface_loc,
                        Scale::from(scale),
                        1.0,
                        Kind::Unspecified,
                    );
                    elements.extend(win_elems);
                }
            }

            // 2c. Server-Side Window Decoration (SSD titlebar + border + fallback fill at `frame_rect`)
            let (dec_fb, dec_serial) = self.shell.render_window_decoration(managed);
            match self.buffer_cache.decoration_element(
                renderer,
                win_id,
                managed.placement.frame_rect,
                &dec_fb,
                dec_serial,
                scale,
            ) {
                Ok(elem) => elements.push(LuminaRenderElement::from(elem)),
                Err(err) => warn!(win_id, "Failed to upload window SSD decoration: {err}"),
            }
        }

        // 3. Bottom & Background external wlr-layer-shell surfaces
        self.collect_wlr_layer_elements(
            renderer,
            scale,
            &[WlrLayer::Bottom, WlrLayer::Background],
            &mut elements,
        );

        // 4. Bottom & Background HTML/CSS shell surfaces
        for tier in [LayerTier::Bottom, LayerTier::Background] {
            for item in shell_layers.iter().rev().filter(|l| l.tier == tier) {
                match self.buffer_cache.layer_element(renderer, item, scale) {
                    Ok(elem) => elements.push(LuminaRenderElement::from(elem)),
                    Err(err) => warn!(layer = %item.name, "Failed to upload shell layer: {err}"),
                }
            }
        }

        // 5. Desktop Wallpaper (backmost element)
        let (wp_fb, wp_serial) = self.shell.wallpaper_with_serial();
        match self
            .buffer_cache
            .wallpaper_element(renderer, &wp_fb, wp_serial)
        {
            Ok(elem) => elements.push(LuminaRenderElement::from(elem)),
            Err(err) => warn!("Failed to upload wallpaper texture: {err}"),
        }

        elements
    }

    fn collect_wlr_layer_elements<R>(
        &self,
        renderer: &mut R,
        scale: f64,
        tiers: &[WlrLayer],
        out: &mut Vec<LuminaRenderElement<R>>,
    ) where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let map = layer_map_for_output(&self.output);
        for &tier in tiers {
            for layer in map.layers_on(tier).rev() {
                if let Some(geo) = map.layer_geometry(layer) {
                    let loc = geo.loc.to_physical_precise_round(scale);
                    let elems: Vec<LuminaRenderElement<R>> = render_elements_from_surface_tree(
                        renderer,
                        layer.wl_surface(),
                        loc,
                        Scale::from(scale),
                        1.0,
                        Kind::Unspecified,
                    );
                    out.extend(elems);
                }
            }
        }
    }

    /// Send `wl_surface.frame` callbacks to all mapped client windows and layer surfaces.
    pub fn send_frame_callbacks(&self) {
        let elapsed = self.start_time.elapsed();
        let output = self.output.clone();

        for window in self.windows.values() {
            window.send_frame(
                &output,
                elapsed,
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }

        let map = layer_map_for_output(&output);
        for layer in map.layers() {
            layer.send_frame(
                &output,
                elapsed,
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }
    }
}
