use std::collections::BTreeMap;
use wm_core::{AnchorEdges, KeyboardInteractivity, LayerTier};
use wm_proto::{Point, Rect, SurfaceMetadata, WidgetStatus};

use crate::daemon::LiveSurface;

/// Computed placement for a single HTML/CSS shell layer surface on a display output.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellLayerPlacement {
    pub name: String,
    pub tier: LayerTier,
    pub anchors: AnchorEdges,
    pub keyboard_mode: KeyboardInteractivity,
    pub exclusive_zone: i32,
    pub rect: Rect,
    pub hit_rect: Rect,
    pub visible: bool,
    pub dismiss_on_outside_click: bool,
}

/// Result of testing a pointer click against an active modal overlay (`launcher`, `quick_settings`).
#[derive(Clone, Debug, PartialEq)]
pub enum ModalHit {
    /// Click landed inside the active modal at surface-local `(x, y)`.
    Inside {
        surface_name: String,
        local_pt: Point,
    },
    /// Click landed outside the active modal, which should dismiss/close the widget.
    Outside { widget_name: String },
}

/// Declarative layout table for all HTML/CSS layer surfaces on an output.
/// Replaces hardcoded pixel coordinates in both rendering and pointer hit-testing.
#[derive(Clone, Debug)]
pub struct ShellLayout {
    output_rect: Rect,
    usable_rect: Rect,
    layers: Vec<ShellLayerPlacement>,
}

impl ShellLayout {
    /// Computes the positions, hit rectangles, and output `usable_rect` for all shell surfaces
    /// on the given `output_rect` using each surface's `<meta name="shell-surface">` metadata.
    pub fn compute_for_output(
        output_rect: Rect,
        surfaces: &BTreeMap<String, LiveSurface>,
        widgets: &BTreeMap<String, WidgetStatus>,
        has_toast: bool,
    ) -> Self {
        let mut usable = output_rect;

        // 1. Determine Topbar placement from `topbar` surface metadata
        let topbar_meta = surfaces
            .get("topbar")
            .map(|s| s.dom.metadata().clone())
            .unwrap_or_else(SurfaceMetadata::default);
        let topbar_visible = surfaces
            .get("topbar")
            .map(|s| s.visible)
            .unwrap_or(topbar_meta.visible_by_default);
        let topbar_h = topbar_meta.height.unwrap_or(44) as f32;
        let topbar_ez = if topbar_visible {
            topbar_meta.exclusive_zone.max(0) as f32
        } else {
            0.0
        };
        if topbar_ez > 0.0 {
            usable.y += topbar_ez;
            usable.height = (usable.height - topbar_ez).max(100.0);
        }
        let topbar_rect = Rect::new(output_rect.x, output_rect.y, output_rect.width, topbar_h);

        // 2. Determine Bottom Dock placement from `dock` surface metadata
        let dock_meta = surfaces.get("dock").map(|s| s.dom.metadata().clone());
        let dock_visible = surfaces.get("dock").map(|s| s.visible).unwrap_or(true);
        let dock_w = widgets
            .get("dock")
            .map(|w| w.width as f32)
            .or_else(|| surfaces.get("dock").map(|s| s.viewport.width))
            .unwrap_or(680.0);
        let dock_h = dock_meta
            .as_ref()
            .and_then(|m| m.height)
            .map(|h| h as f32)
            .or_else(|| surfaces.get("dock").map(|s| s.viewport.height))
            .unwrap_or(58.0);
        let dock_ez = dock_meta
            .as_ref()
            .map(|m| if dock_visible { m.exclusive_zone.max(0) as f32 } else { 0.0 })
            .unwrap_or(0.0);
        if dock_ez > 0.0 {
            usable.height = (usable.height - dock_ez).max(100.0);
        }
        let dock_margin_bottom = 10.0;
        let dock_rect = Rect::new(
            output_rect.x + (output_rect.width - dock_w) / 2.0,
            output_rect.bottom() - dock_h - dock_margin_bottom,
            dock_w,
            dock_h,
        );

        // 3. Quick Settings / Control Center overlay modal
        let qs_visible = widgets
            .get("quick_settings")
            .map(|w| w.visible)
            .unwrap_or(false)
            || widgets
                .get("control-center")
                .map(|w| w.visible)
                .unwrap_or(false);
        let (qs_w, qs_h) = widgets
            .get("quick_settings")
            .map(|w| (w.width as f32, w.height as f32))
            .or_else(|| {
                surfaces
                    .get("quick_settings")
                    .map(|s| (s.viewport.width, s.viewport.height))
            })
            .unwrap_or((380.0, 560.0));
        let top_overlay_y = output_rect.y + topbar_h + 10.0;
        let right_margin = 16.0;
        let qs_rect = Rect::new(
            output_rect.right() - qs_w - right_margin,
            top_overlay_y,
            qs_w,
            qs_h,
        );

        // 4. App Launcher modal (centered overlay)
        let launcher_visible = widgets
            .get("launcher")
            .map(|w| w.visible)
            .unwrap_or(false);
        let (launcher_w, launcher_h) = widgets
            .get("launcher")
            .map(|w| (w.width as f32, w.height as f32))
            .or_else(|| {
                surfaces
                    .get("launcher")
                    .map(|s| (s.viewport.width, s.viewport.height))
            })
            .unwrap_or((640.0, 440.0));
        let launcher_rect = Rect::new(
            output_rect.x + (output_rect.width - launcher_w) / 2.0,
            output_rect.y + (output_rect.height - launcher_h) / 2.0,
            launcher_w,
            launcher_h,
        );

        // 5. Toast notifications surface (top-right, shown when overlays are closed)
        let notif_surf_visible = surfaces
            .get("notifications")
            .map(|s| s.visible)
            .unwrap_or(false);
        let (notif_w, notif_h) = surfaces
            .get("notifications")
            .map(|s| (s.viewport.width, s.viewport.height))
            .unwrap_or((360.0, 96.0));
        let notif_rect = Rect::new(
            output_rect.right() - notif_w - right_margin,
            top_overlay_y,
            notif_w,
            notif_h,
        );
        // Hit box slightly taller (110px) to cover toast card shadow/padding cleanly
        let notif_hit_rect = Rect::new(
            notif_rect.x,
            notif_rect.y,
            notif_rect.width,
            notif_rect.height.max(110.0),
        );

        // Ordered bottom-to-top for painting; reversed for hit-testing
        let layers = vec![
            ShellLayerPlacement {
                name: "topbar".to_string(),
                tier: LayerTier::Top,
                anchors: AnchorEdges {
                    top: true,
                    bottom: false,
                    left: true,
                    right: true,
                },
                keyboard_mode: KeyboardInteractivity::None,
                exclusive_zone: topbar_meta.exclusive_zone,
                rect: topbar_rect,
                hit_rect: topbar_rect,
                visible: topbar_visible,
                dismiss_on_outside_click: false,
            },
            ShellLayerPlacement {
                name: "dock".to_string(),
                tier: LayerTier::Top,
                anchors: AnchorEdges {
                    top: false,
                    bottom: true,
                    left: false,
                    right: false,
                },
                keyboard_mode: KeyboardInteractivity::None,
                exclusive_zone: dock_ez as i32,
                rect: dock_rect,
                hit_rect: dock_rect,
                visible: dock_visible,
                dismiss_on_outside_click: false,
            },
            ShellLayerPlacement {
                name: "notifications".to_string(),
                tier: LayerTier::Top,
                anchors: AnchorEdges {
                    top: true,
                    bottom: false,
                    left: false,
                    right: true,
                },
                keyboard_mode: KeyboardInteractivity::None,
                exclusive_zone: 0,
                rect: notif_rect,
                hit_rect: notif_hit_rect,
                visible: notif_surf_visible && !qs_visible && !launcher_visible && has_toast,
                dismiss_on_outside_click: false,
            },
            ShellLayerPlacement {
                name: "quick_settings".to_string(),
                tier: LayerTier::Overlay,
                anchors: AnchorEdges {
                    top: true,
                    bottom: false,
                    left: false,
                    right: true,
                },
                keyboard_mode: KeyboardInteractivity::OnDemand,
                exclusive_zone: 0,
                rect: qs_rect,
                hit_rect: qs_rect,
                visible: qs_visible,
                dismiss_on_outside_click: true,
            },
            ShellLayerPlacement {
                name: "launcher".to_string(),
                tier: LayerTier::Overlay,
                anchors: AnchorEdges::default(),
                keyboard_mode: KeyboardInteractivity::Exclusive,
                exclusive_zone: 0,
                rect: launcher_rect,
                hit_rect: launcher_rect,
                visible: launcher_visible,
                dismiss_on_outside_click: true,
            },
        ];

        Self {
            output_rect,
            usable_rect: usable,
            layers,
        }
    }

    pub fn output_rect(&self) -> Rect {
        self.output_rect
    }

    pub fn usable_rect(&self) -> Rect {
        self.usable_rect
    }

    /// Returns all shell layer placements in bottom-to-top paint order.
    pub fn layers_bottom_to_top(&self) -> &[ShellLayerPlacement] {
        &self.layers
    }

    /// Looks up a specific shell layer placement by surface name.
    pub fn layer(&self, name: &str) -> Option<&ShellLayerPlacement> {
        let canonical = match name {
            "panel" => "topbar",
            "control-center" => "quick_settings",
            other => other,
        };
        self.layers.iter().find(|l| l.name == canonical)
    }

    /// Returns `true` if any overlay modal (`launcher` or `quick_settings`) is currently visible.
    pub fn any_modal_open(&self) -> bool {
        self.layers
            .iter()
            .any(|l| l.visible && l.dismiss_on_outside_click)
    }

    /// Returns `true` if the `launcher` modal is currently visible.
    pub fn is_launcher_open(&self) -> bool {
        self.layer("launcher").is_some_and(|l| l.visible)
    }

    /// Returns `true` if the `quick_settings` modal is currently visible.
    pub fn is_quick_settings_open(&self) -> bool {
        self.layer("quick_settings").is_some_and(|l| l.visible)
    }

    /// Tests a pointer click against any open modal overlay (`launcher` first, then `quick_settings`).
    pub fn hit_test_modal(&self, pt: Point) -> Option<ModalHit> {
        for layer in self.layers.iter().rev() {
            if layer.visible && layer.dismiss_on_outside_click {
                if layer.hit_rect.contains(pt) {
                    return Some(ModalHit::Inside {
                        surface_name: layer.name.clone(),
                        local_pt: Point::new(pt.x - layer.rect.x, pt.y - layer.rect.y),
                    });
                } else {
                    return Some(ModalHit::Outside {
                        widget_name: layer.name.clone(),
                    });
                }
            }
        }
        None
    }

    /// Hit-tests a pointer position against visible non-modal `Top` layer surfaces (`notifications`, `topbar`, `dock`)
    /// in top-to-bottom order, returning `(surface_name, surface_local_point)`.
    pub fn hit_test_top_layer(&self, pt: Point, notifications_enabled: bool) -> Option<(&str, Point)> {
        for layer in self.layers.iter().rev() {
            if layer.dismiss_on_outside_click {
                continue;
            }
            let is_vis = if layer.name == "notifications" {
                notifications_enabled
            } else {
                layer.visible
            };
            if is_vis && layer.hit_rect.contains(pt) {
                return Some((
                    layer.name.as_str(),
                    Point::new(pt.x - layer.rect.x, pt.y - layer.rect.y),
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::LuminaDaemon;

    #[test]
    fn test_shell_layout_geometry_and_exclusive_zone() {
        let mut daemon = LuminaDaemon::new();
        let out_rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let layout = ShellLayout::compute_for_output(
            out_rect,
            &daemon.surfaces,
            &daemon.widgets,
            true,
        );

        // Topbar has 44px exclusive zone
        assert_eq!(layout.usable_rect(), Rect::new(0.0, 44.0, 1920.0, 1036.0));

        let topbar = layout.layer("topbar").unwrap();
        assert_eq!(topbar.rect, Rect::new(0.0, 0.0, 1920.0, 44.0));
        assert!(topbar.visible);

        let dock = layout.layer("dock").unwrap();
        assert_eq!(dock.rect, Rect::new(620.0, 1012.0, 680.0, 58.0));
        assert!(dock.visible);

        // Open launcher and verify modal hit-testing
        let _ = daemon.dispatch_action(wm_proto::ActionCommand::Shell(
            wm_proto::ShellAction::OpenWidget {
                name: "launcher".to_string(),
                args: Default::default(),
            },
        ));
        let layout2 = ShellLayout::compute_for_output(
            out_rect,
            &daemon.surfaces,
            &daemon.widgets,
            true,
        );
        assert!(layout2.is_launcher_open());
        assert_eq!(
            layout2.hit_test_modal(Point::new(960.0, 540.0)),
            Some(ModalHit::Inside {
                surface_name: "launcher".to_string(),
                local_pt: Point::new(320.0, 220.0),
            })
        );
        assert_eq!(
            layout2.hit_test_modal(Point::new(100.0, 100.0)),
            Some(ModalHit::Outside {
                widget_name: "launcher".to_string(),
            })
        );
    }
}
