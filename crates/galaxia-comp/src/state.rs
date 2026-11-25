use std::{ffi::OsString, sync::Arc};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::layout::{WindowId, Workspace};
use crate::CalloopData;

pub struct Galaxia {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    pub space: Space<Window>,
    pub workspace: Workspace,
    pub windows: Vec<(WindowId, Window)>,
    next_id: WindowId,
    pub theme: galaxia_theme::Theme,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Galaxia>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub seat: Seat<Self>,
}

/// Launch a program inside Galaxia (it inherits our WAYLAND_DISPLAY).
pub fn spawn(cmd: &str) {
    let mut parts = cmd.split_whitespace();
    if let Some(prog) = parts.next() {
        if let Err(e) = std::process::Command::new(prog).args(parts).spawn() {
            tracing::warn!("could not start {prog}: {e}");
        }
    }
}

/// What Alt+Enter opens: $GALAXIA_TERM, else the first terminal found.
pub fn terminal_command() -> String {
    if let Ok(t) = std::env::var("GALAXIA_TERM") {
        return t;
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for t in ["foot", "alacritty", "kitty", "wezterm", "ptyxis", "gnome-terminal", "konsole", "weston-terminal", "nautilus"] {
        if path.split(':').any(|d| std::path::Path::new(d).join(t).exists()) {
            return t.to_string();
        }
    }
    "nautilus".to_string()
}

impl Galaxia {
    pub fn new(event_loop: &mut EventLoop<CalloopData>, display: Display<Self>) -> Self {
        let dh = display.handle();
        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "galaxia");
        seat.add_keyboard(Default::default(), 200, 25).unwrap();
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        Self {
            start_time: std::time::Instant::now(),
            socket_name,
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            workspace: Workspace::default(),
            windows: Vec::new(),
            next_id: 1,
            theme: galaxia_theme::Theme::default(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups: PopupManager::default(),
            seat,
        }
    }

    fn init_wayland_listener(display: Display<Galaxia>, event_loop: &mut EventLoop<CalloopData>) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto().unwrap();
        let socket_name = listening_socket.socket_name().to_os_string();
        let handle = event_loop.handle();

        handle
            .insert_source(listening_socket, |stream, _, data| {
                data.display_handle
                    .insert_client(stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("failed to init the wayland socket");

        handle
            .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, data| {
                // Safety: we never drop the display
                unsafe { display.get_mut().dispatch_clients(&mut data.state).unwrap() };
                Ok(PostAction::Continue)
            })
            .unwrap();

        socket_name
    }

    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    pub fn id_of_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|(_, w)| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false))
            .map(|(id, _)| *id)
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|(i, _)| *i == id).map(|(_, w)| w)
    }

    /// The window that currently has keyboard focus (tiled or floating).
    pub fn focused_id(&self) -> Option<WindowId> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.id_of_surface(&focus)
    }

    pub fn add_window(&mut self, window: Window) {
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push((id, window));
        self.workspace.open(id);
        self.relayout();
    }

    pub fn remove_surface(&mut self, surface: &WlSurface) {
        if let Some(id) = self.id_of_surface(surface) {
            if let Some(w) = self.window(id).cloned() {
                self.space.unmap_elem(&w);
            }
            self.windows.retain(|(i, _)| *i != id);
            self.workspace.close(id);
            self.relayout();
        }
    }

    pub fn output_size(&self) -> (f32, f32) {
        self.space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| (g.size.w as f32, g.size.h as f32))
            .unwrap_or((1280.0, 800.0))
    }

    /// Recompute every window's position and size from the layout and apply it.
    pub fn relayout(&mut self) {
        let (ow, oh) = self.output_size();
        let gap = self.theme.gap;
        self.workspace.ensure_focused_visible(ow, gap);

        for (id, r) in self.workspace.arrange(ow, oh, gap) {
            let Some(window) = self.window(id).cloned() else { continue };
            let Some(toplevel) = window.toplevel() else { continue };
            toplevel.with_pending_state(|s| {
                s.size = Some((r.w.max(1.0) as i32, r.h.max(1.0) as i32).into());
                for st in [
                    xdg_toplevel::State::TiledLeft,
                    xdg_toplevel::State::TiledRight,
                    xdg_toplevel::State::TiledTop,
                    xdg_toplevel::State::TiledBottom,
                ] {
                    s.states.set(st);
                }
            });
            toplevel.send_pending_configure();
            // Offset by the client's own geometry so CSD shadows don't shift the window.
            let loc = (r.x as i32, r.y as i32);
            let geo = window.geometry().loc;
            self.space.map_element(window, (loc.0 - geo.x, loc.1 - geo.y), false);
        }

        for f in self.workspace.floating.clone() {
            let Some(window) = self.window(f.id).cloned() else { continue };
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|s| {
                    s.size = Some((f.rect.w as i32, f.rect.h as i32).into());
                    for st in [
                        xdg_toplevel::State::TiledLeft,
                        xdg_toplevel::State::TiledRight,
                        xdg_toplevel::State::TiledTop,
                        xdg_toplevel::State::TiledBottom,
                    ] {
                        s.states.unset(st);
                    }
                });
                toplevel.send_pending_configure();
            }
            // Floating windows always stack above the strip.
            self.space.map_element(window.clone(), (f.rect.x as i32, f.rect.y as i32), true);
        }

        self.focus_layout_window();
    }

    /// Give keyboard focus to the layout's focused window, unless a floating one has it.
    fn focus_layout_window(&mut self) {
        if let Some(cur) = self.focused_id() {
            if self.workspace.floating.iter().any(|f| f.id == cur) {
                return;
            }
        }
        let target = self
            .workspace
            .focused_window()
            .or_else(|| self.workspace.floating.last().map(|f| f.id));
        self.focus(target);
    }

    pub fn focus(&mut self, id: Option<WindowId>) {
        let window = id.and_then(|id| self.window(id).cloned());
        let surface = window.as_ref().and_then(|w| w.toplevel()).map(|t| t.wl_surface().clone());
        if let Some(w) = &window {
            self.space.raise_element(w, true);
        }
        for (_, w) in &self.windows {
            w.set_activated(window.as_ref() == Some(w));
            if let Some(t) = w.toplevel() {
                t.send_pending_configure();
            }
        }
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
