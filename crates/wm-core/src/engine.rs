use crate::config::{WmConfig, WmThemeConfig};
use crate::layout::{compute_workspace_placements, default_floating_rect};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use wm_proto::{
    Direction, LayoutMode, OutputDescriptor, Rect, ResizeEdge, StatePatch, StateValue,
    SurfaceMetadata, WindowDescriptor, WindowId, WindowManagerEngine, WindowPlacement,
    WorkspaceDescriptor, WorkspaceId, WorkspaceTarget, WmAction,
};

/// Active pointer-driven interactive move or resize operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InteractiveGrab {
    Move {
        win_id: WindowId,
        start_rect: Rect,
    },
    Resize {
        win_id: WindowId,
        edge: ResizeEdge,
        start_rect: Rect,
    },
}

/// Core Window Manager state machine implementing `wm_proto::WindowManagerEngine`.
pub struct LuminaWmCore {
    config: WmConfig,
    workspaces: Vec<WorkspaceDescriptor>,
    windows: Vec<WindowDescriptor>,
    outputs: Vec<OutputDescriptor>,
    active_workspace: WorkspaceId,
    overview_open: bool,
    scrolling_ratios: HashMap<WindowId, f32>,
    saved_floating_rects: HashMap<WindowId, Rect>,
    floating_z_stack: Vec<WindowId>,
    workspace_focus_history: HashMap<WorkspaceId, WindowId>,
    interactive_grab: Option<InteractiveGrab>,
}

impl Default for LuminaWmCore {
    fn default() -> Self {
        Self::new()
    }
}

impl LuminaWmCore {
    pub fn new() -> Self {
        Self::with_config(WmConfig::default())
    }

    pub fn with_config(config: WmConfig) -> Self {
        let primary_output_name = "eDP-1".to_string();
        let full_rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let top_zone = config.default_topbar_exclusive_zone.max(0.0);
        let usable_rect = Rect::new(
            0.0,
            top_zone,
            full_rect.width,
            (full_rect.height - top_zone).max(100.0),
        );

        let primary_output = OutputDescriptor {
            name: primary_output_name.clone(),
            make: "Lumina".to_string(),
            model: "Virtual Wayland Display".to_string(),
            rect: full_rect,
            usable_rect,
            scale: 1.0,
            refresh_hz: 60,
        };

        let ws_count = config.workspace_count.max(1);
        let workspaces = (1..=ws_count)
            .map(|idx| WorkspaceDescriptor {
                id: idx,
                idx,
                name: idx.to_string(),
                output: primary_output_name.clone(),
                layout_mode: config.default_layout,
                is_active: idx == 1,
                is_focused: idx == 1,
                is_urgent: false,
                window_count: 0,
            })
            .collect();

        Self {
            config,
            workspaces,
            windows: Vec::new(),
            outputs: vec![primary_output],
            active_workspace: 1,
            overview_open: false,
            scrolling_ratios: HashMap::new(),
            saved_floating_rects: HashMap::new(),
            floating_z_stack: Vec::new(),
            workspace_focus_history: HashMap::new(),
            interactive_grab: None,
        }
    }

    pub fn active_workspace_id(&self) -> WorkspaceId {
        self.active_workspace
    }

    pub fn active_layout_mode(&self) -> LayoutMode {
        self.workspaces
            .iter()
            .find(|ws| ws.id == self.active_workspace)
            .map(|ws| ws.layout_mode)
            .unwrap_or(self.config.default_layout)
    }

    pub fn overview_open(&self) -> bool {
        self.overview_open
    }

    pub fn focused_window_id(&self) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|w| w.workspace_id == self.active_workspace && w.is_focused && !w.is_minimized)
            .or_else(|| self.windows.iter().find(|w| w.is_focused))
            .map(|w| w.id)
    }

    pub fn focused_window(&self) -> Option<&WindowDescriptor> {
        let fid = self.focused_window_id()?;
        self.windows.iter().find(|w| w.id == fid)
    }

    pub fn theme_config(&self) -> &WmThemeConfig {
        &self.config.theme
    }

    pub fn set_theme_config(&mut self, theme: WmThemeConfig) {
        self.config.theme = theme;
        self.sync_workspace_and_window_geometries();
    }

    pub fn apply_css_vars(&mut self, vars: &BTreeMap<String, String>) {
        self.config.theme.apply_css_vars(vars);
        self.sync_workspace_and_window_geometries();
    }

    pub fn interactive_grab(&self) -> Option<InteractiveGrab> {
        self.interactive_grab
    }

    /// Updates an active interactive move/resize operation by delta `(dx, dy)`.
    pub fn update_interactive_grab(&mut self, dx: f32, dy: f32) -> Vec<StatePatch> {
        let Some(grab) = self.interactive_grab else {
            return Vec::new();
        };
        match grab {
            InteractiveGrab::Move { win_id, start_rect } => {
                if let Some(win) = self.windows.iter_mut().find(|w| w.id == win_id) {
                    win.is_floating = true;
                    win.rect = Rect::new(
                        start_rect.x + dx,
                        start_rect.y + dy,
                        start_rect.width,
                        start_rect.height,
                    );
                    self.saved_floating_rects.insert(win_id, win.rect);
                }
            }
            InteractiveGrab::Resize {
                win_id,
                edge,
                start_rect,
            } => {
                if let Some(win) = self.windows.iter_mut().find(|w| w.id == win_id) {
                    win.is_floating = true;
                    let mut x = start_rect.x;
                    let mut y = start_rect.y;
                    let mut w = start_rect.width;
                    let mut h = start_rect.height;

                    match edge {
                        ResizeEdge::Right => w = (w + dx).max(120.0),
                        ResizeEdge::Bottom => h = (h + dy).max(80.0),
                        ResizeEdge::Left => {
                            let new_w = (w - dx).max(120.0);
                            x += w - new_w;
                            w = new_w;
                        }
                        ResizeEdge::Top => {
                            let new_h = (h - dy).max(80.0);
                            y += h - new_h;
                            h = new_h;
                        }
                        ResizeEdge::TopLeft => {
                            let new_w = (w - dx).max(120.0);
                            let new_h = (h - dy).max(80.0);
                            x += w - new_w;
                            y += h - new_h;
                            w = new_w;
                            h = new_h;
                        }
                        ResizeEdge::TopRight => {
                            let new_h = (h - dy).max(80.0);
                            y += h - new_h;
                            w = (w + dx).max(120.0);
                            h = new_h;
                        }
                        ResizeEdge::BottomLeft => {
                            let new_w = (w - dx).max(120.0);
                            x += w - new_w;
                            w = new_w;
                            h = (h + dy).max(80.0);
                        }
                        ResizeEdge::BottomRight => {
                            w = (w + dx).max(120.0);
                            h = (h + dy).max(80.0);
                        }
                    }

                    win.rect = Rect::new(x, y, w, h);
                    self.saved_floating_rects.insert(win_id, win.rect);
                }
            }
        }
        self.sync_workspace_and_window_geometries();
        self.export_state_patches()
    }

    pub fn end_interactive_grab(&mut self) {
        self.interactive_grab = None;
    }

    /// Adds or replaces a display output descriptor.
    pub fn add_output(&mut self, output: OutputDescriptor) {
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.name == output.name) {
            *existing = output;
        } else {
            self.outputs.push(output);
        }
        self.sync_workspace_and_window_geometries();
    }

    /// Updates the usable area of an output by subtracting exclusive zones from a shell surface.
    pub fn apply_surface_exclusive_zone(&mut self, output_name: &str, meta: &SurfaceMetadata) {
        if meta.exclusive_zone <= 0 || !meta.visible_by_default {
            return;
        }
        let zone = meta.exclusive_zone as f32;
        if let Some(out) = self.outputs.iter_mut().find(|o| o.name == output_name) {
            let mut usable = out.rect;
            let has_top = meta.anchor.iter().any(|a| a == "top");
            let has_bottom = meta.anchor.iter().any(|a| a == "bottom");
            let has_left = meta.anchor.iter().any(|a| a == "left");
            let has_right = meta.anchor.iter().any(|a| a == "right");

            if has_top && !has_bottom {
                usable.y += zone;
                usable.height = (usable.height - zone).max(100.0);
            } else if has_bottom && !has_top {
                usable.height = (usable.height - zone).max(100.0);
            } else if has_left && !has_right {
                usable.x += zone;
                usable.width = (usable.width - zone).max(100.0);
            } else if has_right && !has_left {
                usable.width = (usable.width - zone).max(100.0);
            }
            out.usable_rect = usable;
        }
        self.sync_workspace_and_window_geometries();
    }

    pub fn primary_usable_rect(&self) -> Rect {
        let output_name = self
            .workspaces
            .iter()
            .find(|ws| ws.id == self.active_workspace)
            .map(|ws| ws.output.as_str())
            .unwrap_or("eDP-1");

        self.outputs
            .iter()
            .find(|o| o.name == output_name)
            .or_else(|| self.outputs.first())
            .map(|o| o.usable_rect)
            .unwrap_or_else(|| Rect::new(0.0, 44.0, 1920.0, 1036.0))
    }

    fn resolve_workspace_target(&self, target: &WorkspaceTarget) -> Result<WorkspaceId, String> {
        match target {
            WorkspaceTarget::Index(idx) => self
                .workspaces
                .iter()
                .find(|ws| ws.id == *idx || ws.idx == *idx)
                .map(|ws| ws.id)
                .ok_or_else(|| format!("Workspace index {idx} does not exist")),
            WorkspaceTarget::Name(name) => {
                if let Some(ws) = self.workspaces.iter().find(|ws| ws.name == *name) {
                    return Ok(ws.id);
                }
                if let Ok(idx) = name.trim().parse::<u32>() {
                    if let Some(ws) = self
                        .workspaces
                        .iter()
                        .find(|ws| ws.id == idx || ws.idx == idx)
                    {
                        return Ok(ws.id);
                    }
                }
                Err(format!("Workspace '{name}' does not exist"))
            }
            WorkspaceTarget::Relative(delta) => {
                let n = self.workspaces.len() as i32;
                if n == 0 {
                    return Err("No workspaces configured".to_string());
                }
                let cur_pos = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.id == self.active_workspace)
                    .unwrap_or(0) as i32;
                let next_pos = (cur_pos + delta).rem_euclid(n) as usize;
                Ok(self.workspaces[next_pos].id)
            }
        }
    }

    fn raise_floating_window(&mut self, win_id: WindowId) {
        self.floating_z_stack.retain(|&id| id != win_id);
        self.floating_z_stack.push(win_id);
    }

    fn set_focused_window_internal(&mut self, target_id: Option<WindowId>) {
        for win in &mut self.windows {
            if Some(win.id) == target_id {
                win.is_focused = true;
                win.is_urgent = false;
                self.workspace_focus_history.insert(win.workspace_id, win.id);
            } else {
                win.is_focused = false;
            }
        }
        if let Some(id) = target_id {
            self.raise_floating_window(id);
        }
    }

    fn pick_focus_candidate_for_workspace(&self, ws_id: WorkspaceId) -> Option<WindowId> {
        if let Some(&hist_id) = self.workspace_focus_history.get(&ws_id) {
            if self
                .windows
                .iter()
                .any(|w| w.id == hist_id && w.workspace_id == ws_id && !w.is_minimized)
            {
                return Some(hist_id);
            }
        }
        self.windows
            .iter()
            .rev()
            .find(|w| w.workspace_id == ws_id && !w.is_minimized)
            .map(|w| w.id)
    }

    fn find_directional_neighbor(&self, from_id: WindowId, dir: Direction) -> Option<WindowId> {
        let visible: Vec<&WindowDescriptor> = self
            .windows
            .iter()
            .filter(|w| w.workspace_id == self.active_workspace && !w.is_minimized)
            .collect();

        if visible.len() <= 1 {
            return visible.first().map(|w| w.id);
        }

        let cur_idx = visible.iter().position(|w| w.id == from_id)?;
        let cur = visible[cur_idx];
        let cx = cur.rect.x + cur.rect.width * 0.5;
        let cy = cur.rect.y + cur.rect.height * 0.5;

        let mut best: Option<(WindowId, f32)> = None;
        for cand in &visible {
            if cand.id == from_id {
                continue;
            }
            let tx = cand.rect.x + cand.rect.width * 0.5;
            let ty = cand.rect.y + cand.rect.height * 0.5;
            let dx = tx - cx;
            let dy = ty - cy;

            let score_opt = match dir {
                Direction::Left if dx < -1.0 => Some((-dx) + dy.abs() * 2.0),
                Direction::Right if dx > 1.0 => Some(dx + dy.abs() * 2.0),
                Direction::Up if dy < -1.0 => Some((-dy) + dx.abs() * 2.0),
                Direction::Down if dy > 1.0 => Some(dy + dx.abs() * 2.0),
                _ => None,
            };

            if let Some(score) = score_opt {
                if best.map_or(true, |(_, b_score)| score < b_score) {
                    best = Some((cand.id, score));
                }
            }
        }

        if let Some((best_id, _)) = best {
            return Some(best_id);
        }

        // Fallback to cyclic workspace window order (e.g. Monocle or edge of stack/strip)
        let step: isize = match dir {
            Direction::Left | Direction::Up => -1,
            Direction::Right | Direction::Down => 1,
        };
        let next_idx = (cur_idx as isize + step).rem_euclid(visible.len() as isize) as usize;
        Some(visible[next_idx].id)
    }

    fn sync_workspace_and_window_geometries(&mut self) {
        // 1. Update workspace metadata (active flags, window counts, urgency)
        for ws in &mut self.workspaces {
            ws.is_active = ws.id == self.active_workspace;
            ws.is_focused = ws.id == self.active_workspace;
            ws.window_count = self
                .windows
                .iter()
                .filter(|w| w.workspace_id == ws.id)
                .count();
            ws.is_urgent = self
                .windows
                .iter()
                .any(|w| w.workspace_id == ws.id && w.is_urgent);
        }

        // 2. Compute placements for each workspace and sync back `win.rect`
        let usable_default = self.primary_usable_rect();
        for ws in &self.workspaces {
            let ws_usable = self
                .outputs
                .iter()
                .find(|o| o.name == ws.output)
                .map(|o| o.usable_rect)
                .unwrap_or(usable_default);

            let ws_windows: Vec<&WindowDescriptor> = self
                .windows
                .iter()
                .filter(|w| w.workspace_id == ws.id)
                .collect();

            let placements = compute_workspace_placements(
                &ws_windows,
                ws.layout_mode,
                ws_usable,
                &self.config.theme,
                &self.scrolling_ratios,
                &self.floating_z_stack,
            );

            let rect_map: HashMap<WindowId, Rect> = placements
                .into_iter()
                .map(|p| (p.window_id, p.frame_rect))
                .collect();

            for win in &mut self.windows {
                if win.workspace_id == ws.id {
                    if let Some(&frame_rect) = rect_map.get(&win.id) {
                        win.rect = frame_rect;
                    }
                }
            }
        }
    }
}

impl WindowManagerEngine for LuminaWmCore {
    fn dispatch_wm_action(&mut self, action: &WmAction) -> Result<Vec<StatePatch>, String> {
        match action {
            WmAction::FocusWorkspace { target } => {
                let target_ws_id = self.resolve_workspace_target(target)?;
                self.active_workspace = target_ws_id;
                let next_focus = self.pick_focus_candidate_for_workspace(target_ws_id);
                self.set_focused_window_internal(next_focus);
            }
            WmAction::FocusWindow { id } => {
                let Some(win_ws) = self
                    .windows
                    .iter()
                    .find(|w| w.id == *id)
                    .map(|w| w.workspace_id)
                else {
                    return Err(format!("Window {id} not found"));
                };
                self.active_workspace = win_ws;
                if let Some(win) = self.windows.iter_mut().find(|w| w.id == *id) {
                    win.is_minimized = false;
                    win.is_urgent = false;
                }
                self.set_focused_window_internal(Some(*id));
            }
            WmAction::FocusDirection { dir } => {
                if let Some(cur_id) = self.focused_window_id() {
                    if let Some(next_id) = self.find_directional_neighbor(cur_id, *dir) {
                        self.set_focused_window_internal(Some(next_id));
                    }
                } else if let Some(first_id) =
                    self.pick_focus_candidate_for_workspace(self.active_workspace)
                {
                    self.set_focused_window_internal(Some(first_id));
                }
            }
            WmAction::CloseWindow { id } => {
                let target_id = id
                    .or_else(|| self.focused_window_id())
                    .ok_or_else(|| "No window to close".to_string())?;
                return Ok(self.on_window_closed(target_id));
            }
            WmAction::ToggleFloating { id } => {
                let target_id = id
                    .or_else(|| self.focused_window_id())
                    .ok_or_else(|| "No window to toggle floating".to_string())?;
                let usable = self.primary_usable_rect();
                let win_idx = self
                    .windows
                    .iter()
                    .position(|w| w.id == target_id)
                    .ok_or_else(|| format!("Window {target_id} not found"))?;

                let new_floating = !self.windows[win_idx].is_floating;
                self.windows[win_idx].is_floating = new_floating;
                if new_floating {
                    let float_rect = self
                        .saved_floating_rects
                        .get(&target_id)
                        .copied()
                        .unwrap_or_else(|| default_floating_rect(win_idx, usable));
                    self.windows[win_idx].rect = float_rect;
                    self.saved_floating_rects.insert(target_id, float_rect);
                    self.raise_floating_window(target_id);
                } else {
                    self.saved_floating_rects
                        .insert(target_id, self.windows[win_idx].rect);
                }
            }
            WmAction::ToggleFullscreen { id } => {
                let target_id = id
                    .or_else(|| self.focused_window_id())
                    .ok_or_else(|| "No window to toggle fullscreen".to_string())?;
                let win = self
                    .windows
                    .iter_mut()
                    .find(|w| w.id == target_id)
                    .ok_or_else(|| format!("Window {target_id} not found"))?;
                win.is_fullscreen = !win.is_fullscreen;
            }
            WmAction::ToggleMaximize { id } => {
                let target_id = id
                    .or_else(|| self.focused_window_id())
                    .ok_or_else(|| "No window to toggle maximize".to_string())?;
                let win = self
                    .windows
                    .iter_mut()
                    .find(|w| w.id == target_id)
                    .ok_or_else(|| format!("Window {target_id} not found"))?;
                win.is_maximized = !win.is_maximized;
            }
            WmAction::MinimizeWindow { id } => {
                let target_id = id
                    .or_else(|| self.focused_window_id())
                    .ok_or_else(|| "No window to minimize".to_string())?;
                let win = self
                    .windows
                    .iter_mut()
                    .find(|w| w.id == target_id)
                    .ok_or_else(|| format!("Window {target_id} not found"))?;
                win.is_minimized = true;
                win.is_focused = false;
                if self.workspace_focus_history.get(&self.active_workspace) == Some(&target_id) {
                    self.workspace_focus_history.remove(&self.active_workspace);
                }
                let next_focus = self.pick_focus_candidate_for_workspace(self.active_workspace);
                self.set_focused_window_internal(next_focus);
            }
            WmAction::FocusOrMinimizeWindow { id } => {
                let Some(win) = self.windows.iter().find(|w| w.id == *id) else {
                    return Err(format!("Window {id} not found"));
                };
                if win.workspace_id == self.active_workspace && win.is_focused && !win.is_minimized
                {
                    return self.dispatch_wm_action(&WmAction::MinimizeWindow { id: Some(*id) });
                } else {
                    return self.dispatch_wm_action(&WmAction::FocusWindow { id: *id });
                }
            }
            WmAction::MoveWindowToWorkspace { win_id, workspace } => {
                let target_win_id = win_id
                    .or_else(|| self.focused_window_id())
                    .ok_or_else(|| "No window to move to workspace".to_string())?;
                let target_ws_id = self.resolve_workspace_target(workspace)?;

                let win = self
                    .windows
                    .iter_mut()
                    .find(|w| w.id == target_win_id)
                    .ok_or_else(|| format!("Window {target_win_id} not found"))?;
                let old_ws = win.workspace_id;
                win.workspace_id = target_ws_id;
                self.workspace_focus_history
                    .insert(target_ws_id, target_win_id);

                if old_ws == self.active_workspace && target_ws_id != self.active_workspace {
                    if self.workspace_focus_history.get(&old_ws) == Some(&target_win_id) {
                        self.workspace_focus_history.remove(&old_ws);
                    }
                    let next_focus = self.pick_focus_candidate_for_workspace(self.active_workspace);
                    self.set_focused_window_internal(next_focus);
                }
            }
            WmAction::MoveWindowDirection { dir } => {
                let Some(focused_id) = self.focused_window_id() else {
                    return Ok(self.export_state_patches());
                };
                let layout_mode = self.active_layout_mode();
                let is_floating = self
                    .windows
                    .iter()
                    .find(|w| w.id == focused_id)
                    .map(|w| w.is_floating)
                    .unwrap_or(false)
                    || layout_mode == LayoutMode::Floating;

                if is_floating {
                    let step = self.config.floating_nudge_step;
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == focused_id) {
                        match dir {
                            Direction::Left => win.rect.x -= step,
                            Direction::Right => win.rect.x += step,
                            Direction::Up => win.rect.y -= step,
                            Direction::Down => win.rect.y += step,
                        }
                        self.saved_floating_rects.insert(focused_id, win.rect);
                    }
                } else if let Some(neighbor_id) = self.find_directional_neighbor(focused_id, *dir) {
                    if neighbor_id != focused_id {
                        let pos_a = self.windows.iter().position(|w| w.id == focused_id);
                        let pos_b = self.windows.iter().position(|w| w.id == neighbor_id);
                        if let (Some(a), Some(b)) = (pos_a, pos_b) {
                            self.windows.swap(a, b);
                        }
                    }
                }
            }
            WmAction::ResizeWindow { id, dx, dy } => {
                let target_id = id
                    .or_else(|| self.focused_window_id())
                    .ok_or_else(|| "No window to resize".to_string())?;
                let usable = self.primary_usable_rect();
                let layout_mode = self.active_layout_mode();
                let is_floating = self
                    .windows
                    .iter()
                    .find(|w| w.id == target_id)
                    .map(|w| w.is_floating)
                    .unwrap_or(false)
                    || layout_mode == LayoutMode::Floating;

                if is_floating {
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == target_id) {
                        win.rect.width = (win.rect.width + *dx as f32).max(120.0);
                        win.rect.height = (win.rect.height + *dy as f32).max(80.0);
                        self.saved_floating_rects.insert(target_id, win.rect);
                    }
                } else {
                    let delta_ratio = (*dx as f32) / usable.width.max(100.0);
                    match layout_mode {
                        LayoutMode::Tiling => {
                            self.config.theme.master_ratio =
                                (self.config.theme.master_ratio + delta_ratio).clamp(0.15, 0.85);
                        }
                        LayoutMode::Scrolling => {
                            let cur = self
                                .scrolling_ratios
                                .get(&target_id)
                                .copied()
                                .unwrap_or(self.config.theme.scrolling_column_ratio);
                            self.scrolling_ratios
                                .insert(target_id, (cur + delta_ratio).clamp(0.25, 1.0));
                        }
                        LayoutMode::Bsp => {
                            self.config.theme.bsp_split_ratio =
                                (self.config.theme.bsp_split_ratio + delta_ratio).clamp(0.2, 0.8);
                        }
                        LayoutMode::Monocle | LayoutMode::Floating => {}
                    }
                }
            }
            WmAction::SetLayoutMode { mode } => {
                if let Some(ws) = self
                    .workspaces
                    .iter_mut()
                    .find(|ws| ws.id == self.active_workspace)
                {
                    ws.layout_mode = *mode;
                }
            }
            WmAction::CycleLayoutMode => {
                if let Some(ws) = self
                    .workspaces
                    .iter_mut()
                    .find(|ws| ws.id == self.active_workspace)
                {
                    ws.layout_mode = match ws.layout_mode {
                        LayoutMode::Tiling => LayoutMode::Scrolling,
                        LayoutMode::Scrolling => LayoutMode::Bsp,
                        LayoutMode::Bsp => LayoutMode::Monocle,
                        LayoutMode::Monocle => LayoutMode::Floating,
                        LayoutMode::Floating => LayoutMode::Tiling,
                    };
                }
            }
            WmAction::ToggleOverview => {
                self.overview_open = !self.overview_open;
            }
            WmAction::BeginInteractiveMove { win_id } => {
                let start_rect = self
                    .windows
                    .iter()
                    .find(|w| w.id == *win_id)
                    .map(|w| w.rect)
                    .ok_or_else(|| format!("Window {win_id} not found"))?;
                self.set_focused_window_internal(Some(*win_id));
                self.interactive_grab = Some(InteractiveGrab::Move {
                    win_id: *win_id,
                    start_rect,
                });
            }
            WmAction::BeginInteractiveResize { win_id, edge } => {
                let start_rect = self
                    .windows
                    .iter()
                    .find(|w| w.id == *win_id)
                    .map(|w| w.rect)
                    .ok_or_else(|| format!("Window {win_id} not found"))?;
                self.set_focused_window_internal(Some(*win_id));
                self.interactive_grab = Some(InteractiveGrab::Resize {
                    win_id: *win_id,
                    edge: *edge,
                    start_rect,
                });
            }
        }

        self.sync_workspace_and_window_geometries();
        Ok(self.export_state_patches())
    }

    fn on_window_opened(&mut self, mut win: WindowDescriptor) -> Vec<StatePatch> {
        if win.workspace_id == 0
            || !self
                .workspaces
                .iter()
                .any(|ws| ws.id == win.workspace_id)
        {
            win.workspace_id = self.active_workspace;
        }

        let usable = self.primary_usable_rect();
        if win.rect.width <= 1.0 || win.rect.height <= 1.0 {
            let default_rect = default_floating_rect(self.windows.len(), usable);
            win.rect = default_rect;
            self.saved_floating_rects.insert(win.id, default_rect);
        } else {
            self.saved_floating_rects.insert(win.id, win.rect);
        }

        let win_id = win.id;
        let should_focus = win.workspace_id == self.active_workspace && !win.is_minimized;

        // Replace if id already exists, otherwise push
        if let Some(existing) = self.windows.iter_mut().find(|w| w.id == win_id) {
            *existing = win;
        } else {
            self.windows.push(win);
        }

        self.raise_floating_window(win_id);
        if should_focus {
            self.set_focused_window_internal(Some(win_id));
        }

        self.sync_workspace_and_window_geometries();
        self.export_state_patches()
    }

    fn on_window_closed(&mut self, win_id: WindowId) -> Vec<StatePatch> {
        self.windows.retain(|w| w.id != win_id);
        self.floating_z_stack.retain(|&id| id != win_id);
        self.scrolling_ratios.remove(&win_id);
        self.saved_floating_rects.remove(&win_id);
        self.workspace_focus_history.retain(|_, &mut fid| fid != win_id);

        if let Some(grab) = self.interactive_grab {
            let grab_win = match grab {
                InteractiveGrab::Move { win_id, .. } => win_id,
                InteractiveGrab::Resize { win_id, .. } => win_id,
            };
            if grab_win == win_id {
                self.interactive_grab = None;
            }
        }

        if self.focused_window_id().is_none() {
            let next_focus = self.pick_focus_candidate_for_workspace(self.active_workspace);
            self.set_focused_window_internal(next_focus);
        }

        self.sync_workspace_and_window_geometries();
        self.export_state_patches()
    }

    fn compute_window_placements(&self, usable_output_rect: Rect) -> Vec<WindowPlacement> {
        let ws_windows: Vec<&WindowDescriptor> = self
            .windows
            .iter()
            .filter(|w| w.workspace_id == self.active_workspace)
            .collect();

        compute_workspace_placements(
            &ws_windows,
            self.active_layout_mode(),
            usable_output_rect,
            &self.config.theme,
            &self.scrolling_ratios,
            &self.floating_z_stack,
        )
    }

    fn windows(&self) -> Vec<WindowDescriptor> {
        self.windows.clone()
    }

    fn workspaces(&self) -> Vec<WorkspaceDescriptor> {
        self.workspaces.clone()
    }

    fn outputs(&self) -> Vec<OutputDescriptor> {
        self.outputs.clone()
    }

    fn export_state_patches(&self) -> Vec<StatePatch> {
        let workspaces_val = to_state_value(&self.workspaces);
        let windows_val = to_state_value(&self.windows);
        let outputs_val = to_state_value(&self.outputs);
        let focused_val = match self.focused_window() {
            Some(win) => to_state_value(win),
            None => StateValue::Null,
        };

        vec![
            StatePatch::Set {
                path: "wm.workspaces".to_string(),
                value: workspaces_val,
            },
            StatePatch::Set {
                path: "wm.windows".to_string(),
                value: windows_val,
            },
            StatePatch::Set {
                path: "wm.focused_window".to_string(),
                value: focused_val,
            },
            StatePatch::Set {
                path: "wm.active_workspace".to_string(),
                value: StateValue::Int(self.active_workspace as i64),
            },
            StatePatch::Set {
                path: "wm.layout_mode".to_string(),
                value: StateValue::String(self.active_layout_mode().as_str().to_string()),
            },
            StatePatch::Set {
                path: "wm.overview_open".to_string(),
                value: StateValue::Bool(self.overview_open),
            },
            StatePatch::Set {
                path: "wm.outputs".to_string(),
                value: outputs_val,
            },
        ]
    }
}

fn to_state_value<T: Serialize>(value: &T) -> StateValue {
    serde_json::to_value(value)
        .and_then(serde_json::from_value::<StateValue>)
        .unwrap_or(StateValue::Null)
}
