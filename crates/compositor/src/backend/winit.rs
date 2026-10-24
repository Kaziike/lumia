use std::time::Duration;

use calloop::EventLoop;
use ipc_server::{AssetWatcher, IpcServer};
use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    desktop::layer_map_for_output,
    output::{Mode, Scale},
    reexports::wayland_server::Display,
    wayland::socket::ListeningSocketSource,
};
use tracing::{error, info, warn};

use crate::{
    shell_bridge::ShellHost,
    sources::{insert_shell_maintenance_sources, insert_wayland_socket_source},
    state::LuminaState,
};

/// Run the Smithay Wayland compositor inside a host `winit` window (X11 or Wayland nested session)
/// using `GlesRenderer` hardware-accelerated OpenGL ES 3.0 compositing.
pub fn run_winit_compositor(
    shell: Box<dyn ShellHost>,
    ipc_server: IpcServer,
    watcher: Option<AssetWatcher>,
) -> Result<(), String> {
    let mut event_loop: EventLoop<'static, LuminaState> =
        EventLoop::try_new().map_err(|e| format!("Failed to create calloop EventLoop: {e}"))?;

    let mut display: Display<LuminaState> =
        Display::new().map_err(|e| format!("Failed to create Wayland Display: {e}"))?;

    let listening_socket = ListeningSocketSource::new_auto()
        .map_err(|e| format!("Failed to bind Wayland ListeningSocket: {e}"))?;
    let socket_name = listening_socket
        .socket_name()
        .to_string_lossy()
        .into_owned();

    info!(
        wayland_display = %socket_name,
        "Lumina Smithay compositor listening for Wayland client connections"
    );

    let (mut backend, winit_evt_loop) = winit::init::<GlesRenderer>()
        .map_err(|e| format!("Failed to initialize Smithay winit + EGL/GLES backend: {e}"))?;

    let mut state = LuminaState::new(&display, socket_name, shell);

    // Sync initial winit window dimensions with Output and LuminaShell
    let init_size = backend.window_size();
    let init_scale = backend.scale_factor();
    if init_size.w > 0 && init_size.h > 0 {
        let mode = Mode {
            size: init_size,
            refresh: 60_000,
        };
        state.output.change_current_state(
            Some(mode),
            None,
            Some(Scale::Fractional(init_scale)),
            None,
        );
        state.output.set_preferred(mode);
        state.shell.resize_output(
            init_size.w as u32,
            init_size.h as u32,
            init_scale as f32,
        );
        state.sync_from_shell();
    }

    let mut damage_tracker = OutputDamageTracker::from_output(&state.output);

    let loop_handle = event_loop.handle();
    insert_wayland_socket_source(&loop_handle, listening_socket)?;
    insert_shell_maintenance_sources(&loop_handle, ipc_server, watcher)?;

    loop_handle
        .insert_source(winit_evt_loop, move |event, _, state| match event {
            WinitEvent::Resized { size, scale_factor } => {
                if size.w > 0 && size.h > 0 {
                    let mode = Mode {
                        size,
                        refresh: 60_000,
                    };
                    state.output.change_current_state(
                        Some(mode),
                        None,
                        Some(Scale::Fractional(scale_factor)),
                        None,
                    );
                    state.shell.resize_output(
                        size.w as u32,
                        size.h as u32,
                        scale_factor as f32,
                    );
                    layer_map_for_output(&state.output).arrange();
                    state.sync_from_shell();
                    state.needs_redraw = true;
                }
            }
            WinitEvent::Input(input_evt) => {
                state.process_input_event(input_evt);
            }
            WinitEvent::Focus(_) | WinitEvent::Redraw => {
                state.needs_redraw = true;
            }
            WinitEvent::CloseRequested => {
                info!("Host winit window requested close; shutting down compositor");
                state.should_exit = true;
            }
        })
        .map_err(|e| format!("Failed to insert Smithay WinitEventLoop source: {e}"))?;

    // Render initial frame immediately
    render_winit_frame(&mut backend, &mut damage_tracker, &mut state);

    while !state.should_exit {
        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .map_err(|e| format!("calloop dispatch error: {e}"))?;

        if let Err(err) = display.dispatch_clients(&mut state) {
            warn!("Error dispatching Wayland client requests: {err}");
        }

        if state.needs_redraw {
            state.needs_redraw = false;
            render_winit_frame(&mut backend, &mut damage_tracker, &mut state);
        }

        state.space.refresh();
        state.popups.cleanup();

        if let Err(err) = display.flush_clients() {
            warn!("Error flushing Wayland client buffers: {err}");
        }
    }

    // Terminate any child clients spawned during this session
    for mut child in state.spawned_children.drain(..) {
        let _ = child.kill();
        let _ = child.wait();
    }

    Ok(())
}

fn render_winit_frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: &mut OutputDamageTracker,
    state: &mut LuminaState,
) {
    let elements = state.collect_render_elements(backend.renderer());
    let age = backend.buffer_age().unwrap_or(0);

    let render_outcome = backend.bind().and_then(|(renderer, mut framebuffer)| {
        damage_tracker
            .render_output(
                renderer,
                &mut framebuffer,
                age,
                &elements,
                [0.043, 0.059, 0.098, 1.0],
            )
            .map(|res| res.damage.cloned())
            .map_err(|err| match err {
                smithay::backend::renderer::damage::Error::Rendering(e) => e.into(),
                smithay::backend::renderer::damage::Error::OutputNoMode(_) => {
                    smithay::backend::SwapBuffersError::TemporaryFailure(Box::new(
                        std::io::Error::other("Output has no mode configured"),
                    ))
                }
            })
    });

    match render_outcome {
        Ok(damage) => {
            if let Err(err) = backend.submit(damage.as_deref()) {
                warn!("Failed to submit EGL frame to winit surface: {err}");
            }
        }
        Err(err) => {
            error!("GLES frame render error: {err}");
        }
    }

    state.send_frame_callbacks();
}
