//! Shared design tokens. Every Galaxia component reads its look from here.
//!
//! Loaded from `$XDG_CONFIG_HOME/galaxia/theme.toml` (or `~/.config/galaxia/theme.toml`).
//! Every field is optional; anything missing uses the default below.

use serde::{Deserialize, Deserializer};
use std::path::PathBuf;

/// Premultiplied-ready RGBA color, written in the theme file as "#rrggbb" or "#rrggbbaa".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color(pub [f32; 4]);

impl Color {
    pub fn hex(s: &str) -> Option<Color> {
        let s = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok().map(|b| b as f32 / 255.0);
        match s.len() {
            6 => Some(Color([byte(0)?, byte(2)?, byte(4)?, 1.0])),
            8 => Some(Color([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::hex(&s).ok_or_else(|| serde::de::Error::custom(format!("bad color {s:?}, use #rrggbb or #rrggbbaa")))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Focused window border, highlights.
    pub accent: Color,
    /// Border of windows without focus.
    pub border_inactive: Color,
    /// Base color of the built-in space background (used when no wallpaper is set).
    pub background: Color,
    /// Wallpaper image (png, jpeg or webp). `~` is expanded. Unset = built-in space background.
    pub wallpaper: Option<String>,
    /// Window shadow color; alpha sets strength.
    pub shadow: Color,

    pub corner_radius: f32,
    pub border_width: f32,
    /// Gap between windows and around the screen edge.
    pub gap: f32,
    /// How far the shadow spreads out from the window.
    pub shadow_size: f32,

    pub glass_blur: f32,
    pub glass_opacity: f32,

    /// Animation length in milliseconds. 0 turns animations off.
    pub anim_ms: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color([0.55, 0.45, 1.0, 1.0]),
            border_inactive: Color([1.0, 1.0, 1.0, 0.08]),
            background: Color([0.04, 0.03, 0.09, 1.0]),
            wallpaper: None,
            shadow: Color([0.0, 0.0, 0.0, 0.55]),
            corner_radius: 14.0,
            border_width: 2.0,
            gap: 12.0,
            shadow_size: 28.0,
            glass_blur: 24.0,
            glass_opacity: 0.6,
            anim_ms: 220,
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("galaxia").join("theme.toml")
}

impl Theme {
    /// The wallpaper path with `~` expanded.
    pub fn wallpaper_path(&self) -> Option<PathBuf> {
        let w = self.wallpaper.as_deref()?.trim();
        if w.is_empty() {
            return None;
        }
        match w.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)),
            None => Some(PathBuf::from(w)),
        }
    }

    /// Parse a theme from TOML text.
    pub fn from_toml(text: &str) -> Result<Theme, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Load the user's theme file. A missing file gives the defaults;
    /// a broken file gives the defaults plus the error so the caller can report it.
    pub fn load() -> (Theme, Option<String>) {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::from_toml(&text) {
                Ok(t) => (t, None),
                Err(e) => (Theme::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(_) => (Theme::default(), None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(Color::hex("#ff0000"), Some(Color([1.0, 0.0, 0.0, 1.0])));
        assert_eq!(Color::hex("00ff0080").unwrap().0[3], 128.0 / 255.0);
        assert_eq!(Color::hex("#zzz"), None);
    }

    #[test]
    fn partial_theme_keeps_defaults() {
        let t = Theme::from_toml("gap = 20\naccent = \"#ff8800\"").unwrap();
        assert_eq!(t.gap, 20.0);
        assert_eq!(t.corner_radius, Theme::default().corner_radius);
        assert_eq!(t.accent, Color::hex("#ff8800").unwrap());
    }

    #[test]
    fn wallpaper_tilde_expands() {
        let t = Theme::from_toml("wallpaper = \"~/Pictures/a.png\"").unwrap();
        let p = t.wallpaper_path().unwrap();
        assert!(p.ends_with("Pictures/a.png") && !p.starts_with("~"));
    }

    #[test]
    fn rejects_typos() {
        assert!(Theme::from_toml("gapp = 3").is_err());
    }
}
