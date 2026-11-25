//! Pointer grabs: Alt+drag moves a floating window.
use smithay::{
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
        GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

use crate::{layout::WindowId, Galaxia};

pub struct MoveGrab {
    pub start_data: PointerGrabStartData<Galaxia>,
    pub id: WindowId,
    /// Window position (relative to the usable area) when the drag started.
    pub initial: Point<f64, Logical>,
}

type Handle<'a> = PointerInnerHandle<'a, Galaxia>;

impl PointerGrab<Galaxia> for MoveGrab {
    fn motion(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, _focus: Option<(WlSurface, Point<f64, Logical>)>, event: &MotionEvent) {
        handle.motion(data, None, event);
        let pos = self.initial + (event.location - self.start_data.location);
        data.move_floating(self.id, pos.x as f32, pos.y as f32);
    }

    fn relative_motion(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, focus: Option<(WlSurface, Point<f64, Logical>)>, event: &RelativeMotionEvent) {
        handle.relative_motion(data, focus, event);
    }

    fn button(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, details: AxisFrame) {
        handle.axis(data, details)
    }
    fn frame(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>) {
        handle.frame(data);
    }
    fn gesture_swipe_begin(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GestureSwipeBeginEvent) {
        handle.gesture_swipe_begin(data, event)
    }
    fn gesture_swipe_update(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GestureSwipeUpdateEvent) {
        handle.gesture_swipe_update(data, event)
    }
    fn gesture_swipe_end(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GestureSwipeEndEvent) {
        handle.gesture_swipe_end(data, event)
    }
    fn gesture_pinch_begin(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GesturePinchBeginEvent) {
        handle.gesture_pinch_begin(data, event)
    }
    fn gesture_pinch_update(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GesturePinchUpdateEvent) {
        handle.gesture_pinch_update(data, event)
    }
    fn gesture_pinch_end(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GesturePinchEndEvent) {
        handle.gesture_pinch_end(data, event)
    }
    fn gesture_hold_begin(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GestureHoldBeginEvent) {
        handle.gesture_hold_begin(data, event)
    }
    fn gesture_hold_end(&mut self, data: &mut Galaxia, handle: &mut Handle<'_>, event: &GestureHoldEndEvent) {
        handle.gesture_hold_end(data, event)
    }
    fn start_data(&self) -> &PointerGrabStartData<Galaxia> {
        &self.start_data
    }
    fn unset(&mut self, _data: &mut Galaxia) {}
}
