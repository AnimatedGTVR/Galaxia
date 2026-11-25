//! Drawing: rounded window corners, accent borders, soft shadows, layer surfaces.
//!
//! Element order is front to back:
//!   overlay + top layers  →  for each window (top first): border, window, shadow  →  bottom + background layers
use std::collections::HashMap;

use smithay::{
    backend::renderer::{
        element::{
            surface::WaylandSurfaceRenderElement, AsRenderElements, Element, Id, Kind, RenderElement, UnderlyingStorage,
        },
        gles::{
            element::PixelShaderElement, GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform,
            UniformName, UniformType, UniformValue,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    desktop::layer_map_for_output,
    output::Output,
    utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Transform},
    wayland::shell::wlr_layer::Layer,
};

use crate::{layout::WindowId, Galaxia};

smithay::backend::renderer::element::render_elements! {
    pub GxElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Rounded=RoundedElement,
    Shader=PixelShaderElement,
}

const SDF: &str = r#"
float rbox(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
"#;

const PRECISION: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
"#;

const SHADOW_FRAG: &str = r#"
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform float radius;
uniform float spread;
uniform vec4 color;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
void main() {
    vec2 p = v_coords * size - size * 0.5;
    float d = rbox(p, size * 0.5 - spread, radius);
    float a = 1.0 - smoothstep(-spread * 0.2, spread, d);
    a = a * a;
    gl_FragColor = vec4(color.rgb * color.a, color.a) * a * alpha;
}
"#;

const BORDER_FRAG: &str = r#"
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform float radius;
uniform float width;
uniform vec4 color;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
void main() {
    vec2 p = v_coords * size - size * 0.5;
    vec2 h = size * 0.5;
    float outer = rbox(p, h, radius + width);
    float inner = rbox(p, h - width, radius);
    float a = (1.0 - smoothstep(-0.5, 0.5, outer)) * smoothstep(-0.5, 0.5, inner);
    gl_FragColor = vec4(color.rgb * color.a, color.a) * a * alpha;
}
"#;

/// Same as Smithay's default texture shader, plus rounded-corner clipping
/// against the window geometry. Pixels outside the geometry (popups) are left alone.
const ROUNDED_TEX_FRAG: &str = r#"
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec4 elem;   // this surface's rect relative to the window geometry (physical px)
uniform vec2 geo;    // window geometry size (physical px)
uniform float radius;
void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif
    vec2 p = elem.xy + v_coords * elem.zw;
    if (p.x >= 0.0 && p.y >= 0.0 && p.x <= geo.x && p.y <= geo.y) {
        float d = rbox(p - geo * 0.5, geo * 0.5, radius);
        color *= 1.0 - smoothstep(-0.5, 0.5, d);
    }
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

pub struct Shaders {
    shadow: GlesPixelProgram,
    border: GlesPixelProgram,
    rounded: GlesTexProgram,
}

impl Shaders {
    pub fn compile(r: &mut GlesRenderer) -> Result<Self, GlesError> {
        let color = UniformName::new("color", UniformType::_4f);
        let radius = UniformName::new("radius", UniformType::_1f);
        let shadow = r.compile_custom_pixel_shader(
            format!("{PRECISION}{SDF}{SHADOW_FRAG}"),
            &[radius.clone(), UniformName::new("spread", UniformType::_1f), color.clone()],
        )?;
        let border = r.compile_custom_pixel_shader(
            format!("{PRECISION}{SDF}{BORDER_FRAG}"),
            &[radius.clone(), UniformName::new("width", UniformType::_1f), color],
        )?;
        let tex_header = "#version 100\n//_DEFINES_\n#if defined(EXTERNAL)\n#extension GL_OES_EGL_image_external : require\n#endif\n";
        let sampler = "#if defined(EXTERNAL)\nuniform samplerExternalOES tex;\n#else\nuniform sampler2D tex;\n#endif\n";
        let rounded = r.compile_custom_texture_shader(
            format!("{tex_header}{PRECISION}{sampler}{SDF}{ROUNDED_TEX_FRAG}"),
            &[UniformName::new("elem", UniformType::_4f), UniformName::new("geo", UniformType::_2f), radius],
        )?;
        Ok(Shaders { shadow, border, rounded })
    }
}

/// A window surface drawn through the rounded-corner shader.
#[derive(Debug)]
pub struct RoundedElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    geo: Rectangle<i32, Physical>,
    radius: f32,
}

impl Element for RoundedElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // Corners are now see-through, so nothing is promised opaque.
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for RoundedElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let uniforms = vec![
            Uniform::new(
                "elem",
                UniformValue::_4f(
                    (dst.loc.x - self.geo.loc.x) as f32,
                    (dst.loc.y - self.geo.loc.y) as f32,
                    dst.size.w as f32,
                    dst.size.h as f32,
                ),
            ),
            Uniform::new("geo", UniformValue::_2f(self.geo.size.w as f32, self.geo.size.h as f32)),
            Uniform::new("radius", UniformValue::_1f(self.radius)),
        ];
        frame.override_default_tex_program(self.program.clone(), uniforms);
        let res = self.inner.draw(frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None // must go through our shader, never straight to a plane
    }
}

/// Cached border + shadow per window, so unchanged frames cause no extra damage.
struct Deco {
    border: PixelShaderElement,
    shadow: PixelShaderElement,
    key: (Rectangle<i32, Logical>, bool),
}

#[derive(Default)]
pub struct Decorations {
    map: HashMap<WindowId, Deco>,
    theme_gen: u64,
}

fn expand(r: Rectangle<i32, Logical>, by: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((r.loc.x - by, r.loc.y - by).into(), (r.size.w + by * 2, r.size.h + by * 2).into())
}

impl Decorations {
    fn get(&mut self, shaders: &Shaders, state: &Galaxia, id: WindowId, rect: Rectangle<i32, Logical>, focused: bool) -> &Deco {
        let t = &state.theme;
        let bw = t.border_width.round() as i32;
        let ss = t.shadow_size.round() as i32;
        let border_area = expand(rect, bw);
        let mut shadow_area = expand(rect, ss);
        shadow_area.loc.y += ss / 4; // light from above
        let border_color = if focused { t.accent.0 } else { t.border_inactive.0 };
        let border_uniforms = || {
            vec![
                Uniform::new("radius", UniformValue::_1f(t.corner_radius)),
                Uniform::new("width", UniformValue::_1f(t.border_width)),
                Uniform::new("color", UniformValue::_4f(border_color[0], border_color[1], border_color[2], border_color[3])),
            ]
        };
        let shadow_uniforms = || {
            let c = t.shadow.0;
            vec![
                Uniform::new("radius", UniformValue::_1f(t.corner_radius + t.shadow_size * 0.5)),
                Uniform::new("spread", UniformValue::_1f(t.shadow_size)),
                Uniform::new("color", UniformValue::_4f(c[0], c[1], c[2], c[3])),
            ]
        };

        let key = (rect, focused);
        let deco = self.map.entry(id).or_insert_with(|| Deco {
            border: PixelShaderElement::new(shaders.border.clone(), border_area, None, 1.0, border_uniforms(), Kind::Unspecified),
            shadow: PixelShaderElement::new(shaders.shadow.clone(), shadow_area, None, 1.0, shadow_uniforms(), Kind::Unspecified),
            key,
        });
        if deco.key != key {
            deco.border.resize(border_area, None);
            deco.shadow.resize(shadow_area, None);
            if deco.key.1 != focused {
                deco.border.update_uniforms(border_uniforms());
            }
            deco.key = key;
        }
        deco
    }

    /// Drop cached state for windows that are gone, and everything after a theme reload.
    fn prune(&mut self, state: &Galaxia) {
        if self.theme_gen != state.theme_gen {
            self.map.clear();
            self.theme_gen = state.theme_gen;
        }
        self.map.retain(|id, _| state.window(*id).is_some());
    }
}

fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    layers: &[Layer],
    scale: Scale<f64>,
    out: &mut Vec<GxElement>,
) {
    let map = layer_map_for_output(output);
    for layer_kind in layers {
        for layer in map.layers_on(*layer_kind).rev() {
            let Some(geo) = map.layer_geometry(layer) else { continue };
            let loc = geo.loc.to_physical_precise_round(scale);
            out.extend(
                AsRenderElements::<GlesRenderer>::render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                    layer, renderer, loc, scale, 1.0,
                )
                .into_iter()
                .map(GxElement::Surface),
            );
        }
    }
}

pub fn build_elements(
    renderer: &mut GlesRenderer,
    state: &Galaxia,
    output: &Output,
    shaders: &Shaders,
    decos: &mut Decorations,
) -> Vec<GxElement> {
    decos.prune(state);
    let scale = Scale::from(output.current_scale().fractional_scale());
    let out_rect = state.space.output_geometry(output).unwrap_or_default();
    let focused = state.focused_id();
    let radius = state.theme.corner_radius * scale.x as f32;
    let mut elements = Vec::new();

    layer_elements(renderer, output, &[Layer::Overlay, Layer::Top], scale, &mut elements);

    for window in state.space.elements().rev() {
        let Some(id) = state.id_of_window(window) else { continue };
        let Some(loc) = state.space.element_location(window) else { continue };
        let rect = Rectangle::new(loc, window.geometry().size);
        if !expand(rect, state.theme.shadow_size as i32).overlaps(out_rect) {
            continue; // scrolled off screen
        }
        let deco = decos.get(shaders, state, id, rect, focused == Some(id));
        if state.theme.border_width > 0.0 {
            elements.push(GxElement::Shader(deco.border.clone()));
        }
        let render_loc = (loc - window.geometry().loc).to_physical_precise_round(scale);
        let geo = rect.to_physical_precise_round(scale);
        elements.extend(
            window
                .render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(renderer, render_loc, scale, 1.0)
                .into_iter()
                .map(|inner| GxElement::Rounded(RoundedElement { inner, program: shaders.rounded.clone(), geo, radius })),
        );
        if state.theme.shadow_size > 0.0 && state.theme.shadow.0[3] > 0.0 {
            elements.push(GxElement::Shader(deco.shadow.clone()));
        }
    }

    layer_elements(renderer, output, &[Layer::Bottom, Layer::Background], scale, &mut elements);
    elements
}
