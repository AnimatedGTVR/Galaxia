//! Nested backend: Galaxia runs as a window inside your current desktop.
use std::time::Duration;

use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::{CalloopData, Galaxia};

/// Deep-space background until wallpapers exist.
const BACKGROUND: [f32; 4] = [0.04, 0.03, 0.09, 1.0];

pub fn init_winit(event_loop: &mut EventLoop<CalloopData>, data: &mut CalloopData) -> Result<(), Box<dyn std::error::Error>> {
    let state = &mut data.state;
    let (mut backend, winit) = winit::init()?;

    let mode = Mode { size: backend.window_size(), refresh: 60_000 };
    let output = Output::new(
        "galaxia-nested".into(),
        PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Galaxia".into(), model: "Nested".into() },
    );
    let _global = output.create_global::<Galaxia>(&data.display_handle);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    // Programs we launch connect to Galaxia, not the outer desktop.
    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    event_loop.handle().insert_source(winit, move |event, _, data| {
        let state = &mut data.state;
        match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(Some(Mode { size, refresh: 60_000 }), None, None, None);
                state.relayout();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);
                {
                    let (renderer, mut framebuffer) = backend.bind().unwrap();
                    smithay::desktop::space::render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _>(
                        &output,
                        renderer,
                        &mut framebuffer,
                        1.0,
                        0,
                        [&state.space],
                        &[],
                        &mut damage_tracker,
                        BACKGROUND,
                    )
                    .unwrap();
                }
                backend.submit(Some(&[damage])).unwrap();

                state.space.elements().for_each(|window| {
                    window.send_frame(&output, state.start_time.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()))
                });
                state.space.refresh();
                state.popups.cleanup();
                let _ = data.display_handle.flush_clients();
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => (),
        }
    })?;

    Ok(())
}
