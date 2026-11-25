//! Shared design tokens. Every Galaxia component reads its look from here.

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: [f32; 4],
    pub corner_radius: f32,
    pub gap: f32,
    pub glass_blur: f32,
    pub glass_opacity: f32,
    pub anim_ms: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: [0.55, 0.45, 1.0, 1.0],
            corner_radius: 14.0,
            gap: 10.0,
            glass_blur: 24.0,
            glass_opacity: 0.6,
            anim_ms: 180,
        }
    }
}
