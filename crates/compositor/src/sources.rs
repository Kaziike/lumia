use std::{sync::Arc, time::Duration};

use calloop::{
    timer::{TimeoutAction, Timer},
    LoopHandle,
};
use ipc_server::{AssetWatcher, IpcServer};
use smithay::wayland::socket::ListeningSocketSource;
use tracing::{info, warn};

use crate::state::{ClientState, LuminaState};

/// Attach the Wayland listening socket (`WAYLAND_DISPLAY`) to the `calloop` event loop.
pub fn insert_wayland_socket_source(
    loop_handle: &LoopHandle<'static, LuminaState>,
    listening_socket: ListeningSocketSource,
) -> Result<(), String> {
    loop_handle
        .insert_source(listening_socket, move |client_stream, _, state| {
            if let Err(err) = state
                .display_handle
                .insert_client(client_stream, Arc::new(ClientState::default()))
            {
                warn!("Failed to insert Wayland client into Smithay display: {err}");
            }
        })
        .map_err(|e| format!("Failed to register Wayland ListeningSocketSource: {e}"))?;
    Ok(())
}

/// Attach `IpcServer` (`lumina-ctl` NDJSON socket), `AssetWatcher` (HTML/CSS hot-reload),
/// and `sys_providers` telemetry polling to the `calloop` event loop.
pub fn insert_shell_maintenance_sources(
    loop_handle: &LoopHandle<'static, LuminaState>,
    mut ipc_server: IpcServer,
    mut watcher: Option<AssetWatcher>,
) -> Result<(), String> {
    info!(
        socket = %ipc_server.socket_path().display(),
        "Registered Lumina IPC & hot-reload sources on calloop event loop"
    );

    // Fast tick (16ms) for non-blocking IPC requests and CSS/HTML hot-reload events
    loop_handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(16)),
            move |_, _, state| {
                if let Ok(count) = ipc_server.poll_once(|req| state.shell.handle_ipc_request(req)) {
                    if count > 0 {
                        state.sync_from_shell();
                        let focused = if state.shell.any_modal_open() {
                            None
                        } else {
                            state.shell.focused_window_id()
                        };
                        state.focus_window(focused);
                        state.needs_redraw = true;
                    }
                }

                if let Some(ref mut w) = watcher {
                    if state.shell.poll_hot_reload(w) {
                        state.sync_from_shell();
                        state.needs_redraw = true;
                    }
                }

                TimeoutAction::ToDuration(Duration::from_millis(16))
            },
        )
        .map_err(|e| format!("Failed to register IPC/hot-reload timer source: {e}"))?;

    // Periodic tick (1000ms) for system providers (clock, battery, CPU/RAM, network, audio)
    loop_handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(1000)),
            move |_, _, state| {
                if state.shell.poll_sys_providers() {
                    state.needs_redraw = true;
                }
                TimeoutAction::ToDuration(Duration::from_millis(1000))
            },
        )
        .map_err(|e| format!("Failed to register sys_providers timer source: {e}"))?;

    Ok(())
}
