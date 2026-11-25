//! Nested backend: Galaxia runs as a window inside your current desktop.
use std::time::Duration;

use smithay::{
    backend::{
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    desktop::layer_map_for_output,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::{
    render::{build_elements, Decorations, Shaders},
    CalloopData, Galaxia,
};

pub fn init_winit(event_loop: &mut EventLoop<CalloopData>, data: &mut CalloopData) -> Result<(), Box<dyn std::error::Error>> {
    let state = &mut data.state;
    let (mut backend, winit) = winit::init()?;

    let shaders = Shaders::compile(backend.renderer())?;
    let mut decos = Decorations::default();

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
                layer_map_for_output(&output).arrange();
                state.relayout();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                state.tick();
                let age = backend.buffer_age().unwrap_or(0);
                let background = state.theme.background.0;
                let backend_size = backend.window_size();
                let damage = {
                    let (renderer, mut framebuffer) = backend.bind().unwrap();
                    let elements = build_elements(renderer, state, &output, &shaders, &mut decos);
                    let damage = match damage_tracker.render_output(renderer, &mut framebuffer, age, &elements, background) {
                        Ok(res) => res.damage.cloned(),
                        Err(e) => {
                            tracing::warn!("render failed: {e:?}");
                            None
                        }
                    };
                    if state.take_screenshot_due() {
                        match crate::screenshot::save(renderer, &framebuffer, backend_size) {
                            Ok(path) => tracing::info!("screenshot saved to {}", path.display()),
                            Err(e) => tracing::warn!("screenshot failed: {e}"),
                        }
                    }
                    damage
                };
                // Only push pixels that changed. We still swap every frame (with a
                // 1px damage when idle) so the loop waits for vsync instead of spinning the CPU.
                let damage = damage.filter(|d| !d.is_empty()).unwrap_or_else(|| vec![Rectangle::from_size((1, 1).into())]);
                if let Err(e) = backend.submit(Some(&damage)) {
                    tracing::warn!("submit failed: {e:?}");
                }

                state.space.elements().for_each(|window| {
                    window.send_frame(&output, state.start_time.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()))
                });
                for layer in layer_map_for_output(&output).layers() {
                    layer.send_frame(&output, state.start_time.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()));
                }
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
