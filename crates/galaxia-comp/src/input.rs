//! Keyboard shortcuts and pointer input.
//!
//! Shortcuts use Alt (or Super) so they work while nested inside another desktop.
//!   Alt+Enter       open terminal         Alt+Q          close window
//!   Alt+Left/Right  focus column          Alt+H / Alt+L  same, vim style
//!   Alt+R           cycle column width    Alt+V          float / unfloat
//!   Alt+[           stack into left column
//!   Alt+Shift+E     quit Galaxia
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::SERIAL_COUNTER,
};

use crate::state::{spawn, terminal_command, Galaxia};

#[derive(Debug, Clone, Copy)]
enum Action {
    Terminal,
    Close,
    FocusLeft,
    FocusRight,
    CycleWidth,
    ToggleFloat,
    ConsumeLeft,
    Quit,
}

fn action_for(sym: Keysym, shift: bool) -> Option<Action> {
    Some(match sym {
        Keysym::Return => Action::Terminal,
        Keysym::q | Keysym::Q => Action::Close,
        Keysym::Left | Keysym::h | Keysym::H => Action::FocusLeft,
        Keysym::Right | Keysym::l | Keysym::L => Action::FocusRight,
        Keysym::r | Keysym::R => Action::CycleWidth,
        Keysym::v | Keysym::V => Action::ToggleFloat,
        Keysym::bracketleft => Action::ConsumeLeft,
        Keysym::e | Keysym::E if shift => Action::Quit,
        _ => return None,
    })
}

impl Galaxia {
    fn run(&mut self, action: Action) {
        match action {
            Action::Terminal => spawn(&terminal_command()),
            Action::Close => {
                if let Some(t) = self.focused_id().and_then(|id| self.window(id)).and_then(|w| w.toplevel().cloned()) {
                    t.send_close();
                }
            }
            Action::FocusLeft => {
                self.workspace.focus_left();
                self.relayout_and_focus();
            }
            Action::FocusRight => {
                self.workspace.focus_right();
                self.relayout_and_focus();
            }
            Action::CycleWidth => {
                self.workspace.cycle_width();
                self.relayout();
            }
            Action::ToggleFloat => {
                if let Some(id) = self.focused_id() {
                    let (w, h) = self.output_size();
                    self.workspace.toggle_float(id, w, h);
                    self.relayout();
                    self.focus(Some(id));
                }
            }
            Action::ConsumeLeft => {
                self.workspace.consume_left();
                self.relayout();
            }
            Action::Quit => self.loop_signal.stop(),
        }
    }

    /// Keyboard navigation always moves focus into the strip.
    fn relayout_and_focus(&mut self) {
        self.relayout();
        let id = self.workspace.focused_window();
        self.focus(id);
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let pressed = event.state() == KeyState::Pressed;
                let action = self.seat.get_keyboard().unwrap().input::<Action, _>(
                    self,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
                    |_, mods, handle| {
                        if pressed && (mods.alt || mods.logo) {
                            if let Some(a) = action_for(handle.modified_sym(), mods.shift) {
                                return FilterResult::Intercept(a);
                            }
                        }
                        FilterResult::Forward
                    },
                );
                if let Some(a) = action {
                    self.run(a);
                }
            }
            InputEvent::PointerMotion { .. } => {}
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self.space.outputs().next().unwrap();
                let geo = self.space.output_geometry(output).unwrap();
                let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
                let under = self.surface_under(pos);
                let pointer = self.seat.get_pointer().unwrap();
                pointer.motion(self, under, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time: event.time_msec() });
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let state = event.state();

                if state == ButtonState::Pressed && !pointer.is_grabbed() {
                    let clicked = self
                        .space
                        .element_under(pointer.current_location())
                        .and_then(|(w, _)| w.toplevel().map(|t| t.wl_surface().clone()))
                        .and_then(|s| self.id_of_surface(&s));
                    if let Some(id) = clicked {
                        // Clicking a tiled window focuses its column and scrolls to it.
                        if self.workspace.focus_window(id) {
                            self.relayout();
                        }
                        self.focus(Some(id));
                    }
                }

                pointer.button(self, &ButtonEvent { button: event.button_code(), state, serial, time: event.time_msec() });
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let amount = |axis| event.amount(axis).unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0);
                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let v = amount(axis);
                    if v != 0.0 {
                        frame = frame.value(axis, v);
                        if let Some(d) = event.amount_v120(axis) {
                            frame = frame.v120(axis, d as i32);
                        }
                    }
                    if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                        frame = frame.stop(axis);
                    }
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
}
