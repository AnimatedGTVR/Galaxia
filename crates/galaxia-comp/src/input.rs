//! Keyboard shortcuts and pointer input.
//!
//! Shortcuts use Alt (or Super) so they work while nested inside another desktop.
//! The full list lives in the README.
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
    },
    utils::SERIAL_COUNTER,
};

use crate::{
    grabs::MoveGrab,
    state::{spawn, terminal_command, Galaxia},
};

const BTN_LEFT: u32 = 0x110;
/// Scroll distance that counts as one step for Alt+scroll.
const SCROLL_STEP: f64 = 15.0;

#[derive(Debug, Clone, Copy)]
enum Action {
    Terminal,
    Close,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    MoveLeft,
    MoveRight,
    CycleWidth,
    FullWidth,
    ToggleFloat,
    TogglePin,
    ConsumeLeft,
    ExpelRight,
    Workspace(usize),
    MoveToWorkspace(usize),
    WorkspaceUp,
    WorkspaceDown,
    ReloadTheme,
    Screenshot,
    Quit,
}

fn action_for(sym: Keysym, shift: bool) -> Option<Action> {
    use Action::*;
    // Number row: workspaces. `sym` is the unshifted key, so Shift+1 still reads as 1.
    let raw = sym.raw();
    if (Keysym::_1.raw()..=Keysym::_9.raw()).contains(&raw) {
        let n = (raw - Keysym::_1.raw()) as usize;
        return Some(if shift { MoveToWorkspace(n) } else { Workspace(n) });
    }
    Some(match (sym, shift) {
        (Keysym::Return, _) => Terminal,
        (Keysym::q, _) => Close,
        (Keysym::Left | Keysym::h, false) => FocusLeft,
        (Keysym::Right | Keysym::l, false) => FocusRight,
        (Keysym::Left | Keysym::h, true) => MoveLeft,
        (Keysym::Right | Keysym::l, true) => MoveRight,
        (Keysym::Up | Keysym::k, _) => FocusUp,
        (Keysym::Down | Keysym::j, _) => FocusDown,
        (Keysym::Page_Up | Keysym::u, _) => WorkspaceUp,
        (Keysym::Page_Down | Keysym::i, _) => WorkspaceDown,
        (Keysym::r, false) => CycleWidth,
        (Keysym::r, true) => ReloadTheme,
        (Keysym::f, _) => FullWidth,
        (Keysym::v, _) => ToggleFloat,
        (Keysym::p, _) => TogglePin,
        (Keysym::bracketleft, _) => ConsumeLeft,
        (Keysym::bracketright, _) => ExpelRight,
        (Keysym::s, true) => Screenshot,
        (Keysym::e, true) => Quit,
        _ => return None,
    })
}

impl Galaxia {
    fn run(&mut self, action: Action) {
        use Action::*;
        match action {
            Terminal => spawn(&terminal_command()),
            Close => {
                if let Some(t) = self.focused_id().and_then(|id| self.window(id)).and_then(|w| w.toplevel().cloned()) {
                    t.send_close();
                }
            }
            FocusLeft => self.strip(|ws| ws.focus_left()),
            FocusRight => self.strip(|ws| ws.focus_right()),
            FocusUp => self.strip(|ws| ws.focus_up()),
            FocusDown => self.strip(|ws| ws.focus_down()),
            MoveLeft => self.strip(|ws| ws.move_column_left()),
            MoveRight => self.strip(|ws| ws.move_column_right()),
            CycleWidth => self.strip(|ws| ws.cycle_width()),
            FullWidth => self.strip(|ws| ws.toggle_full_width()),
            ConsumeLeft => self.strip(|ws| ws.consume_left()),
            ExpelRight => self.strip(|ws| ws.expel_right()),
            ToggleFloat => {
                if let Some(id) = self.focused_id() {
                    let area = self.usable_area();
                    if let Some(ws) = self.ws_of(id) {
                        self.workspaces[ws].toggle_float(id, area.size.w as f32, area.size.h as f32);
                    }
                    self.relayout();
                    self.focus(Some(id));
                }
            }
            TogglePin => {
                if let Some(id) = self.focused_id() {
                    if let Some(ws) = self.ws_of(id) {
                        if let Some(f) = self.workspaces[ws].floating_mut(id) {
                            f.pinned = !f.pinned;
                        }
                    }
                    self.apply_positions();
                }
            }
            Workspace(n) => self.switch_workspace(n),
            MoveToWorkspace(n) => self.move_to_workspace(n),
            WorkspaceUp => self.switch_workspace(self.active.saturating_sub(1)),
            WorkspaceDown => self.switch_workspace(self.active + 1),
            ReloadTheme => self.reload_theme(),
            Screenshot => self.screenshot_requested = true,
            Quit => self.loop_signal.stop(),
        }
    }

    /// Change the active strip, then re-place windows and focus the strip's focused window.
    fn strip(&mut self, f: impl FnOnce(&mut crate::layout::Workspace)) {
        f(self.ws());
        self.relayout();
        self.focus_layout_window(true);
    }

    fn alt_held(&self) -> bool {
        self.seat.get_keyboard().map(|k| { let m = k.modifier_state(); m.alt || m.logo }).unwrap_or(false)
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
                            let sym = handle.raw_latin_sym_or_raw_current_sym().unwrap_or_else(|| handle.modified_sym());
                            if let Some(a) = action_for(sym, mods.shift) {
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
                let Some(output) = self.space.outputs().next() else { return };
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
                let button = event.button_code();

                if state == ButtonState::Pressed && !pointer.is_grabbed() {
                    let location = pointer.current_location();
                    let clicked = self
                        .space
                        .element_under(location)
                        .and_then(|(w, _)| w.toplevel().map(|t| t.wl_surface().clone()))
                        .and_then(|s| self.id_of_surface(&s));

                    if let Some(id) = clicked {
                        if self.ws().focus_window(id) {
                            self.relayout();
                        }
                        self.focus(Some(id));

                        // Alt+drag: move a floating window (a tiled one pops out to float first).
                        if button == BTN_LEFT && self.alt_held() {
                            if self.floating_pos(id).is_none() {
                                let area = self.usable_area();
                                self.ws().toggle_float(id, area.size.w as f32, area.size.h as f32);
                                // Center the new float under the cursor.
                                if let Some(f) = self.ws().floating_mut(id) {
                                    f.rect.x = (location.x - area.loc.x as f64) as f32 - f.rect.w / 2.0;
                                    f.rect.y = (location.y - area.loc.y as f64) as f32 - 20.0;
                                }
                                self.relayout();
                                self.focus(Some(id));
                            }
                            if let Some(initial) = self.floating_pos(id) {
                                let start_data = GrabStartData { focus: None, button, location };
                                pointer.set_grab(self, MoveGrab { start_data, id, initial }, serial, Focus::Clear);
                                pointer.frame(self);
                                return;
                            }
                        }
                    } else if let Some((surface, _)) = self.surface_under(location) {
                        // A layer surface (panel, launcher) that takes keyboard input.
                        if let Some(kb) = self.seat.get_keyboard() {
                            kb.set_focus(self, Some(surface), serial);
                        }
                    }
                }

                pointer.button(self, &ButtonEvent { button, state, serial, time: event.time_msec() });
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let amount = |axis| event.amount(axis).unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0);

                // Alt+scroll moves through the strip; Alt+Shift+scroll moves through workspaces.
                if self.alt_held() {
                    let shift = self.seat.get_keyboard().map(|k| k.modifier_state().shift).unwrap_or(false);
                    let delta = amount(Axis::Vertical) + amount(Axis::Horizontal);
                    self.scroll_accum += delta;
                    while self.scroll_accum.abs() >= SCROLL_STEP {
                        let forward = self.scroll_accum > 0.0;
                        self.scroll_accum -= SCROLL_STEP * self.scroll_accum.signum();
                        self.run(match (shift, forward) {
                            (false, true) => Action::FocusRight,
                            (false, false) => Action::FocusLeft,
                            (true, true) => Action::WorkspaceDown,
                            (true, false) => Action::WorkspaceUp,
                        });
                    }
                    return;
                }
                self.scroll_accum = 0.0;

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
