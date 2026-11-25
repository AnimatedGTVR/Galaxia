//! Galaxia compositor: Smithay, scrollable tiling + floating, glass.
#![allow(irrefutable_let_patterns)]

mod background;
mod grabs;
mod handlers;
mod input;
mod layout;
mod render;
mod screenshot;
mod state;
mod winit;

use smithay::reexports::{
    calloop::EventLoop,
    wayland_server::{Display, DisplayHandle},
};
pub use state::Galaxia;

pub struct CalloopData {
    pub state: Galaxia,
    pub display_handle: DisplayHandle,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match tracing_subscriber::EnvFilter::try_from_default_env() {
        Ok(f) => tracing_subscriber::fmt().with_env_filter(f).init(),
        Err(_) => tracing_subscriber::fmt().init(),
    }

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new()?;
    let display: Display<Galaxia> = Display::new()?;
    let display_handle = display.handle();
    let state = Galaxia::new(&mut event_loop, display);
    let mut data = CalloopData { state, display_handle };

    winit::init_winit(&mut event_loop, &mut data)?;
    tracing::info!(socket = ?data.state.socket_name, "Galaxia running. Alt+Enter opens a window.");

    // Optional first client: `galaxia-comp -c <command>`
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, cmd, ..] = args.as_slice() {
        if flag == "-c" || flag == "--command" {
            state::spawn(cmd);
        }
    }

    event_loop.run(None, &mut data, |_| {})?;
    Ok(())
}
