use renderer::{hit_test_resize_edge, hit_test_titlebar, TitlebarHit};
use wm_proto::{
    ActionCommand, AppsAction, Direction, Edges, Point, ResizeEdge, ShellAction, WindowId,
    WindowManagerEngine, WmAction, WorkspaceTarget,
};

use crate::daemon::LuminaDaemon;
use crate::shell_layout::ModalHit;

/// Backend-agnostic keyboard modifier state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    /// Encodes depressed XKB modifier mask (`Shift=1, Control=4, Mod1/Alt=8, Mod4/Super=64`).
    pub fn to_xkb_depressed_mask(self) -> u32 {
        let mut mask = 0u32;
        if self.shift {
            mask |= 1;
        }
        if self.ctrl {
            mask |= 4;
        }
        if self.alt {
            mask |= 8;
        }
        if self.logo {
            mask |= 64;
        }
        mask
    }
}

/// Backend-agnostic pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Other(u32),
}

impl PointerButton {
    pub fn to_evdev_code(self) -> u32 {
        match self {
            Self::Left => 0x110,
            Self::Right => 0x111,
            Self::Middle => 0x112,
            Self::Other(code) => code,
        }
    }

    pub fn from_evdev_code(code: u32) -> Self {
        match code {
            0x110 => Self::Left,
            0x111 => Self::Right,
            0x112 => Self::Middle,
            other => Self::Other(other),
        }
    }
}

/// Backend-agnostic logical key symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySymbol {
    Enter,
    Escape,
    Space,
    Tab,
    Backspace,
    Super,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    F(u8),
    Digit(u8),
    Char(String),
    Other,
}

impl KeySymbol {
    /// Normalized lowercase ASCII character if this key represents a single ASCII letter/symbol.
    pub fn ascii_lower_char(&self) -> Option<char> {
        match self {
            Self::Char(s) if s.len() == 1 => s.chars().next().map(|c| c.to_ascii_lowercase()),
            _ => None,
        }
    }
}

/// Normalized keyboard event passed into `InputRouter`.
#[derive(Clone, Debug)]
pub struct KeyInput {
    pub sym: KeySymbol,
    pub digit_code: Option<u8>,
    pub evdev_code: Option<u32>,
    pub pressed: bool,
    pub repeat: bool,
    pub mods: Modifiers,
}

/// Trigger pattern for a configurable keybinding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyPattern {
    Symbol(KeySymbol),
    AsciiChar(char),
    Digit(u8),
    FunctionKey(u8),
}

/// Single entry in the configurable `KeybindTable`.
#[derive(Clone, Debug)]
pub struct KeybindEntry {
    pub mods: Modifiers,
    pub pattern: KeyPattern,
    pub action: ActionCommand,
    pub close_launcher_on_trigger: bool,
}

/// Configurable keybinding table replacing hardcoded `match` chains.
#[derive(Clone, Debug)]
pub struct KeybindTable {
    entries: Vec<KeybindEntry>,
}

impl Default for KeybindTable {
    fn default() -> Self {
        let mut entries = Vec::new();

        // Alt + Space -> Toggle Launcher
        entries.push(KeybindEntry {
            mods: Modifiers {
                alt: true,
                ..Default::default()
            },
            pattern: KeyPattern::Symbol(KeySymbol::Space),
            action: ActionCommand::Shell(ShellAction::ToggleWidget {
                name: "launcher".to_string(),
                args: Default::default(),
            }),
            close_launcher_on_trigger: false,
        });

        // Alt + F4 -> Close focused window
        entries.push(KeybindEntry {
            mods: Modifiers {
                alt: true,
                ..Default::default()
            },
            pattern: KeyPattern::FunctionKey(4),
            action: ActionCommand::Wm(WmAction::CloseWindow { id: None }),
            close_launcher_on_trigger: false,
        });

        // Super + 1..9 -> Focus Workspace 1..9
        // Super + Shift + 1..9 -> Move Window to Workspace 1..9
        for idx in 1..=9u8 {
            entries.push(KeybindEntry {
                mods: Modifiers {
                    logo: true,
                    ..Default::default()
                },
                pattern: KeyPattern::Digit(idx),
                action: ActionCommand::Wm(WmAction::FocusWorkspace {
                    target: WorkspaceTarget::Index(idx as u32),
                }),
                close_launcher_on_trigger: true,
            });
            entries.push(KeybindEntry {
                mods: Modifiers {
                    logo: true,
                    shift: true,
                    ..Default::default()
                },
                pattern: KeyPattern::Digit(idx),
                action: ActionCommand::Wm(WmAction::MoveWindowToWorkspace {
                    win_id: None,
                    workspace: WorkspaceTarget::Index(idx as u32),
                }),
                close_launcher_on_trigger: true,
            });
        }

        // Super + Enter -> Launch Terminal
        entries.push(KeybindEntry {
            mods: Modifiers {
                logo: true,
                ..Default::default()
            },
            pattern: KeyPattern::Symbol(KeySymbol::Enter),
            action: ActionCommand::Apps(AppsAction::Launch {
                app_id: "org.gnome.Console".to_string(),
            }),
            close_launcher_on_trigger: true,
        });

        // Super + Q -> Close focused window
        entries.push(KeybindEntry {
            mods: Modifiers {
                logo: true,
                ..Default::default()
            },
            pattern: KeyPattern::AsciiChar('q'),
            action: ActionCommand::Wm(WmAction::CloseWindow { id: None }),
            close_launcher_on_trigger: true,
        });

        // Super + F -> Toggle Maximize, Super + Shift + F -> Toggle Fullscreen
        entries.push(KeybindEntry {
            mods: Modifiers {
                logo: true,
                ..Default::default()
            },
            pattern: KeyPattern::AsciiChar('f'),
            action: ActionCommand::Wm(WmAction::ToggleMaximize { id: None }),
            close_launcher_on_trigger: true,
        });
        entries.push(KeybindEntry {
            mods: Modifiers {
                logo: true,
                shift: true,
                ..Default::default()
            },
            pattern: KeyPattern::AsciiChar('f'),
            action: ActionCommand::Wm(WmAction::ToggleFullscreen { id: None }),
            close_launcher_on_trigger: true,
        });

        // Super + V -> Toggle Floating
        entries.push(KeybindEntry {
            mods: Modifiers {
                logo: true,
                ..Default::default()
            },
            pattern: KeyPattern::AsciiChar('v'),
            action: ActionCommand::Wm(WmAction::ToggleFloating { id: None }),
            close_launcher_on_trigger: true,
        });

        // Super + Space -> Cycle Layout Mode
        entries.push(KeybindEntry {
            mods: Modifiers {
                logo: true,
                ..Default::default()
            },
            pattern: KeyPattern::Symbol(KeySymbol::Space),
            action: ActionCommand::Wm(WmAction::CycleLayoutMode),
            close_launcher_on_trigger: true,
        });

        // Super + Arrow keys -> Focus / Move direction
        for (sym, dir) in [
            (KeySymbol::ArrowLeft, Direction::Left),
            (KeySymbol::ArrowRight, Direction::Right),
            (KeySymbol::ArrowUp, Direction::Up),
            (KeySymbol::ArrowDown, Direction::Down),
        ] {
            entries.push(KeybindEntry {
                mods: Modifiers {
                    logo: true,
                    ..Default::default()
                },
                pattern: KeyPattern::Symbol(sym.clone()),
                action: ActionCommand::Wm(WmAction::FocusDirection { dir }),
                close_launcher_on_trigger: true,
            });
            entries.push(KeybindEntry {
                mods: Modifiers {
                    logo: true,
                    shift: true,
                    ..Default::default()
                },
                pattern: KeyPattern::Symbol(sym),
                action: ActionCommand::Wm(WmAction::MoveWindowDirection { dir }),
                close_launcher_on_trigger: true,
            });
        }

        // Super + Tab & Alt + Tab -> Cycle Window Focus
        entries.push(KeybindEntry {
            mods: Modifiers {
                logo: true,
                ..Default::default()
            },
            pattern: KeyPattern::Symbol(KeySymbol::Tab),
            action: ActionCommand::Wm(WmAction::FocusDirection {
                dir: Direction::Right,
            }),
            close_launcher_on_trigger: true,
        });
        entries.push(KeybindEntry {
            mods: Modifiers {
                alt: true,
                ..Default::default()
            },
            pattern: KeyPattern::Symbol(KeySymbol::Tab),
            action: ActionCommand::Wm(WmAction::FocusDirection {
                dir: Direction::Right,
            }),
            close_launcher_on_trigger: false,
        });

        // Unmodified F1..F9 -> Switch Workspace 1..9
        for idx in 1..=9u8 {
            entries.push(KeybindEntry {
                mods: Modifiers::default(),
                pattern: KeyPattern::FunctionKey(idx),
                action: ActionCommand::Wm(WmAction::FocusWorkspace {
                    target: WorkspaceTarget::Index(idx as u32),
                }),
                close_launcher_on_trigger: false,
            });
        }

        Self { entries }
    }
}

impl KeybindTable {
    pub fn entries(&self) -> &[KeybindEntry] {
        &self.entries
    }

    pub fn bind(&mut self, entry: KeybindEntry) {
        self.entries.push(entry);
    }

    pub fn match_key(&self, key: &KeyInput) -> Option<&KeybindEntry> {
        let digit = key.digit_code.or(match key.sym {
            KeySymbol::Digit(d) => Some(d),
            _ => None,
        });
        let fn_key = match key.sym {
            KeySymbol::F(n) => Some(n),
            _ => None,
        };
        let ascii_ch = key.sym.ascii_lower_char();

        self.entries.iter().find(|entry| {
            if entry.mods != key.mods {
                return false;
            }
            match &entry.pattern {
                KeyPattern::Digit(d) => digit == Some(*d),
                KeyPattern::FunctionKey(n) => fn_key == Some(*n),
                KeyPattern::AsciiChar(c) => ascii_ch == Some(*c),
                KeyPattern::Symbol(s) => &key.sym == s,
            }
        })
    }
}

/// Interactive window move/resize grab state tracked by `InputRouter`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InteractiveGrabKind {
    Move {
        window_id: WindowId,
        start_pt: Point,
    },
    Resize {
        window_id: WindowId,
        edge: ResizeEdge,
        start_pt: Point,
    },
}

impl InteractiveGrabKind {
    pub fn start_pt(self) -> Point {
        match self {
            Self::Move { start_pt, .. } | Self::Resize { start_pt, .. } => start_pt,
        }
    }
}

/// Event that was not consumed by the shell/WM and should be forwarded to a Wayland client surface.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientInputForward {
    PointerMotion {
        window_id: WindowId,
        local_x: f32,
        local_y: f32,
    },
    PointerButton {
        window_id: WindowId,
        local_x: f32,
        local_y: f32,
        button_code: u32,
        pressed: bool,
    },
    PointerAxis {
        window_id: WindowId,
        dx: f32,
        dy: f32,
    },
    KeyboardFocus {
        window_id: Option<WindowId>,
    },
    Key {
        window_id: WindowId,
        evdev_code: u32,
        pressed: bool,
    },
}

/// Structured result of routing a pointer or keyboard event through `InputRouter`.
#[derive(Clone, Debug, Default)]
pub struct InputOutcome {
    pub consumed_by_shell: bool,
    pub redraw: bool,
    pub exit_requested: bool,
    pub client_forwards: Vec<ClientInputForward>,
    pub started_grab: Option<InteractiveGrabKind>,
    pub ended_grab: bool,
}

/// Backend-agnostic input router decoupling `LuminaDaemon` (`LuminaShell`) from `winit` and Smithay.
#[derive(Clone, Debug, Default)]
pub struct InputRouter {
    cursor_pos: Point,
    mods: Modifiers,
    active_grab: Option<InteractiveGrabKind>,
    pressed_client_win: Option<(WindowId, u32)>,
    keybinds: KeybindTable,
}

impl InputRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cursor_pos(&self) -> Point {
        self.cursor_pos
    }

    pub fn modifiers(&self) -> Modifiers {
        self.mods
    }

    pub fn set_modifiers(&mut self, mods: Modifiers) {
        self.mods = mods;
    }

    pub fn active_grab(&self) -> Option<InteractiveGrabKind> {
        self.active_grab
    }

    pub fn keybinds_mut(&mut self) -> &mut KeybindTable {
        &mut self.keybinds
    }

    /// Processes pointer motion in logical desktop coordinates.
    pub fn on_pointer_motion(
        &mut self,
        daemon: &mut LuminaDaemon,
        desktop_pt: Point,
    ) -> InputOutcome {
        self.cursor_pos = desktop_pt;
        let mut outcome = InputOutcome::default();

        // 1. Fast O(1) interactive window move/resize during active grab
        if let Some(grab) = self.active_grab {
            let start = grab.start_pt();
            let dx = desktop_pt.x - start.x;
            let dy = desktop_pt.y - start.y;
            outcome.consumed_by_shell = true;
            outcome.redraw = daemon.wm.move_interactive_grab_fast(dx, dy);
            return outcome;
        }

        let layout = daemon.shell_layout();
        let mut hover_changed = false;

        // 2. Update hover state on visible shell surfaces using declarative `ShellLayout`
        if let Some(launcher) = layout.layer("launcher") {
            if launcher.visible {
                hover_changed |= daemon.update_hover(
                    "launcher",
                    desktop_pt.x - launcher.rect.x,
                    desktop_pt.y - launcher.rect.y,
                );
            }
        }

        if let Some(qs) = layout.layer("quick_settings") {
            if qs.visible {
                hover_changed |= daemon.update_hover(
                    "quick_settings",
                    desktop_pt.x - qs.rect.x,
                    desktop_pt.y - qs.rect.y,
                );
            } else if let Some(notif) = layout.layer("notifications") {
                if daemon
                    .surfaces
                    .get("notifications")
                    .map(|s| s.visible)
                    .unwrap_or(false)
                {
                    hover_changed |= daemon.update_hover(
                        "notifications",
                        desktop_pt.x - notif.rect.x,
                        desktop_pt.y - notif.rect.y,
                    );
                }
            }
        }

        if let Some(topbar) = layout.layer("topbar") {
            if topbar.visible {
                hover_changed |= daemon.update_hover(
                    "topbar",
                    desktop_pt.x - topbar.rect.x,
                    desktop_pt.y - topbar.rect.y,
                );
            }
        }

        if let Some(dock) = layout.layer("dock") {
            if dock.visible {
                hover_changed |= daemon.update_hover(
                    "dock",
                    desktop_pt.x - dock.rect.x,
                    desktop_pt.y - dock.rect.y,
                );
            }
        }

        outcome.redraw = hover_changed;

        // 3. Forward pointer motion to topmost client window if no modal overlay is open
        if !layout.any_modal_open() {
            let usable = layout.usable_rect();
            let placements = daemon.wm.compute_window_placements(usable);
            for p in placements.iter().rev() {
                if p.content_rect.contains(desktop_pt) {
                    let local_x = desktop_pt.x - p.content_rect.x;
                    let local_y = desktop_pt.y - p.content_rect.y;
                    outcome
                        .client_forwards
                        .push(ClientInputForward::PointerMotion {
                            window_id: p.window_id,
                            local_x,
                            local_y,
                        });
                    break;
                }
            }
        }

        outcome
    }

    /// Processes a pointer button press or release at `desktop_pt`.
    pub fn on_pointer_button(
        &mut self,
        daemon: &mut LuminaDaemon,
        desktop_pt: Point,
        button: PointerButton,
        pressed: bool,
    ) -> InputOutcome {
        self.cursor_pos = desktop_pt;
        let btn_code = button.to_evdev_code();
        let mut outcome = InputOutcome::default();

        if !pressed {
            if matches!(button, PointerButton::Left | PointerButton::Right)
                && self.active_grab.take().is_some()
            {
                daemon.wm.end_interactive_grab();
                let patches = daemon.wm.export_state_patches();
                daemon.apply_state_patches(patches);
                outcome.consumed_by_shell = true;
                outcome.ended_grab = true;
                outcome.redraw = true;
            }

            if let Some((win_id, pressed_code)) = self.pressed_client_win.take() {
                let usable = daemon.shell_layout().usable_rect();
                let placements = daemon.wm.compute_window_placements(usable);
                let (local_x, local_y) = placements
                    .iter()
                    .find(|p| p.window_id == win_id)
                    .map(|p| (desktop_pt.x - p.content_rect.x, desktop_pt.y - p.content_rect.y))
                    .unwrap_or((0.0, 0.0));
                outcome
                    .client_forwards
                    .push(ClientInputForward::PointerButton {
                        window_id: win_id,
                        local_x,
                        local_y,
                        button_code: pressed_code,
                        pressed: false,
                    });
                outcome.redraw = true;
            }
            return outcome;
        }

        let layout = daemon.shell_layout();

        if button == PointerButton::Left {
            // 1. Modal overlays (`launcher` and `quick_settings`): click inside dispatches DOM click,
            // click outside dismisses the modal.
            if let Some(modal_hit) = layout.hit_test_modal(desktop_pt) {
                match modal_hit {
                    ModalHit::Inside {
                        surface_name,
                        local_pt,
                    } => {
                        let _ = daemon.click_surface(&surface_name, local_pt.x, local_pt.y);
                    }
                    ModalHit::Outside { widget_name } => {
                        let _ = daemon.dispatch_action(ActionCommand::Shell(
                            ShellAction::CloseWidget { name: widget_name },
                        ));
                    }
                }
                outcome.consumed_by_shell = true;
                outcome.redraw = true;
                outcome.exit_requested = daemon
                    .get_state_path(Some("sys.session.exit_requested"))
                    .as_bool();
                return outcome;
            }

            // 2. Non-modal top layer surfaces (`notifications`, `topbar`, `dock`)
            let notif_visible = daemon
                .surfaces
                .get("notifications")
                .map(|s| s.visible)
                .unwrap_or(false);
            if let Some((surf_name, local_pt)) =
                layout.hit_test_top_layer(desktop_pt, notif_visible)
            {
                let surf_name_owned = surf_name.to_string();
                let _ = daemon.click_surface(&surf_name_owned, local_pt.x, local_pt.y);
                outcome.consumed_by_shell = true;
                outcome.redraw = true;
                outcome.exit_requested = daemon
                    .get_state_path(Some("sys.session.exit_requested"))
                    .as_bool();
                return outcome;
            }
        }

        // 3. Window Frame / Server-Side Titlebar buttons, 8-edge resize & Client Area
        let usable = layout.usable_rect();
        let placements = daemon.wm.compute_window_placements(usable);

        for p in placements.iter().rev() {
            if p.frame_rect.inset(Edges::all(-4.0)).contains(desktop_pt) {
                if !p.is_focused {
                    let _ = daemon.dispatch_action(ActionCommand::Wm(WmAction::FocusWindow {
                        id: p.window_id,
                    }));
                }
                outcome
                    .client_forwards
                    .push(ClientInputForward::KeyboardFocus {
                        window_id: Some(p.window_id),
                    });

                // Super + Left Click anywhere inside window starts interactive move
                if self.mods.logo && button == PointerButton::Left {
                    let _ = daemon.wm.dispatch_wm_action(&WmAction::BeginInteractiveMove {
                        win_id: p.window_id,
                    });
                    let grab = InteractiveGrabKind::Move {
                        window_id: p.window_id,
                        start_pt: desktop_pt,
                    };
                    self.active_grab = Some(grab);
                    outcome.started_grab = Some(grab);
                    outcome.consumed_by_shell = true;
                    outcome.redraw = true;
                    return outcome;
                }

                // Super + Right Click anywhere inside window starts interactive resize (BottomRight)
                if self.mods.logo && button == PointerButton::Right {
                    let _ = daemon.wm.dispatch_wm_action(&WmAction::BeginInteractiveResize {
                        win_id: p.window_id,
                        edge: ResizeEdge::BottomRight,
                    });
                    let grab = InteractiveGrabKind::Resize {
                        window_id: p.window_id,
                        edge: ResizeEdge::BottomRight,
                        start_pt: desktop_pt,
                    };
                    self.active_grab = Some(grab);
                    outcome.started_grab = Some(grab);
                    outcome.consumed_by_shell = true;
                    outcome.redraw = true;
                    return outcome;
                }

                // Normal Left Click: check titlebar buttons, 8-edge resize border, or titlebar drag
                if button == PointerButton::Left {
                    let tb_hit = hit_test_titlebar(p, desktop_pt);
                    match tb_hit {
                        Some(TitlebarHit::Float) => {
                            let _ = daemon.dispatch_action(ActionCommand::Wm(
                                WmAction::ToggleFloating {
                                    id: Some(p.window_id),
                                },
                            ));
                            outcome.consumed_by_shell = true;
                            outcome.redraw = true;
                            return outcome;
                        }
                        Some(TitlebarHit::Maximize) => {
                            let _ = daemon.dispatch_action(ActionCommand::Wm(
                                WmAction::ToggleMaximize {
                                    id: Some(p.window_id),
                                },
                            ));
                            outcome.consumed_by_shell = true;
                            outcome.redraw = true;
                            return outcome;
                        }
                        Some(TitlebarHit::Close) => {
                            let _ = daemon.dispatch_action(ActionCommand::Wm(
                                WmAction::CloseWindow {
                                    id: Some(p.window_id),
                                },
                            ));
                            outcome.consumed_by_shell = true;
                            outcome.redraw = true;
                            return outcome;
                        }
                        _ => {}
                    }

                    if let Some(edge) = hit_test_resize_edge(p, desktop_pt) {
                        let _ = daemon.wm.dispatch_wm_action(&WmAction::BeginInteractiveResize {
                            win_id: p.window_id,
                            edge,
                        });
                        let grab = InteractiveGrabKind::Resize {
                            window_id: p.window_id,
                            edge,
                            start_pt: desktop_pt,
                        };
                        self.active_grab = Some(grab);
                        outcome.started_grab = Some(grab);
                        outcome.consumed_by_shell = true;
                        outcome.redraw = true;
                        return outcome;
                    }

                    if matches!(tb_hit, Some(TitlebarHit::DragArea)) {
                        let _ = daemon.wm.dispatch_wm_action(&WmAction::BeginInteractiveMove {
                            win_id: p.window_id,
                        });
                        let grab = InteractiveGrabKind::Move {
                            window_id: p.window_id,
                            start_pt: desktop_pt,
                        };
                        self.active_grab = Some(grab);
                        outcome.started_grab = Some(grab);
                        outcome.consumed_by_shell = true;
                        outcome.redraw = true;
                        return outcome;
                    }
                }

                if p.content_rect.contains(desktop_pt) {
                    let local_x = desktop_pt.x - p.content_rect.x;
                    let local_y = desktop_pt.y - p.content_rect.y;
                    self.pressed_client_win = Some((p.window_id, btn_code));
                    outcome
                        .client_forwards
                        .push(ClientInputForward::PointerButton {
                            window_id: p.window_id,
                            local_x,
                            local_y,
                            button_code: btn_code,
                            pressed: true,
                        });
                }
                outcome.redraw = true;
                return outcome;
            }
        }

        outcome
    }

    /// Processes a scroll wheel / touchpad axis event at `desktop_pt`.
    pub fn on_pointer_axis(
        &mut self,
        daemon: &mut LuminaDaemon,
        desktop_pt: Point,
        dx: f32,
        dy: f32,
    ) -> InputOutcome {
        let mut outcome = InputOutcome::default();
        if dx.abs() < 0.01 && dy.abs() < 0.01 {
            return outcome;
        }
        let scroll_up = dy > 0.0;
        let layout = daemon.shell_layout();

        if let Some(topbar) = layout.layer("topbar") {
            if topbar.visible && topbar.hit_rect.contains(desktop_pt) {
                let _ = daemon.scroll_surface(
                    "topbar",
                    desktop_pt.x - topbar.rect.x,
                    desktop_pt.y - topbar.rect.y,
                    scroll_up,
                );
                outcome.consumed_by_shell = true;
                outcome.redraw = true;
                return outcome;
            }
        }

        let usable = layout.usable_rect();
        let placements = daemon.wm.compute_window_placements(usable);
        for p in placements.iter().rev() {
            if p.content_rect.contains(desktop_pt) {
                // Wayland axis value: positive is down/right
                outcome
                    .client_forwards
                    .push(ClientInputForward::PointerAxis {
                        window_id: p.window_id,
                        dx: -dx,
                        dy: -dy,
                    });
                outcome.redraw = true;
                break;
            }
        }

        outcome
    }

    /// Processes a keyboard key press/release through the configurable `KeybindTable`,
    /// active modal overlays, built-in terminal, or Wayland client surface.
    pub fn on_key_input(&mut self, daemon: &mut LuminaDaemon, key: KeyInput) -> InputOutcome {
        self.mods = key.mods;
        let mut outcome = InputOutcome::default();

        let layout = daemon.shell_layout();
        let launcher_open = layout.is_launcher_open();
        let qs_open = layout.is_quick_settings_open();

        if key.pressed {
            // 1. Check configurable global keybindings
            if let Some(binding) = self.keybinds.match_key(&key).cloned() {
                if binding.close_launcher_on_trigger && launcher_open {
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "launcher".to_string(),
                    }));
                }
                let _ = daemon.dispatch_action(binding.action);
                outcome.consumed_by_shell = true;
                outcome.redraw = true;
                outcome.exit_requested = daemon
                    .get_state_path(Some("sys.session.exit_requested"))
                    .as_bool();
                return outcome;
            }

            // 2. Standalone Super key toggles Launcher; Escape closes open modals
            match &key.sym {
                KeySymbol::Super => {
                    let _ =
                        daemon.dispatch_action(ActionCommand::Shell(ShellAction::ToggleWidget {
                            name: "launcher".to_string(),
                            args: Default::default(),
                        }));
                    outcome.consumed_by_shell = true;
                    outcome.redraw = true;
                    return outcome;
                }
                KeySymbol::Escape if launcher_open || qs_open => {
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "launcher".to_string(),
                    }));
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "quick_settings".to_string(),
                    }));
                    outcome.consumed_by_shell = true;
                    outcome.redraw = true;
                    return outcome;
                }
                _ => {}
            }
        }

        // 3. When Launcher modal is open, capture keyboard exclusively for app search/navigation
        if launcher_open {
            outcome.consumed_by_shell = true;
            if !key.pressed {
                return outcome;
            }
            match &key.sym {
                KeySymbol::ArrowDown => {
                    let _ = daemon.dispatch_action(ActionCommand::Apps(
                        AppsAction::SelectRelative { delta: 1 },
                    ));
                    outcome.redraw = true;
                }
                KeySymbol::ArrowUp => {
                    let _ = daemon.dispatch_action(ActionCommand::Apps(
                        AppsAction::SelectRelative { delta: -1 },
                    ));
                    outcome.redraw = true;
                }
                KeySymbol::Enter => {
                    let _ = daemon.dispatch_action(ActionCommand::Apps(AppsAction::LaunchSelected));
                    let _ = daemon.dispatch_action(ActionCommand::Shell(ShellAction::CloseWidget {
                        name: "launcher".to_string(),
                    }));
                    outcome.redraw = true;
                }
                KeySymbol::Space => {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.push(' ');
                    let _ = daemon
                        .dispatch_action(ActionCommand::Apps(AppsAction::SetQuery { query }));
                    outcome.redraw = true;
                }
                KeySymbol::Char(c) => {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.push_str(c);
                    let _ = daemon
                        .dispatch_action(ActionCommand::Apps(AppsAction::SetQuery { query }));
                    outcome.redraw = true;
                }
                KeySymbol::Backspace => {
                    let mut query = daemon
                        .get_state_path(Some("apps.query"))
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    query.pop();
                    let _ = daemon
                        .dispatch_action(ActionCommand::Apps(AppsAction::SetQuery { query }));
                    outcome.redraw = true;
                }
                _ => {}
            }
            return outcome;
        }

        // 4. Route keyboard input to focused window (built-in terminal or Wayland client)
        if let Some(focused_id) = daemon.wm.focused_window_id() {
            let is_builtin_term = daemon
                .wm
                .windows()
                .into_iter()
                .find(|w| w.id == focused_id)
                .map(|w| w.app_id == "org.lumina.terminal")
                .unwrap_or(false);

            if is_builtin_term {
                outcome.consumed_by_shell = true;
                if !key.pressed {
                    return outcome;
                }
                match &key.sym {
                    KeySymbol::Enter => {
                        daemon.terminal_execute();
                        outcome.redraw = true;
                    }
                    KeySymbol::Space => {
                        daemon.terminal_push_str(" ");
                        outcome.redraw = true;
                    }
                    KeySymbol::Char(c) => {
                        daemon.terminal_push_str(c);
                        outcome.redraw = true;
                    }
                    KeySymbol::Backspace => {
                        daemon.terminal_backspace();
                        outcome.redraw = true;
                    }
                    _ => {}
                }
                return outcome;
            }

            if !key.repeat {
                if let Some(evdev_code) = key.evdev_code {
                    outcome.client_forwards.push(ClientInputForward::Key {
                        window_id: focused_id,
                        evdev_code,
                        pressed: key.pressed,
                    });
                    outcome.redraw = true;
                }
            }
        }

        outcome
    }

    /// Forwards any `ClientInputForward` events to the legacy `RealWaylandServer` (when active).
    pub fn apply_forwards_to_legacy_server(
        daemon: &mut LuminaDaemon,
        forwards: &[ClientInputForward],
    ) {
        let Some(ref mut wl) = daemon.wayland_server else {
            return;
        };
        for fwd in forwards {
            match *fwd {
                ClientInputForward::PointerMotion {
                    window_id,
                    local_x,
                    local_y,
                } => {
                    wl.send_pointer_motion(window_id, local_x, local_y);
                }
                ClientInputForward::PointerButton {
                    window_id,
                    local_x,
                    local_y,
                    button_code,
                    pressed,
                } => {
                    wl.send_pointer_button_code(window_id, local_x, local_y, button_code, pressed);
                }
                ClientInputForward::PointerAxis { window_id, dx, dy } => {
                    wl.send_pointer_axis(window_id, dx, dy);
                }
                ClientInputForward::KeyboardFocus { window_id } => {
                    wl.focus_window_keyboard(window_id);
                }
                ClientInputForward::Key {
                    window_id,
                    evdev_code,
                    pressed,
                } => {
                    wl.send_key(window_id, evdev_code, pressed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keybind_table_and_launcher_capture() {
        let mut daemon = LuminaDaemon::new();
        let mut router = InputRouter::new();

        // Super + 3 switches to workspace 3
        let out = router.on_key_input(
            &mut daemon,
            KeyInput {
                sym: KeySymbol::Digit(3),
                digit_code: Some(3),
                evdev_code: Some(4),
                pressed: true,
                repeat: false,
                mods: Modifiers {
                    logo: true,
                    ..Default::default()
                },
            },
        );
        assert!(out.consumed_by_shell);
        assert_eq!(daemon.wm.active_workspace_id(), 3);

        // Press Super to open launcher
        let out_super = router.on_key_input(
            &mut daemon,
            KeyInput {
                sym: KeySymbol::Super,
                digit_code: None,
                evdev_code: Some(125),
                pressed: true,
                repeat: false,
                mods: Modifiers::default(),
            },
        );
        assert!(out_super.consumed_by_shell);
        assert!(daemon.shell_layout().is_launcher_open());

        // Type "fi" while launcher is open -> updates apps.query
        let _ = router.on_key_input(
            &mut daemon,
            KeyInput {
                sym: KeySymbol::Char("f".to_string()),
                digit_code: None,
                evdev_code: Some(33),
                pressed: true,
                repeat: false,
                mods: Modifiers::default(),
            },
        );
        assert_eq!(
            daemon.get_state_path(Some("apps.query")).as_str(),
            Some("f")
        );

        // Escape closes launcher
        let _ = router.on_key_input(
            &mut daemon,
            KeyInput {
                sym: KeySymbol::Escape,
                digit_code: None,
                evdev_code: Some(1),
                pressed: true,
                repeat: false,
                mods: Modifiers::default(),
            },
        );
        assert!(!daemon.shell_layout().is_launcher_open());
    }
}
