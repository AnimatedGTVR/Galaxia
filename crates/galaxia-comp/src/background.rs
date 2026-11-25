//! Desktop background: a wallpaper image, or Galaxia's built-in deep-space backdrop.
//!
//! Wallpapers are decoded and scaled on a worker thread so a big image never
//! freezes the desktop; the space backdrop shows until the image is ready.
use std::{
    path::PathBuf,
    sync::mpsc::{channel, Receiver},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                Kind,
            },
            gles::{element::PixelShaderElement, GlesError, GlesPixelProgram, GlesRenderer, Uniform, UniformName, UniformType, UniformValue},
        },
    },
    utils::{Logical, Rectangle, Size, Transform},
};

use crate::render::GxElement;

/// Galaxia's own wallpaper, used when the theme doesn't set one.
/// Looked up in the install location first, then in the source tree (for dev builds).
pub fn default_wallpaper() -> Option<PathBuf> {
    const NAME: &str = "cosmic-cliffs.jpg";
    let mut candidates = Vec::new();
    if let Some(d) = std::env::var_os("GALAXIA_DATA_DIR") {
        candidates.push(PathBuf::from(d).join("wallpapers").join(NAME));
    }
    candidates.push(PathBuf::from("/usr/local/share/galaxia/wallpapers").join(NAME));
    candidates.push(PathBuf::from("/usr/share/galaxia/wallpapers").join(NAME));
    candidates.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/wallpapers")).join(NAME));
    candidates.into_iter().find(|p| p.exists())
}

/// What to show: the theme's wallpaper, `none` for the space backdrop, or Galaxia's default.
fn wanted_wallpaper(theme: &galaxia_theme::Theme) -> Option<PathBuf> {
    match theme.wallpaper.as_deref().map(str::trim) {
        Some("none") | Some("") => None,
        Some(_) => theme.wallpaper_path(),
        None => default_wallpaper(),
    }
}

const SPACE_FRAG: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform vec4 base;
uniform vec4 accent;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float glow(vec2 q, vec2 c, float k) {
    float d = length(q - c);
    return exp(-d * d * k);
}

void main() {
    float aspect = size.x / size.y;
    vec2 q = vec2(v_coords.x * aspect, v_coords.y);
    vec3 col = base.rgb;
    // Nebula: soft clouds of accent, teal and magenta.
    col += accent.rgb * 0.30 * glow(q, vec2(0.22 * aspect, 0.28), 4.0);
    col += vec3(0.10, 0.45, 0.65) * 0.22 * glow(q, vec2(0.85 * aspect, 0.80), 5.0);
    col += vec3(0.75, 0.25, 0.60) * 0.12 * glow(q, vec2(0.60 * aspect, 0.12), 9.0);
    // Stars: sparse, varied brightness, on a 3px grid so they stay crisp.
    vec2 cell = floor(v_coords * size / 3.0);
    float h = hash(cell);
    float star = step(0.9972, h) * (0.35 + 0.65 * hash(cell + 7.0));
    col += vec3(star) * vec3(0.9, 0.92, 1.0);
    gl_FragColor = vec4(col, 1.0) * alpha;
}
"#;

pub fn compile_space(r: &mut GlesRenderer) -> Result<GlesPixelProgram, GlesError> {
    r.compile_custom_pixel_shader(
        SPACE_FRAG,
        &[UniformName::new("base", UniformType::_4f), UniformName::new("accent", UniformType::_4f)],
    )
}

/// The blurred copy is stored at 1/BLUR_DOWNSCALE size; the GPU's smooth
/// scaling when it's stretched back up adds to the blur for free.
const BLUR_DOWNSCALE: i32 = 4;

/// What a loaded wallpaper was made for. A change in any part means reload.
#[derive(Clone, PartialEq)]
struct Key {
    path: PathBuf,
    size: Size<i32, Logical>,
    blur: u32,
}

struct Loaded {
    key: Key,
    buffer: MemoryRenderBuffer,
    /// Blurred, downscaled copy used for frosted glass.
    blurred: MemoryRenderBuffer,
    blurred_size: Size<i32, Logical>,
}

struct Decoded {
    key: Key,
    pixels: Vec<u8>,
    blurred: Vec<u8>,
    blurred_size: (u32, u32),
}

type LoadResult = Result<Decoded, String>;

fn rgba_buffer(pixels: &[u8], w: i32, h: i32) -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(
        pixels,
        Fourcc::Abgr8888, // RGBA bytes in memory
        (w, h),
        1,
        Transform::Normal,
        Some(vec![Rectangle::from_size((w, h).into())]),
    )
}

/// The frosted-glass source: a blurred wallpaper, and how to map screen pixels into it.
pub struct GlassSource<'a> {
    pub buffer: &'a MemoryRenderBuffer,
    /// Multiply a screen position by this to get a position in `buffer`.
    pub scale: f64,
}

pub struct Background {
    program: GlesPixelProgram,
    space: Option<(PixelShaderElement, Size<i32, Logical>, u64)>,
    image: Option<Loaded>,
    /// What we asked the worker for, so we don't ask twice.
    requested: Option<Key>,
    pending: Option<Receiver<LoadResult>>,
}

impl Background {
    pub fn new(program: GlesPixelProgram) -> Self {
        Background { program, space: None, image: None, requested: None, pending: None }
    }

    fn request(&mut self, key: Key) {
        let (tx, rx) = channel();
        self.requested = Some(key.clone());
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let res = (|| {
                use image::imageops::{self, FilterType};
                let img = image::open(&key.path).map_err(|e| format!("{}: {e}", key.path.display()))?;
                // Cover: fill the screen, cropping whatever doesn't fit.
                let full = img.resize_to_fill(key.size.w as u32, key.size.h as u32, FilterType::Triangle).to_rgba8();
                // Glass: shrink, then blur the small copy (fast even for 4K screens).
                let (bw, bh) = ((key.size.w / BLUR_DOWNSCALE).max(1) as u32, (key.size.h / BLUR_DOWNSCALE).max(1) as u32);
                let small = imageops::resize(&full, bw, bh, FilterType::Triangle);
                let sigma = (key.blur as f32 / BLUR_DOWNSCALE as f32).max(0.5);
                let blurred = imageops::fast_blur(&small, sigma);
                Ok(Decoded { key, pixels: full.into_raw(), blurred: blurred.into_raw(), blurred_size: (bw, bh) })
            })();
            let _ = tx.send(res);
        });
    }

    /// The blurred wallpaper for frosted glass, once it's loaded.
    pub fn glass_source(&self) -> Option<GlassSource<'_>> {
        let img = self.image.as_ref()?;
        Some(GlassSource { buffer: &img.blurred, scale: img.blurred_size.w as f64 / img.key.size.w as f64 })
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        let Ok(res) = rx.try_recv() else { return };
        self.pending = None;
        match res {
            Ok(d) => {
                let buffer = rgba_buffer(&d.pixels, d.key.size.w, d.key.size.h);
                let (bw, bh) = (d.blurred_size.0 as i32, d.blurred_size.1 as i32);
                let blurred = rgba_buffer(&d.blurred, bw, bh);
                tracing::info!("wallpaper loaded: {}", d.key.path.display());
                self.image = Some(Loaded { key: d.key, buffer, blurred, blurred_size: (bw, bh).into() });
            }
            Err(e) => tracing::warn!("wallpaper failed, using the space background: {e}"),
        }
    }

    pub fn elements(&mut self, renderer: &mut GlesRenderer, theme: &galaxia_theme::Theme, theme_gen: u64, size: Size<i32, Logical>) -> Vec<GxElement> {
        self.poll();

        // Wallpaper wanted: make sure the right one (path + size) is loaded or loading.
        let wanted = wanted_wallpaper(theme);
        match &wanted {
            Some(path) => {
                let key = Key { path: path.clone(), size, blur: theme.glass_blur.max(0.0).round() as u32 };
                let have = self.image.as_ref().map(|l| &l.key) == Some(&key);
                if !have && self.requested.as_ref() != Some(&key) {
                    self.request(key);
                }
            }
            None => {
                self.image = None;
                self.requested = None;
            }
        }

        if let Some(img) = self.image.as_ref().filter(|l| Some(&l.key.path) == wanted.as_ref()) {
            // Stretch a stale-size image while the right size loads, rather than flash.
            let stretch = (img.key.size != size).then_some(size);
            match MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), &img.buffer, None, None, stretch, Kind::Unspecified) {
                Ok(e) => return vec![GxElement::Memory(e)],
                Err(e) => tracing::warn!("wallpaper upload failed: {e:?}"),
            }
        }

        // Built-in space backdrop, cached until the size or theme changes.
        let stale = self.space.as_ref().is_none_or(|(_, s, g)| *s != size || *g != theme_gen);
        if stale {
            let b = theme.background.0;
            let a = theme.accent.0;
            let el = PixelShaderElement::new(
                self.program.clone(),
                Rectangle::from_size(size),
                Some(vec![Rectangle::from_size(size)]),
                1.0,
                vec![Uniform::new("base", UniformValue::_4f(b[0], b[1], b[2], 1.0)), Uniform::new("accent", UniformValue::_4f(a[0], a[1], a[2], 1.0))],
                Kind::Unspecified,
            );
            self.space = Some((el, size, theme_gen));
        }
        vec![GxElement::Shader(self.space.as_ref().unwrap().0.clone())]
    }
}
