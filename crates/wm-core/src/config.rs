use std::collections::BTreeMap;
use wm_proto::LayoutMode;

/// Visual decoration and layout spacing configuration for `LuminaWmCore`.
#[derive(Clone, Debug, PartialEq)]
pub struct WmThemeConfig {
    /// Enable server-side window decorations (titlebar + border).
    pub ssd_enabled: bool,
    /// Height in pixels of the server-side window titlebar when SSD is enabled.
    pub titlebar_height: f32,
    /// Width in pixels of the window frame border.
    pub border_width: f32,
    /// Corner radius in pixels of the window frame.
    pub border_radius: f32,
    /// Inner gap in pixels between adjacent tiled windows.
    pub inner_gap: f32,
    /// Outer gap in pixels between tiled windows and the usable output edges.
    pub outer_gap: f32,
    /// Default master area width ratio (0.15..=0.85) in `LayoutMode::Tiling`.
    pub master_ratio: f32,
    /// Default column width ratio (0.25..=1.0) in `LayoutMode::Scrolling`.
    pub scrolling_column_ratio: f32,
    /// Default split ratio (0.2..=0.8) in `LayoutMode::Bsp`.
    pub bsp_split_ratio: f32,
}

impl Default for WmThemeConfig {
    fn default() -> Self {
        Self {
            ssd_enabled: true,
            titlebar_height: 28.0,
            border_width: 2.0,
            border_radius: 10.0,
            inner_gap: 10.0,
            outer_gap: 12.0,
            master_ratio: 0.55,
            scrolling_column_ratio: 0.65,
            bsp_split_ratio: 0.5,
        }
    }
}

impl WmThemeConfig {
    /// Updates theme parameters from CSS custom properties (`--wm-*` or `-wm-*`).
    pub fn apply_css_vars(&mut self, vars: &BTreeMap<String, String>) {
        for (k, v) in vars {
            let key = k.trim().trim_start_matches('-');
            let val = v.trim().trim_end_matches("px");
            match key {
                "wm-ssd" | "ssd-enabled" => {
                    self.ssd_enabled = matches!(val, "true" | "1" | "yes" | "on");
                }
                "wm-titlebar-height" | "titlebar-height" => {
                    if let Ok(n) = val.parse::<f32>() {
                        self.titlebar_height = n.max(0.0);
                    }
                }
                "wm-border-width" | "border-width" => {
                    if let Ok(n) = val.parse::<f32>() {
                        self.border_width = n.max(0.0);
                    }
                }
                "wm-border-radius" | "border-radius" => {
                    if let Ok(n) = val.parse::<f32>() {
                        self.border_radius = n.max(0.0);
                    }
                }
                "wm-gap-inner" | "wm-inner-gap" | "inner-gap" | "gap" => {
                    if let Ok(n) = val.parse::<f32>() {
                        self.inner_gap = n.max(0.0);
                    }
                }
                "wm-gap-outer" | "wm-outer-gap" | "outer-gap" => {
                    if let Ok(n) = val.parse::<f32>() {
                        self.outer_gap = n.max(0.0);
                    }
                }
                "wm-master-ratio" | "master-ratio" => {
                    if let Ok(n) = val.parse::<f32>() {
                        self.master_ratio = n.clamp(0.15, 0.85);
                    }
                }
                "wm-column-ratio" | "scrolling-column-ratio" => {
                    if let Ok(n) = val.parse::<f32>() {
                        self.scrolling_column_ratio = n.clamp(0.25, 1.0);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Top-level initialization settings for `LuminaWmCore`.
#[derive(Clone, Debug)]
pub struct WmConfig {
    pub workspace_count: u32,
    pub default_layout: LayoutMode,
    pub default_topbar_exclusive_zone: f32,
    pub floating_nudge_step: f32,
    pub theme: WmThemeConfig,
}

impl Default for WmConfig {
    fn default() -> Self {
        Self {
            workspace_count: 9,
            default_layout: LayoutMode::Tiling,
            default_topbar_exclusive_zone: 44.0,
            floating_nudge_step: 40.0,
            theme: WmThemeConfig::default(),
        }
    }
}
