//! Drawing: rounded window corners, accent borders, soft shadows, layer surfaces.
//!
//! Element order is front to back:
//!   overlay + top layers  →  for each window (top first): border, window, shadow  →  bottom + background layers
use std::collections::HashMap;

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
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

use crate::{background::GlassSource, layout::WindowId, Galaxia};

smithay::backend::renderer::element::render_elements! {
    pub GxElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Rounded=RoundedElement,
    Shader=PixelShaderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Glass=GlassElement,
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

/// Frosted glass: a patch of the blurred wallpaper, tinted, grained, with rounded corners.
const GLASS_TEX_FRAG: &str = r#"
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec4 srcn;     // the sampled region of the texture, normalized (x, y, w, h)
uniform vec2 geo;      // size of the glass on screen (physical px)
uniform float radius;
uniform vec4 glass_tint;
float hash(vec2 p) {
    p = fract(p * vec2(443.897, 441.423));
    p += dot(p, p.yx + 19.19);
    return fract((p.x + p.y) * p.x);
}
void main() {
    vec3 rgb = texture2D(tex, v_coords).rgb;
    vec2 p = (v_coords - srcn.xy) / srcn.zw * geo;
    rgb = mix(rgb, glass_tint.rgb, glass_tint.a);
    rgb += (hash(floor(p)) - 0.5) * 0.025;   // fine grain: reads as frosted, not smeared
    float d = rbox(p - geo * 0.5, geo * 0.5, radius);
    float mask = 1.0 - smoothstep(-0.5, 0.5, d);
    vec4 color = vec4(rgb, 1.0) * mask * alpha;
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
    glass: GlesTexProgram,
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
            &[UniformName::new("elem", UniformType::_4f), UniformName::new("geo", UniformType::_2f), radius.clone()],
        )?;
        let glass = r.compile_custom_texture_shader(
            format!("{tex_header}{PRECISION}{sampler}{SDF}{GLASS_TEX_FRAG}"),
            &[
                UniformName::new("srcn", UniformType::_4f),
                UniformName::new("geo", UniformType::_2f),
                radius,
                UniformName::new("glass_tint", UniformType::_4f),
            ],
        )?;
        Ok(Shaders { shadow, border, rounded, glass })
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

/// Frosted glass behind a window or panel.
#[derive(Debug)]
pub struct GlassElement {
    inner: MemoryRenderBufferRenderElement<GlesRenderer>,
    /// Own id per window/panel: every glass patch samples the same buffer,
    /// so the buffer's id can't tell them apart for damage tracking.
    id: Id,
    program: GlesTexProgram,
    buffer_size: (f64, f64),
    radius: f32,
    tint: [f32; 4],
}

impl Element for GlassElement {
    fn id(&self) -> &Id {
        &self.id
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
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for GlassElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let (bw, bh) = self.buffer_size;
        let t = self.tint;
        let uniforms = vec![
            Uniform::new(
                "srcn",
                UniformValue::_4f((src.loc.x / bw) as f32, (src.loc.y / bh) as f32, (src.size.w / bw) as f32, (src.size.h / bh) as f32),
            ),
            Uniform::new("geo", UniformValue::_2f(dst.size.w as f32, dst.size.h as f32)),
            Uniform::new("radius", UniformValue::_1f(self.radius)),
            Uniform::new("glass_tint", UniformValue::_4f(t[0], t[1], t[2], t[3])),
        ];
        frame.override_default_tex_program(self.program.clone(), uniforms);
        let res = self.inner.draw(frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// Which glass patch an id belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GlassKey {
    Window(WindowId),
    Layer(u32),
}

/// Build a glass patch covering `rect` (screen logical coords), clipped to the screen.
fn glass_element(
    renderer: &mut GlesRenderer,
    shaders: &Shaders,
    glass: &GlassSource<'_>,
    id: Id,
    rect: Rectangle<i32, Logical>,
    screen: Rectangle<i32, Logical>,
    theme: &galaxia_theme::Theme,
    scale: Scale<f64>,
) -> Option<GxElement> {
    let rect = rect.intersection(screen)?;
    if rect.size.w <= 0 || rect.size.h <= 0 {
        return None;
    }
    let k = glass.scale;
    let src = Rectangle::<f64, Logical>::new(
        ((rect.loc.x - screen.loc.x) as f64 * k, (rect.loc.y - screen.loc.y) as f64 * k).into(),
        (rect.size.w as f64 * k, rect.size.h as f64 * k).into(),
    );
    let loc = rect.loc.to_f64().to_physical(scale);
    let inner = MemoryRenderBufferRenderElement::from_buffer(renderer, loc, glass.buffer, None, Some(src), Some(rect.size), Kind::Unspecified)
        .map_err(|e| tracing::warn!("glass upload failed: {e:?}"))
        .ok()?;
    let buffer_size = ((screen.size.w as f64 * k).round(), (screen.size.h as f64 * k).round());
    Some(GxElement::Glass(GlassElement {
        inner,
        id,
        program: shaders.glass.clone(),
        buffer_size,
        radius: theme.corner_radius * scale.x as f32,
        tint: theme.glass_tint.0,
    }))
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
    glass_ids: HashMap<GlassKey, Id>,
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
        self.glass_ids.retain(|k, _| match k {
            GlassKey::Window(id) => state.window(*id).is_some(),
            GlassKey::Layer(_) => true,
        });
    }

    fn glass_id(&mut self, key: GlassKey) -> Id {
        self.glass_ids.entry(key).or_insert_with(Id::new).clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    layers: &[Layer],
    scale: Scale<f64>,
    screen: Rectangle<i32, Logical>,
    state: &Galaxia,
    shaders: &Shaders,
    glass: Option<&GlassSource<'_>>,
    decos: &mut Decorations,
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
            // Glass behind panels and launchers (not behind wallpaper tools).
            if let (Some(glass), true) = (glass, state.theme.glass_layers && *layer_kind != Layer::Background) {
                use smithay::reexports::wayland_server::Resource;
                let id = decos.glass_id(GlassKey::Layer(layer.wl_surface().id().protocol_id()));
                let rect = Rectangle::new(geo.loc + screen.loc, geo.size);
                out.extend(glass_element(renderer, shaders, glass, id, rect, screen, &state.theme, scale));
            }
        }
    }
}

pub fn build_elements(
    renderer: &mut GlesRenderer,
    state: &Galaxia,
    output: &Output,
    shaders: &Shaders,
    decos: &mut Decorations,
    background: &mut crate::background::Background,
) -> Vec<GxElement> {
    decos.prune(state);
    let scale = Scale::from(output.current_scale().fractional_scale());
    let out_rect = state.space.output_geometry(output).unwrap_or_default();
    let focused = state.focused_id();
    let radius = state.theme.corner_radius * scale.x as f32;
    let mut elements = Vec::new();

    let glass = background.glass_source();
    layer_elements(renderer, output, &[Layer::Overlay, Layer::Top], scale, out_rect, state, shaders, glass.as_ref(), decos, &mut elements);

    for window in state.space.elements().rev() {
        let Some(id) = state.id_of_window(window) else { continue };
        let Some(loc) = state.space.element_location(window) else { continue };
        let rect = Rectangle::new(loc, window.geometry().size);
        if !expand(rect, state.theme.shadow_size as i32).overlaps(out_rect) {
            continue; // scrolled off screen
        }
        let deco = decos.get(shaders, state, id, rect, focused == Some(id));
        let (border, shadow) = (deco.border.clone(), deco.shadow.clone());
        if state.theme.border_width > 0.0 {
            elements.push(GxElement::Shader(border));
        }
        let render_loc = (loc - window.geometry().loc).to_physical_precise_round(scale);
        let geo = rect.to_physical_precise_round(scale);
        elements.extend(
            window
                .render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(renderer, render_loc, scale, 1.0)
                .into_iter()
                .map(|inner| GxElement::Rounded(RoundedElement { inner, program: shaders.rounded.clone(), geo, radius })),
        );
        if let (Some(glass), true) = (glass.as_ref(), state.theme.glass_windows) {
            let gid = decos.glass_id(GlassKey::Window(id));
            elements.extend(glass_element(renderer, shaders, glass, gid, rect, out_rect, &state.theme, scale));
        }
        if state.theme.shadow_size > 0.0 && state.theme.shadow.0[3] > 0.0 {
            elements.push(GxElement::Shader(shadow));
        }
    }

    layer_elements(renderer, output, &[Layer::Bottom, Layer::Background], scale, out_rect, state, shaders, glass.as_ref(), decos, &mut elements);
    drop(glass);
    elements.extend(background.elements(renderer, &state.theme, state.theme_gen, out_rect.size));
    elements
}
