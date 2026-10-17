use wayland_client::{
    event_created_child,
    protocol::wl_registry::{self, WlRegistry},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

#[derive(Clone, Debug)]
struct ModeInfo {
    proxy: ZwlrOutputModeV1,
    width: i32,
    height: i32,
    refresh: i32,
    preferred: bool,
}

#[derive(Clone, Debug)]
struct HeadInfo {
    proxy: ZwlrOutputHeadV1,
    modes: Vec<ZwlrOutputModeV1>,
}

#[derive(Default)]
struct OutputConfigState {
    manager: Option<ZwlrOutputManagerV1>,
    done_serial: Option<u32>,
    heads: Vec<HeadInfo>,
    modes: Vec<ModeInfo>,
    succeeded: bool,
}

fn find_exact_mode(modes: &[ModeInfo], w: i32, h: i32) -> Option<&ModeInfo> {
    modes
        .iter()
        .filter(|m| m.width == w && m.height == h)
        .max_by_key(|m| (m.refresh, m.preferred))
}

fn select_best_mode(modes: &[ModeInfo], target_w: i32, target_h: i32) -> Option<&ModeInfo> {
    if let Some(m) = find_exact_mode(modes, target_w, target_h) {
        return Some(m);
    }
    for (fw, fh) in [(1920, 1080), (1920, 1200), (1600, 900)] {
        if let Some(m) = find_exact_mode(modes, fw, fh) {
            return Some(m);
        }
    }
    if let Some(m) = modes
        .iter()
        .filter(|m| m.width > 0 && m.height > 0 && m.width <= 1920 && m.height <= 1200)
        .max_by_key(|m| ((m.width as i64) * (m.height as i64), m.refresh, m.preferred))
    {
        return Some(m);
    }
    modes
        .iter()
        .filter(|m| m.width > 0 && m.height > 0)
        .max_by_key(|m| (m.preferred, m.refresh, (m.width as i64) * (m.height as i64)))
}

/// Connects to the parent Wayland compositor and requests setting the output mode
/// to `(target_w, target_h)` at scale `1.0` via `zwlr_output_manager_v1`.
pub fn request_parent_wayland_mode(target_w: i32, target_h: i32) -> Result<bool, String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("Failed to connect to parent Wayland display: {e}"))?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let _registry = display.get_registry(&qh, ());
    let mut state = OutputConfigState::default();

    event_queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland roundtrip 1 failed: {e}"))?;
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland roundtrip 2 failed: {e}"))?;

    let (Some(ref manager), Some(serial)) = (&state.manager, state.done_serial) else {
        return Ok(false);
    };

    if state.heads.is_empty() {
        return Ok(false);
    }

    let config = manager.create_configuration(serial, &qh, ());
    for head in &state.heads {
        let head_modes: Vec<ModeInfo> = state
            .modes
            .iter()
            .filter(|m| head.modes.iter().any(|hm| hm == &m.proxy))
            .cloned()
            .collect();

        let head_cfg = config.enable_head(&head.proxy, &qh, ());
        if let Some(chosen_mode) = select_best_mode(&head_modes, target_w, target_h) {
            head_cfg.set_mode(&chosen_mode.proxy);
        } else {
            head_cfg.set_custom_mode(target_w, target_h, 60_000);
        }
        head_cfg.set_position(0, 0);
        if head_cfg.version() >= 1 {
            head_cfg.set_scale(1.0);
        }
    }
    config.apply();

    event_queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland roundtrip 3 failed: {e}"))?;

    Ok(state.succeeded)
}

impl Dispatch<WlRegistry, ()> for OutputConfigState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == ZwlrOutputManagerV1::interface().name && state.manager.is_none() {
                let bind_ver = version.clamp(1, 4);
                let mgr = registry.bind::<ZwlrOutputManagerV1, _, _>(name, bind_ver, qh, ());
                state.manager = Some(mgr);
            }
        }
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for OutputConfigState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.heads.push(HeadInfo {
                    proxy: head,
                    modes: Vec::new(),
                });
            }
            zwlr_output_manager_v1::Event::Done { serial } => {
                state.done_serial = Some(serial);
            }
            zwlr_output_manager_v1::Event::Finished => {}
            _ => {}
        }
    }

    event_created_child!(OutputConfigState, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for OutputConfigState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_output_head_v1::Event::Mode { mode } = event {
            if let Some(head) = state.heads.iter_mut().find(|h| &h.proxy == proxy) {
                head.modes.push(mode.clone());
            }
            state.modes.push(ModeInfo {
                proxy: mode,
                width: 0,
                height: 0,
                refresh: 0,
                preferred: false,
            });
        }
    }

    event_created_child!(OutputConfigState, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for OutputConfigState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(mode_info) = state.modes.iter_mut().find(|m| &m.proxy == proxy) else {
            return;
        };
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => {
                mode_info.width = width;
                mode_info.height = height;
            }
            zwlr_output_mode_v1::Event::Refresh { refresh } => {
                mode_info.refresh = refresh;
            }
            zwlr_output_mode_v1::Event::Preferred => {
                mode_info.preferred = true;
            }
            zwlr_output_mode_v1::Event::Finished => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for OutputConfigState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_configuration_v1::Event::Succeeded => {
                state.succeeded = true;
                proxy.destroy();
            }
            zwlr_output_configuration_v1::Event::Failed
            | zwlr_output_configuration_v1::Event::Cancelled => {
                proxy.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for OutputConfigState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrOutputConfigurationHeadV1,
        _event: <ZwlrOutputConfigurationHeadV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}
