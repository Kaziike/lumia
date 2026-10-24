use std::{
    collections::HashMap,
    process::{Child, Command, Stdio},
    time::Instant,
};

use smithay::{
    desktop::{PopupManager, Space, Window},
    input::{Seat, SeatState},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle, Resource,
        },
    },
    utils::{Logical, Point, Size, Transform, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        seat::WaylandFocus,
        selection::data_device::{set_data_device_focus, DataDeviceState},
        shell::{
            wlr_layer::WlrLayerShellState,
            xdg::{decoration::XdgDecorationState, ToplevelSurface, XdgShellState},
        },
        shm::ShmState,
    },
};
use tracing::{debug, info, warn};

use crate::shell_bridge::{ShellBufferCache, ShellHost, ShellInputOutcome};

/// Per-client Wayland state required by Smithay's `CompositorHandler`.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Central Smithay compositor state for Lumina.
pub struct LuminaState {
    pub display_handle: DisplayHandle,
    pub start_time: Instant,
    pub socket_name: String,

    // Wayland protocol globals
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<LuminaState>,
    pub data_device_state: DataDeviceState,
    pub output_manager_state: OutputManagerState,

    // Desktop space & popups
    pub space: Space<Window>,
    pub popups: PopupManager,
    pub seat: Seat<LuminaState>,
    pub output: Output,

    // Lumina HTML/CSS shell & WM engine bridge
    pub shell: Box<dyn ShellHost>,
    pub windows: HashMap<u64, Window>,
    pub buffer_cache: ShellBufferCache,
    pub next_window_id: u64,

    // Input & frame state
    pub pointer_location: Point<f64, Logical>,
    pub needs_redraw: bool,
    pub should_exit: bool,
    pub spawned_children: Vec<Child>,
}

impl LuminaState {
    pub fn new(
        display: &Display<LuminaState>,
        socket_name: String,
        shell: Box<dyn ShellHost>,
    ) -> Self {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);

        let mut seat = seat_state.new_wl_seat(&dh, "seat-0");
        if let Err(e) = seat.add_keyboard(Default::default(), 200, 25) {
            warn!("Failed to initialize Wayland seat keyboard: {e}");
        }
        seat.add_pointer();

        let (out_w, out_h) = shell.output_size();
        let output = Output::new(
            "eDP-1".to_string(),
            PhysicalProperties {
                size: (340, 190).into(),
                subpixel: Subpixel::HorizontalRgb,
                make: "Lumina".into(),
                model: "Smithay Output".into(),
            },
        );
        let _output_global = output.create_global::<Self>(&dh);
        let mode = Mode {
            size: (out_w as i32, out_h as i32).into(),
            refresh: 60_000,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(1.0)),
            Some((0, 0).into()),
        );
        output.set_preferred(mode);

        let mut space = Space::default();
        space.map_output(&output, (0, 0));

        Self {
            display_handle: dh,
            start_time: Instant::now(),
            socket_name,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            output_manager_state,
            space,
            popups: PopupManager::default(),
            seat,
            output,
            shell,
            windows: HashMap::new(),
            buffer_cache: ShellBufferCache::new(),
            next_window_id: 1,
            pointer_location: (out_w as f64 / 2.0, out_h as f64 / 2.0).into(),
            needs_redraw: true,
            should_exit: false,
            spawned_children: Vec::new(),
        }
    }

    pub fn window_id_for_surface(&self, surface: &WlSurface) -> Option<u64> {
        self.windows.iter().find_map(|(&id, win)| {
            win.wl_surface()
                .filter(|s| s.as_ref() == surface)
                .map(|_| id)
        })
    }

    pub fn window_id_for_toplevel(&self, toplevel: &ToplevelSurface) -> Option<u64> {
        self.window_id_for_surface(toplevel.wl_surface())
    }

    pub fn focus_window(&mut self, window_id: Option<u64>) {
        let target_surface = window_id
            .and_then(|id| self.windows.get(&id))
            .and_then(|w| w.wl_surface().map(|s| s.into_owned()));

        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, target_surface.clone(), serial);
        }

        let client = target_surface
            .as_ref()
            .and_then(|s| self.display_handle.get_client(s.id()).ok());
        set_data_device_focus(&self.display_handle, &self.seat, client);
    }

    pub fn apply_shell_outcome(&mut self, outcome: ShellInputOutcome) {
        if outcome.redraw {
            self.needs_redraw = true;
        }
        if outcome.exit_requested || self.shell.is_exit_requested() {
            self.should_exit = true;
        }
        if let Some(win_id) = outcome.focused_window {
            self.focus_window(Some(win_id));
        } else if self.shell.any_modal_open() {
            self.focus_window(None);
        }
        self.sync_from_shell();
    }

    /// Synchronize window positions, sizes, activation states, close requests, and spawns
    /// from `LuminaShell` (`wm-core`) into Smithay's `Space<Window>` and `xdg_toplevel` surfaces.
    pub fn sync_from_shell(&mut self) {
        self.spawned_children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));

        if self.shell.is_exit_requested() {
            self.should_exit = true;
        }

        for close_id in self.shell.drain_pending_close_windows() {
            if let Some(window) = self.windows.get(&close_id) {
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_close();
                }
            }
        }

        for cmd in self.shell.drain_pending_spawn_commands() {
            let trimmed = cmd.trim();
            if trimmed.is_empty() {
                continue;
            }
            info!(
                command = %trimmed,
                wayland_display = %self.socket_name,
                "Spawning child process on Smithay Wayland socket"
            );
            match Command::new("sh")
                .arg("-c")
                .arg(trimmed)
                .env("WAYLAND_DISPLAY", &self.socket_name)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => self.spawned_children.push(child),
                Err(e) => warn!(command = %trimmed, "Failed to spawn command: {e}"),
            }
        }

        let focused_id = if self.shell.any_modal_open() {
            None
        } else {
            self.shell.focused_window_id()
        };

        let placements = self.shell.compute_window_placements();
        for managed in &placements {
            let win_id = managed.placement.window_id;
            let Some(window) = self.windows.get(&win_id).cloned() else {
                continue;
            };
            let content = managed.placement.content_rect;
            let loc: Point<i32, Logical> = (content.x.round() as i32, content.y.round() as i32).into();
            let is_focused = focused_id == Some(win_id);
            self.space.map_element(window.clone(), loc, is_focused);

            if let Some(toplevel) = window.toplevel() {
                let target_size: Size<i32, Logical> = (
                    (content.width.round() as i32).max(1),
                    (content.height.round() as i32).max(1),
                )
                    .into();
                toplevel.with_pending_state(|state| {
                    state.size = Some(target_size);
                    if is_focused {
                        state.states.set(xdg_toplevel::State::Activated);
                    } else {
                        state.states.unset(xdg_toplevel::State::Activated);
                    }
                    if managed.is_maximized {
                        state.states.set(xdg_toplevel::State::Maximized);
                    } else {
                        state.states.unset(xdg_toplevel::State::Maximized);
                    }
                });
                toplevel.send_pending_configure();
            }
        }

        debug!(
            window_count = self.windows.len(),
            visible_placements = placements.len(),
            "Synchronized Smithay space with LuminaShell"
        );
    }
}
