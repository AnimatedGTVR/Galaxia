//! Wayland protocol handlers: compositor, shm, xdg-shell, decorations, layer-shell,
//! seat, data device, output.
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface as DesktopLayer, PopupKind,
        Window, WindowSurfaceType,
    },
    input::{
        pointer::{CursorImageStatus, Focus},
        Seat, SeatHandler, SeatState,
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        wayland_server::{
            protocol::{wl_buffer, wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
            Client, Resource,
        },
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState},
        output::OutputHandler,
        selection::{
            data_device::{set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
            SelectionHandler,
        },
        shell::{
            wlr_layer::{Layer, LayerSurface, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState},
            xdg::{
                decoration::XdgDecorationHandler, PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler,
                XdgShellState, XdgToplevelSurfaceData,
            },
        },
        shm::{ShmHandler, ShmState},
    },
};

use crate::{
    grabs::MoveGrab,
    state::{ClientState, Galaxia},
};

// ---- Compositor ----

impl CompositorHandler for Galaxia {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some((_, window)) = self.windows.iter().find(|(_, w)| w.toplevel().map(|t| t.wl_surface() == &root).unwrap_or(false)) {
                window.on_commit();
            }
        }

        // First commit of a new toplevel: decide tiled vs floating, then send its initial configure.
        if let Some(id) = self.id_of_surface(surface) {
            let sent = with_states(surface, |s| {
                s.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().initial_configure_sent
            });
            if !sent {
                self.maybe_autofloat(id);
                if let Some(t) = self.window(id).and_then(|w| w.toplevel()) {
                    t.send_configure();
                }
            }
        }

        self.handle_layer_commit(surface);

        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(ref xdg)) = self.popups.find_popup(surface) {
            if !xdg.is_initial_configure_sent() {
                xdg.send_configure().expect("initial configure failed");
            }
        }
    }
}

impl Galaxia {
    fn handle_layer_commit(&mut self, surface: &WlSurface) {
        let Some(output) = self
            .space
            .outputs()
            .find(|o| layer_map_for_output(o).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).is_some())
            .cloned()
        else {
            return;
        };
        let initial_sent = with_states(surface, |s| {
            s.data_map.get::<LayerSurfaceData>().map(|d| d.lock().unwrap().initial_configure_sent).unwrap_or(true)
        });

        let before = self.usable_area();
        let wants_focus = {
            let mut map = layer_map_for_output(&output);
            map.arrange();
            let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).unwrap();
            if !initial_sent {
                layer.layer_surface().send_configure();
            }
            initial_sent && layer.can_receive_keyboard_focus() && matches!(layer.layer(), Layer::Top | Layer::Overlay)
        };
        if wants_focus {
            self.focus_layer_once(surface);
        }
        // A panel appearing or resizing changes the space left for windows.
        if self.usable_area() != before {
            self.relayout();
        }
    }
}

impl BufferHandler for Galaxia {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Galaxia {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Galaxia);
delegate_shm!(Galaxia);

// ---- XDG shell (app windows and popups) ----

impl XdgShellHandler for Galaxia {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Galaxia draws the frame (border, corners, shadow), so ask apps to drop their own title bars.
        surface.with_pending_state(|s| s.decoration_mode = Some(DecorationMode::ServerSide));
        self.add_window(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_surface(surface.wl_surface());
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// Dragging an app's own title bar moves it, if it is floating. Tiled windows stay put.
    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Galaxia>::from_resource(&seat) else { return };
        let Some(id) = self.id_of_surface(surface.wl_surface()) else { return };
        let Some(initial) = self.floating_pos(id) else { return };
        let Some(pointer) = seat.get_pointer() else { return };
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start_data) = pointer.grab_start_data() else { return };
        pointer.set_grab(self, MoveGrab { start_data, id, initial }, serial, Focus::Clear);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}

delegate_xdg_shell!(Galaxia);

impl Galaxia {
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else { return };
        let Some(window) = self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface() == &root).unwrap_or(false)) else {
            return;
        };
        let Some(output) = self.space.outputs().next() else { return };
        let (Some(output_geo), Some(window_geo)) = (self.space.output_geometry(output), self.space.element_geometry(window)) else {
            return;
        };
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

// ---- Decorations: always server side ----

impl XdgDecorationHandler for Galaxia {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecorationMode::ServerSide));
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        self.new_decoration(toplevel.clone());
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.request_mode(toplevel, DecorationMode::ServerSide);
    }
}

delegate_xdg_decoration!(Galaxia);

// ---- Layer shell: panels, docks, launchers, wallpapers ----

impl WlrLayerShellHandler for Galaxia {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: LayerSurface, output: Option<WlOutput>, _layer: Layer, namespace: String) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else { return };
        tracing::info!(namespace, "new layer surface");
        let mut map = layer_map_for_output(&output);
        if let Err(e) = map.map_layer(&DesktopLayer::new(surface, namespace)) {
            tracing::warn!("could not map layer surface: {e}");
        }
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let wl = surface.wl_surface().clone();
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(layer) = found {
                map.unmap_layer(&layer);
            }
        }
        self.forget_layer(&wl);
        self.relayout();
    }
}

delegate_layer_shell!(Galaxia);

// ---- Seat (keyboard / pointer) ----

impl SeatHandler for Galaxia {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Galaxia> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(Galaxia);

// ---- Clipboard / drag and drop ----

impl SelectionHandler for Galaxia {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Galaxia {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Galaxia {}
impl ServerDndGrabHandler for Galaxia {}

delegate_data_device!(Galaxia);

// ---- Outputs ----

impl OutputHandler for Galaxia {}
delegate_output!(Galaxia);
