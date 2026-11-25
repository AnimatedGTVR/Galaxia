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

struct Loaded {
    path: PathBuf,
    size: Size<i32, Logical>,
    buffer: MemoryRenderBuffer,
}

type LoadResult = Result<(PathBuf, Size<i32, Logical>, Vec<u8>), String>;

pub struct Background {
    program: GlesPixelProgram,
    space: Option<(PixelShaderElement, Size<i32, Logical>, u64)>,
    image: Option<Loaded>,
    /// What we asked the worker for, so we don't ask twice.
    requested: Option<(PathBuf, Size<i32, Logical>)>,
    pending: Option<Receiver<LoadResult>>,
}

impl Background {
    pub fn new(program: GlesPixelProgram) -> Self {
        Background { program, space: None, image: None, requested: None, pending: None }
    }

    fn request(&mut self, path: PathBuf, size: Size<i32, Logical>) {
        let (tx, rx) = channel();
        self.requested = Some((path.clone(), size));
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let res = (|| {
                let img = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                // Cover: fill the screen, cropping whatever doesn't fit.
                let img = img.resize_to_fill(size.w as u32, size.h as u32, image::imageops::FilterType::Triangle);
                Ok((path, size, img.to_rgba8().into_raw()))
            })();
            let _ = tx.send(res);
        });
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        let Ok(res) = rx.try_recv() else { return };
        self.pending = None;
        match res {
            Ok((path, size, pixels)) => {
                let buffer = MemoryRenderBuffer::from_slice(
                    &pixels,
                    Fourcc::Abgr8888, // RGBA bytes in memory
                    (size.w, size.h),
                    1,
                    Transform::Normal,
                    Some(vec![Rectangle::from_size((size.w, size.h).into())]),
                );
                tracing::info!("wallpaper loaded: {}", path.display());
                self.image = Some(Loaded { path, size, buffer });
            }
            Err(e) => tracing::warn!("wallpaper failed, using the space background: {e}"),
        }
    }

    pub fn elements(&mut self, renderer: &mut GlesRenderer, theme: &galaxia_theme::Theme, theme_gen: u64, size: Size<i32, Logical>) -> Vec<GxElement> {
        self.poll();

        // Wallpaper wanted: make sure the right one (path + size) is loaded or loading.
        let wanted = theme.wallpaper_path();
        match &wanted {
            Some(path) => {
                let have = self.image.as_ref().map(|l| (&l.path, l.size)) == Some((path, size));
                let asked = self.requested.as_ref() == Some(&(path.clone(), size));
                if !have && !asked {
                    self.request(path.clone(), size);
                }
            }
            None => {
                self.image = None;
                self.requested = None;
            }
        }

        if let Some(img) = self.image.as_ref().filter(|l| Some(&l.path) == wanted.as_ref()) {
            // Stretch a stale-size image while the right size loads, rather than flash.
            let stretch = (img.size != size).then_some(size);
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
