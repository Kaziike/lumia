use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::WindowSurfaceType,
    input::{
        keyboard::{keysyms, FilterResult, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};
use wm_proto::Point as ProtoPoint;

use crate::{
    shell_bridge::{ShellKeyInput, ShellKeySymbol, ShellModifiers, ShellPointerButton},
    state::LuminaState,
};

impl LuminaState {
    /// Find the topmost client `WlSurface` under `point` (in logical output coordinates),
    /// returning `(surface, surface_origin_in_global_logical_coords)`.
    pub fn surface_under_pointer(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.shell.any_modal_open() {
            return None;
        }
        let usable = self.shell.usable_rect();
        if !usable.contains(ProtoPoint {
            x: point.x as f32,
            y: point.y as f32,
        }) {
            return None;
        }

        self.space
            .element_under(point)
            .and_then(|(window, win_loc)| {
                window
                    .surface_under(point - win_loc.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surf_loc)| (surface, (surf_loc + win_loc).to_f64()))
            })
    }

    /// Process a generic Smithay `InputEvent<B>` from any backend (`winit`, `libinput`, etc.).
    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let key_state = event.state();
                let keycode = event.key_code();

                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };

                keyboard.input::<(), _>(
                    self,
                    keycode,
                    key_state,
                    serial,
                    time,
                    |state, modifiers, handle| {
                        let shell_mods = to_shell_modifiers(modifiers);
                        state.shell.set_modifiers(shell_mods);

                        let key_input = to_shell_key_input(&handle, key_state, shell_mods);
                        let outcome = state.shell.on_key_input(key_input);
                        let consumed = outcome.consumed_by_shell;
                        state.apply_shell_outcome(outcome);

                        if consumed {
                            FilterResult::Intercept(())
                        } else {
                            FilterResult::Forward
                        }
                    },
                );
            }

            InputEvent::PointerMotionAbsolute { event } => {
                let (out_w, out_h) = self.shell.output_size();
                let pos = event.position_transformed((out_w as i32, out_h as i32).into());
                self.pointer_location = pos;

                let outcome = self.shell.on_pointer_motion(ProtoPoint {
                    x: pos.x as f32,
                    y: pos.y as f32,
                });
                let consumed = outcome.consumed_by_shell;
                self.apply_shell_outcome(outcome);

                if let Some(pointer) = self.seat.get_pointer() {
                    let serial = SERIAL_COUNTER.next_serial();
                    let time = Event::time_msec(&event);
                    let under = if consumed {
                        None
                    } else {
                        self.surface_under_pointer(pos)
                    };
                    pointer.motion(
                        self,
                        under,
                        &MotionEvent {
                            location: pos,
                            serial,
                            time,
                        },
                    );
                    pointer.frame(self);
                }
            }

            InputEvent::PointerMotion { event } => {
                let (out_w, out_h) = self.shell.output_size();
                let delta = event.delta();
                self.pointer_location.x =
                    (self.pointer_location.x + delta.x).clamp(0.0, (out_w.saturating_sub(1)) as f64);
                self.pointer_location.y =
                    (self.pointer_location.y + delta.y).clamp(0.0, (out_h.saturating_sub(1)) as f64);
                let pos = self.pointer_location;

                let outcome = self.shell.on_pointer_motion(ProtoPoint {
                    x: pos.x as f32,
                    y: pos.y as f32,
                });
                let consumed = outcome.consumed_by_shell;
                self.apply_shell_outcome(outcome);

                if let Some(pointer) = self.seat.get_pointer() {
                    let serial = SERIAL_COUNTER.next_serial();
                    let time = Event::time_msec(&event);
                    let under = if consumed {
                        None
                    } else {
                        self.surface_under_pointer(pos)
                    };
                    pointer.motion(
                        self,
                        under.clone(),
                        &MotionEvent {
                            location: pos,
                            serial,
                            time,
                        },
                    );
                    pointer.relative_motion(
                        self,
                        under,
                        &RelativeMotionEvent {
                            delta,
                            delta_unaccel: event.delta_unaccel(),
                            utime: event.time(),
                        },
                    );
                    pointer.frame(self);
                }
            }

            InputEvent::PointerButton { event } => {
                let pressed = event.state() == ButtonState::Pressed;
                let button_code = event.button_code();
                let shell_btn = match button_code {
                    0x110 => ShellPointerButton::Left,
                    0x111 => ShellPointerButton::Right,
                    0x112 => ShellPointerButton::Middle,
                    other => ShellPointerButton::Other(other),
                };

                let pos = self.pointer_location;
                let outcome = self.shell.on_pointer_button(
                    ProtoPoint {
                        x: pos.x as f32,
                        y: pos.y as f32,
                    },
                    shell_btn,
                    pressed,
                );
                let consumed = outcome.consumed_by_shell;
                self.apply_shell_outcome(outcome);

                if !consumed {
                    if let Some(pointer) = self.seat.get_pointer() {
                        let serial = SERIAL_COUNTER.next_serial();
                        let time = Event::time_msec(&event);
                        pointer.button(
                            self,
                            &ButtonEvent {
                                button: button_code,
                                state: event.state(),
                                serial,
                                time,
                            },
                        );
                        pointer.frame(self);
                    }
                }
            }

            InputEvent::PointerAxis { event } => {
                let horizontal = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0);
                let vertical = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0);

                let pos = self.pointer_location;
                let outcome = self.shell.on_pointer_axis(
                    ProtoPoint {
                        x: pos.x as f32,
                        y: pos.y as f32,
                    },
                    horizontal as f32,
                    vertical as f32,
                );
                let consumed = outcome.consumed_by_shell;
                self.apply_shell_outcome(outcome);

                if !consumed {
                    if let Some(pointer) = self.seat.get_pointer() {
                        let source = event.source();
                        let mut frame = AxisFrame::new(Event::time_msec(&event)).source(source);
                        if horizontal != 0.0 {
                            frame = frame.value(Axis::Horizontal, horizontal);
                            if let Some(v120) = event.amount_v120(Axis::Horizontal) {
                                frame = frame.v120(Axis::Horizontal, v120 as i32);
                            }
                        }
                        if vertical != 0.0 {
                            frame = frame.value(Axis::Vertical, vertical);
                            if let Some(v120) = event.amount_v120(Axis::Vertical) {
                                frame = frame.v120(Axis::Vertical, v120 as i32);
                            }
                        }
                        if source == AxisSource::Finger {
                            if event.amount(Axis::Horizontal) == Some(0.0) {
                                frame = frame.stop(Axis::Horizontal);
                            }
                            if event.amount(Axis::Vertical) == Some(0.0) {
                                frame = frame.stop(Axis::Vertical);
                            }
                        }
                        pointer.axis(self, frame);
                        pointer.frame(self);
                    }
                }
            }

            _ => {}
        }
    }
}

fn to_shell_modifiers(mods: &ModifiersState) -> ShellModifiers {
    ShellModifiers {
        shift: mods.shift,
        ctrl: mods.ctrl,
        alt: mods.alt,
        logo: mods.logo,
    }
}

fn to_shell_key_input(
    handle: &KeysymHandle<'_>,
    state: KeyState,
    mods: ShellModifiers,
) -> ShellKeyInput {
    let raw_code = handle.raw_code().raw();
    let evdev_code = raw_code.checked_sub(8);

    // Physical top-row digits 1..=9 map to evdev keycodes 2..=10 (XKB keycodes 10..=18)
    let digit_code = match evdev_code {
        Some(code @ 2..=10) => Some((code - 1) as u8),
        _ => None,
    };

    let sym_val = handle
        .raw_latin_sym_or_raw_current_sym()
        .unwrap_or_else(|| handle.modified_sym());
    let modified_sym = handle.modified_sym();

    let sym = match sym_val.raw() {
        keysyms::KEY_Return | keysyms::KEY_KP_Enter => ShellKeySymbol::Enter,
        keysyms::KEY_Escape => ShellKeySymbol::Escape,
        keysyms::KEY_space => ShellKeySymbol::Space,
        keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab => ShellKeySymbol::Tab,
        keysyms::KEY_BackSpace => ShellKeySymbol::Backspace,
        keysyms::KEY_Super_L | keysyms::KEY_Super_R | keysyms::KEY_Meta_L | keysyms::KEY_Meta_R => {
            ShellKeySymbol::Super
        }
        keysyms::KEY_Left => ShellKeySymbol::ArrowLeft,
        keysyms::KEY_Right => ShellKeySymbol::ArrowRight,
        keysyms::KEY_Up => ShellKeySymbol::ArrowUp,
        keysyms::KEY_Down => ShellKeySymbol::ArrowDown,
        raw @ keysyms::KEY_F1..=keysyms::KEY_F12 => {
            ShellKeySymbol::F((raw - keysyms::KEY_F1 + 1) as u8)
        }
        raw @ keysyms::KEY_1..=keysyms::KEY_9 if !mods.shift => {
            ShellKeySymbol::Digit((raw - keysyms::KEY_1 + 1) as u8)
        }
        _ => {
            if let Some(ch) = modified_sym.key_char().or_else(|| sym_val.key_char()) {
                if !ch.is_control() {
                    ShellKeySymbol::Char(ch.to_string())
                } else {
                    ShellKeySymbol::Other
                }
            } else {
                ShellKeySymbol::Other
            }
        }
    };

    ShellKeyInput {
        sym,
        digit_code,
        evdev_code,
        pressed: state == KeyState::Pressed,
        repeat: false,
        mods,
    }
}
