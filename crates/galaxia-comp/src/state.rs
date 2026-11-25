use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{layer_map_for_output, PopupManager, Space, Window, WindowSurfaceType},
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
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::{decoration::XdgDecorationState, XdgShellState},
        },
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::layout::{ease, Rect, WindowId, Workspace};
use crate::CalloopData;

pub const WORKSPACES: usize = 9;

pub struct Galaxia {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    pub space: Space<Window>,
    pub windows: Vec<(WindowId, Window)>,
    next_id: WindowId,

    /// Workspaces stack vertically; `active` is the one in view.
    pub workspaces: Vec<Workspace>,
    pub active: usize,
    /// Animated strip offsets (one per workspace) and vertical workspace position.
    view_x: Vec<f32>,
    view_y: f32,
    last_tick: Instant,

    pub theme: galaxia_theme::Theme,
    /// Bumped on every theme reload so cached drawing state gets rebuilt.
    pub theme_gen: u64,

    /// Layer surfaces (panel, launcher) that already got their first keyboard focus.
    focused_layers: Vec<WlSurface>,
    /// Accumulated Alt+scroll distance, so a wheel notch moves exactly one step.
    pub scroll_accum: f64,
    /// A screenshot is wanted on the next frame (or at this time, for GALAXIA_SCREENSHOT_AFTER).
    pub screenshot_requested: bool,
    screenshot_at: Option<Instant>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
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
    // Single-instance apps must be told to start fresh, or they hand the window
    // to a copy already running on the outer desktop.
    let candidates = [
        ("ghostty", "ghostty --gtk-single-instance=false"),
        ("foot", "foot"),
        ("alacritty", "alacritty"),
        ("kitty", "kitty"),
        ("wezterm", "wezterm start --always-new-process"),
        ("konsole", "konsole"),
        ("weston-terminal", "weston-terminal"),
    ];
    for (bin, cmd) in candidates {
        if path.split(':').any(|d| std::path::Path::new(d).join(bin).exists()) {
            return cmd.to_string();
        }
    }
    tracing::warn!("no terminal found; set GALAXIA_TERM");
    "foot".to_string()
}

fn load_theme() -> galaxia_theme::Theme {
    let (theme, err) = galaxia_theme::Theme::load();
    if let Some(e) = err {
        tracing::warn!("theme file has a problem, using defaults: {e}");
    }
    theme
}

const TILED_STATES: [xdg_toplevel::State; 4] = [
    xdg_toplevel::State::TiledLeft,
    xdg_toplevel::State::TiledRight,
    xdg_toplevel::State::TiledTop,
    xdg_toplevel::State::TiledBottom,
];

impl Galaxia {
    pub fn new(event_loop: &mut EventLoop<CalloopData>, display: Display<Self>) -> Self {
        let dh = display.handle();
        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "galaxia");
        seat.add_keyboard(Default::default(), 200, 25).unwrap();
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        Self {
            start_time: Instant::now(),
            socket_name,
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            windows: Vec::new(),
            next_id: 1,
            workspaces: vec![Workspace::default(); WORKSPACES],
            active: 0,
            view_x: vec![0.0; WORKSPACES],
            view_y: 0.0,
            last_tick: Instant::now(),
            theme: load_theme(),
            theme_gen: 0,
            focused_layers: Vec::new(),
            scroll_accum: 0.0,
            screenshot_requested: false,
            screenshot_at: std::env::var("GALAXIA_SCREENSHOT_AFTER")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .map(|secs| Instant::now() + std::time::Duration::from_secs_f32(secs)),
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
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

    /// True once when a screenshot should be taken this frame.
    pub fn take_screenshot_due(&mut self) -> bool {
        if self.screenshot_at.is_some_and(|t| Instant::now() >= t) {
            self.screenshot_at = None;
            return true;
        }
        std::mem::take(&mut self.screenshot_requested)
    }

    pub fn reload_theme(&mut self) {
        self.theme = load_theme();
        self.theme_gen += 1;
        tracing::info!("theme reloaded from {}", galaxia_theme::config_path().display());
        self.relayout();
    }

    // ---- lookups ----

    pub fn id_of_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|(_, w)| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false))
            .map(|(id, _)| *id)
    }

    pub fn id_of_window(&self, window: &Window) -> Option<WindowId> {
        self.windows.iter().find(|(_, w)| w == window).map(|(id, _)| *id)
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|(i, _)| *i == id).map(|(_, w)| w)
    }

    pub fn ws_of(&self, id: WindowId) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.contains(id))
    }

    pub fn ws(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active]
    }

    /// The window that currently has keyboard focus (tiled or floating).
    pub fn focused_id(&self) -> Option<WindowId> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.id_of_surface(&focus)
    }

    /// Screen area not covered by panels (layer-shell exclusive zones).
    pub fn usable_area(&self) -> Rectangle<i32, Logical> {
        let Some(output) = self.space.outputs().next() else {
            return Rectangle::from_size((1280, 800).into());
        };
        let zone = layer_map_for_output(output).non_exclusive_zone();
        let origin = self.space.output_geometry(output).map(|g| g.loc).unwrap_or_default();
        Rectangle::new(zone.loc + origin, zone.size)
    }

    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.outputs().next()?;
        let origin = self.space.output_geometry(output)?.loc;
        let map = layer_map_for_output(output);
        let layer_hit = |kinds: &[Layer]| {
            kinds.iter().find_map(|k| {
                let layer = map.layer_under(*k, pos - origin.to_f64())?;
                let loc = map.layer_geometry(layer)?.loc + origin;
                layer
                    .surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + loc).to_f64()))
            })
        };
        layer_hit(&[Layer::Overlay, Layer::Top])
            .or_else(|| {
                self.space.element_under(pos).and_then(|(window, location)| {
                    window
                        .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                        .map(|(s, p)| (s, (p + location).to_f64()))
                })
            })
            .or_else(|| layer_hit(&[Layer::Bottom, Layer::Background]))
    }

    // ---- window lifecycle ----

    pub fn add_window(&mut self, window: Window) {
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push((id, window));
        self.ws().open(id);
        self.relayout();
        self.focus(Some(id));
    }

    /// Dialogs (windows with a parent) float instead of taking a column.
    pub fn maybe_autofloat(&mut self, id: WindowId) {
        let has_parent = self.window(id).and_then(|w| w.toplevel()).and_then(|t| t.parent()).is_some();
        let Some(ws) = self.ws_of(id) else { return };
        if has_parent && !self.workspaces[ws].is_floating(id) {
            let area = self.usable_area();
            let ws = &mut self.workspaces[ws];
            ws.close(id);
            let (w, h) = (area.size.w as f32 * 0.4, area.size.h as f32 * 0.5);
            ws.open_floating(id, Rect { x: (area.size.w as f32 - w) / 2.0, y: (area.size.h as f32 - h) / 2.0, w, h }, false);
            self.relayout();
            self.focus(Some(id));
        }
    }

    pub fn remove_surface(&mut self, surface: &WlSurface) {
        let Some(id) = self.id_of_surface(surface) else { return };
        if let Some(w) = self.window(id).cloned() {
            self.space.unmap_elem(&w);
        }
        self.windows.retain(|(i, _)| *i != id);
        for ws in &mut self.workspaces {
            ws.close(id);
        }
        self.relayout();
        let next = self.workspaces[self.active].focused_window();
        self.focus(next.or_else(|| self.workspaces[self.active].floating.last().map(|f| f.id)));
    }

    // ---- layout ----

    /// Recompute sizes for every window, send them to clients, and place everything.
    pub fn relayout(&mut self) {
        let area = self.usable_area();
        let (aw, ah) = (area.size.w as f32, area.size.h as f32);
        let gap = self.theme.gap;
        self.workspaces[self.active].ensure_focused_visible(aw, gap);

        for i in 0..self.workspaces.len() {
            let ws = &self.workspaces[i];
            let tiled = ws.arrange(aw, ah, gap, 0.0);
            let floating: Vec<_> = ws.floating.iter().map(|f| (f.id, f.rect)).collect();
            for (id, r, tiled) in tiled.into_iter().map(|(id, r)| (id, r, true)).chain(floating.into_iter().map(|(id, r)| (id, r, false))) {
                let Some(toplevel) = self.window(id).and_then(|w| w.toplevel()).cloned() else { continue };
                toplevel.with_pending_state(|s| {
                    s.size = Some((r.w.max(1.0) as i32, r.h.max(1.0) as i32).into());
                    for st in TILED_STATES {
                        if tiled {
                            s.states.set(st);
                        } else {
                            s.states.unset(st);
                        }
                    }
                });
                if toplevel.is_initial_configure_sent() {
                    toplevel.send_pending_configure();
                }
            }
        }

        self.apply_positions();
    }

    /// Map windows at their (animated) positions. Cheap: no client round trips.
    pub fn apply_positions(&mut self) {
        let area = self.usable_area();
        let (aw, ah) = (area.size.w as f32, area.size.h as f32);
        let gap = self.theme.gap;
        let mut floats_on_top = Vec::new();

        for i in 0..self.workspaces.len() {
            let off_y = (i as f32 - self.view_y) * ah;
            let visible = off_y.abs() < ah;
            let ws = &self.workspaces[i];
            let tiled = ws.arrange(aw, ah, gap, self.view_x[i]);
            let floating: Vec<_> = ws.floating.clone();

            for (id, r) in tiled {
                let Some(w) = self.window(id).cloned() else { continue };
                if visible {
                    let loc = (area.loc.x + r.x.round() as i32, area.loc.y + (r.y + off_y).round() as i32);
                    self.space.map_element(w, loc, false);
                } else {
                    self.space.unmap_elem(&w);
                }
            }
            for f in floating {
                let Some(w) = self.window(f.id).cloned() else { continue };
                let off = if f.pinned { 0.0 } else { off_y };
                if f.pinned || visible {
                    let loc = (area.loc.x + f.rect.x.round() as i32, area.loc.y + (f.rect.y + off).round() as i32);
                    self.space.map_element(w.clone(), loc, false);
                    floats_on_top.push(w);
                } else {
                    self.space.unmap_elem(&w);
                }
            }
        }
        // Floating windows always stack above the strip, in their own order.
        for w in floats_on_top {
            self.space.raise_element(&w, false);
        }
    }

    /// Advance animations. Call once per frame.
    pub fn tick(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        let ms = self.theme.anim_ms;
        let mut moving = false;
        for i in 0..self.workspaces.len() {
            let target = self.workspaces[i].scroll;
            moving |= ease(&mut self.view_x[i], target, dt, ms);
        }
        moving |= ease(&mut self.view_y, self.active as f32, dt, ms);
        if moving {
            self.apply_positions();
        }
    }

    pub fn move_floating(&mut self, id: WindowId, x: f32, y: f32) {
        let Some(ws) = self.ws_of(id) else { return };
        if let Some(f) = self.workspaces[ws].floating_mut(id) {
            f.rect.x = x;
            f.rect.y = y;
        }
        self.apply_positions();
    }

    /// Where a floating window sits, relative to the usable area.
    pub fn floating_pos(&self, id: WindowId) -> Option<Point<f64, Logical>> {
        let ws = self.ws_of(id)?;
        let f = self.workspaces[ws].floating.iter().find(|f| f.id == id)?;
        Some((f.rect.x as f64, f.rect.y as f64).into())
    }

    pub fn switch_workspace(&mut self, n: usize) {
        if n >= self.workspaces.len() || n == self.active {
            return;
        }
        self.active = n;
        self.relayout();
        self.focus_layout_window(true);
    }

    pub fn move_to_workspace(&mut self, n: usize) {
        let Some(id) = self.focused_id() else { return };
        let Some(from) = self.ws_of(id) else { return };
        if n >= self.workspaces.len() || n == from {
            return;
        }
        match self.workspaces[from].close(id) {
            Some(f) => self.workspaces[n].open_floating(id, f.rect, f.pinned),
            None => self.workspaces[n].open(id),
        }
        self.relayout();
        self.focus_layout_window(true);
    }

    // ---- focus ----

    /// Focus the active workspace's focused window. With `force`, even if a float has focus.
    pub fn focus_layout_window(&mut self, force: bool) {
        let ws = &self.workspaces[self.active];
        if !force {
            if let Some(cur) = self.focused_id() {
                if ws.is_floating(cur) {
                    return;
                }
            }
        }
        let target = ws.focused_window().or_else(|| ws.floating.last().map(|f| f.id));
        self.focus(target);
    }

    pub fn focus(&mut self, id: Option<WindowId>) {
        let window = id.and_then(|id| self.window(id).cloned());
        if let Some(id) = id {
            if let Some(ws) = self.ws_of(id) {
                self.workspaces[ws].raise_floating(id);
                if self.workspaces[ws].is_floating(id) {
                    if let Some(w) = &window {
                        self.space.raise_element(w, false);
                    }
                }
            }
        }
        for (_, w) in &self.windows {
            w.set_activated(window.as_ref() == Some(w));
            if let Some(t) = w.toplevel() {
                if t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
        }
        let surface = window.as_ref().and_then(|w| w.toplevel()).map(|t| t.wl_surface().clone());
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    /// Give keyboard focus to a layer surface (launcher, panel menu) once, when it first asks for it.
    pub fn focus_layer_once(&mut self, surface: &WlSurface) {
        if self.focused_layers.contains(surface) {
            return;
        }
        self.focused_layers.push(surface.clone());
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        }
    }

    pub fn forget_layer(&mut self, surface: &WlSurface) {
        self.focused_layers.retain(|s| s != surface);
        let had_focus = self.seat.get_keyboard().and_then(|k| k.current_focus()).as_ref() == Some(surface);
        if had_focus {
            self.focus_layout_window(true);
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
