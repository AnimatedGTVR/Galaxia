//! Screenshots of Galaxia's own output (never the outer desktop).
//!
//! Alt+Shift+S saves one. `GALAXIA_SCREENSHOT_AFTER=<seconds>` takes one automatically,
//! which is handy for scripted tests and README images.
//! Saved to `$GALAXIA_SCREENSHOT_DIR`, else `~/Pictures/Galaxia/`.
use std::path::PathBuf;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::RenderElement,
            gles::{GlesRenderer, GlesTexture},
            Bind, ExportMem, Offscreen, TextureMapping,
        },
    },
    utils::{Physical, Rectangle, Size, Transform},
};

pub fn screenshot_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GALAXIA_SCREENSHOT_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Pictures").join("Galaxia")
}

/// Draw `elements` into an offscreen texture (never the on-screen surface) and save it as a PNG.
pub fn save<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    elements: &[E],
    clear: [f32; 4],
    size: Size<i32, Physical>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let region = Rectangle::from_size((size.w, size.h).into());
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;
    let mapping = {
        let mut target = renderer.bind(&mut texture)?;
        let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        damage.render_output(renderer, &mut target, 0, elements, clear)?;
        renderer.copy_framebuffer(&target, region, Fourcc::Abgr8888)?
    };
    // GL textures store rows bottom-up; flip unless the mapping already did.
    let flipped = !mapping.flipped();
    let pixels = renderer.map_texture(&mapping)?;

    let (w, h) = (size.w as usize, size.h as usize);
    let stride = w * 4;
    let mut rows: Vec<u8> = Vec::with_capacity(stride * h);
    for y in 0..h {
        let src_y = if flipped { h - 1 - y } else { y };
        rows.extend_from_slice(&pixels[src_y * stride..(src_y + 1) * stride]);
    }
    // The framebuffer has no meaningful alpha; make the PNG fully opaque.
    for px in rows.chunks_exact_mut(4) {
        px[3] = 255;
    }

    let dir = screenshot_dir();
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis();
    let path = dir.join(format!("galaxia-{stamp}.png"));
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rows)?;
    Ok(path)
}
