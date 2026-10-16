use crate::action::{LayoutMode, WmAction};
use crate::layout::Rect;
use crate::state::StatePatch;
use serde::{Deserialize, Serialize};

pub type WindowId = u64;
pub type WorkspaceId = u32;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowDescriptor {
    pub id: WindowId,
    pub title: String,
    pub app_id: String,
    pub icon: String,
    pub workspace_id: WorkspaceId,
    pub is_focused: bool,
    pub is_floating: bool,
    pub is_fullscreen: bool,
    pub is_maximized: bool,
    pub is_minimized: bool,
    pub is_urgent: bool,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceDescriptor {
    pub id: WorkspaceId,
    pub idx: u32,
    pub name: String,
    pub output: String,
    pub layout_mode: LayoutMode,
    pub is_active: bool,
    pub is_focused: bool,
    pub is_urgent: bool,
    pub window_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputDescriptor {
    pub name: String,
    pub make: String,
    pub model: String,
    pub rect: Rect,
    pub usable_rect: Rect,
    pub scale: f32,
    pub refresh_hz: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub window_id: WindowId,
    /// Outer frame rect including server-side titlebar and CSS border.
    pub frame_rect: Rect,
    /// Client surface content rect inside the frame.
    pub content_rect: Rect,
    /// Optional server-side titlebar rect at the top of the frame.
    pub titlebar_rect: Option<Rect>,
    pub border_width: f32,
    pub border_radius: f32,
    pub is_focused: bool,
    pub is_floating: bool,
    pub is_fullscreen: bool,
    pub z_order: i32,
}

/// Trait implemented by `crates/wm-core` for the Window Manager state machine.
pub trait WindowManagerEngine: Send + Sync {
    fn dispatch_wm_action(&mut self, action: &WmAction) -> Result<Vec<StatePatch>, String>;
    fn on_window_opened(&mut self, win: WindowDescriptor) -> Vec<StatePatch>;
    fn on_window_closed(&mut self, win_id: WindowId) -> Vec<StatePatch>;
    fn compute_window_placements(&self, usable_output_rect: Rect) -> Vec<WindowPlacement>;
    fn windows(&self) -> Vec<WindowDescriptor>;
    fn workspaces(&self) -> Vec<WorkspaceDescriptor>;
    fn outputs(&self) -> Vec<OutputDescriptor>;
    fn export_state_patches(&self) -> Vec<StatePatch>;
}
