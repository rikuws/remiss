use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{px, Pixels, Rgba, WindowAppearance};
use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;

const THEME_SETTINGS_CACHE_KEY: &str = "theme-settings-v1";
const UI_FONT_FAMILY: &str = ".AppleSystemUIFont";
const DISPLAY_SERIF_FONT_FAMILY: &str = "Instrument Serif";

const INK: u32 = 0x0f1624;
const SLATE: u32 = 0x3b4452;
const MIST: u32 = 0xe6e8eb;
const PAPER: u32 = 0xf7f6f3;
const SAGE: u32 = 0x55756a;
const OCHRE: u32 = 0xb6924d;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn label(&self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn all() -> &'static [ThemePreference] {
        &[
            ThemePreference::System,
            ThemePreference::Light,
            ThemePreference::Dark,
        ]
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActiveTheme {
    Light = 0,
    #[default]
    Dark = 1,
}

impl ActiveTheme {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSettings {
    #[serde(default)]
    pub preference: ThemePreference,
}

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(ActiveTheme::Dark as u8);

fn color(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

pub fn transparent() -> Rgba {
    color(0.0, 0.0, 0.0, 0.0)
}

fn hex(hex: u32) -> Rgba {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: 1.0 }
}

fn hex_alpha(value: u32, a: f32) -> Rgba {
    let mut rgba = hex(value);
    rgba.a = a;
    rgba
}

fn theme_hex(light: u32, dark: u32) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => hex(light),
        ActiveTheme::Dark => hex(dark),
    }
}

fn theme_hex_alpha(light: (u32, f32), dark: (u32, f32)) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => hex_alpha(light.0, light.1),
        ActiveTheme::Dark => hex_alpha(dark.0, dark.1),
    }
}

fn theme_rgba(light: (f32, f32, f32, f32), dark: (f32, f32, f32, f32)) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => color(light.0, light.1, light.2, light.3),
        ActiveTheme::Dark => color(dark.0, dark.1, dark.2, dark.3),
    }
}

pub fn load_theme_settings(cache: &CacheStore) -> Result<ThemeSettings, String> {
    Ok(cache
        .get::<ThemeSettings>(THEME_SETTINGS_CACHE_KEY)?
        .map(|document| document.value)
        .unwrap_or_default())
}

pub fn save_theme_settings(cache: &CacheStore, settings: &ThemeSettings) -> Result<(), String> {
    cache.put(THEME_SETTINGS_CACHE_KEY, settings, now_ms())
}

pub fn resolve_theme(preference: ThemePreference, appearance: WindowAppearance) -> ActiveTheme {
    match preference {
        ThemePreference::System => {
            if is_light_appearance(appearance) {
                ActiveTheme::Light
            } else {
                ActiveTheme::Dark
            }
        }
        ThemePreference::Light => ActiveTheme::Light,
        ThemePreference::Dark => ActiveTheme::Dark,
    }
}

pub fn set_active_theme(theme: ActiveTheme) {
    ACTIVE_THEME.store(theme as u8, Ordering::Relaxed);
}

pub fn active_theme() -> ActiveTheme {
    match ACTIVE_THEME.load(Ordering::Relaxed) {
        value if value == ActiveTheme::Light as u8 => ActiveTheme::Light,
        _ => ActiveTheme::Dark,
    }
}

pub fn appearance_label(appearance: WindowAppearance) -> &'static str {
    if is_light_appearance(appearance) {
        "Light"
    } else {
        "Dark"
    }
}

pub fn is_light_appearance(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Light | WindowAppearance::VibrantLight
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn ui_font_family() -> &'static str {
    UI_FONT_FAMILY
}

pub fn display_serif_font_family() -> &'static str {
    DISPLAY_SERIF_FONT_FAMILY
}

pub fn bg_canvas() -> Rgba {
    theme_hex(PAPER, INK)
}

pub fn bg_surface() -> Rgba {
    theme_hex(0xfcfbf8, 0x141c29)
}

pub fn bg_overlay() -> Rgba {
    theme_hex(0xffffff, 0x1a2330)
}

pub fn bg_inset() -> Rgba {
    theme_hex(MIST, 0x0b111b)
}

pub fn bg_subtle() -> Rgba {
    theme_hex(0xf2f1ed, 0x18212d)
}

pub fn bg_emphasis() -> Rgba {
    theme_hex(0xecebe6, 0x202a36)
}

pub fn bg_selected() -> Rgba {
    theme_hex(0xe8eee9, 0x1b2a2a)
}

pub fn accent() -> Rgba {
    theme_hex(SAGE, 0x8ba99d)
}

pub fn accent_muted() -> Rgba {
    theme_hex_alpha((SAGE, 0.14), (0x8ba99d, 0.18))
}

pub fn ochre() -> Rgba {
    theme_hex(OCHRE, 0xd3b36f)
}

pub fn ochre_muted() -> Rgba {
    theme_hex_alpha((OCHRE, 0.16), (0xd3b36f, 0.18))
}

pub fn border_default() -> Rgba {
    theme_hex_alpha((SLATE, 0.18), (MIST, 0.20))
}

pub fn border_muted() -> Rgba {
    theme_hex_alpha((SLATE, 0.10), (MIST, 0.12))
}

pub fn diff_hunk_bg() -> Rgba {
    theme_hex(0xe8ecec, 0x111b26)
}

pub fn diff_hunk_fg() -> Rgba {
    theme_hex(SAGE, 0xaac2b6)
}

pub fn diff_context_bg() -> Rgba {
    theme_hex(PAPER, INK)
}

pub fn diff_context_gutter_bg() -> Rgba {
    theme_hex(0xf1f0ec, 0x111927)
}

pub fn diff_meta_bg() -> Rgba {
    theme_hex(MIST, 0x18212d)
}

pub fn diff_add_bg() -> Rgba {
    theme_hex(0xebf3ee, 0x122019)
}

pub fn diff_add_gutter_bg() -> Rgba {
    theme_hex(0xdfebe4, 0x17271f)
}

pub fn diff_add_emphasis_bg() -> Rgba {
    theme_hex_alpha((SAGE, 0.18), (0x8fb2a2, 0.25))
}

pub fn diff_add_border() -> Rgba {
    transparent()
}

pub fn diff_remove_bg() -> Rgba {
    theme_hex(0xf8ece9, 0x241719)
}

pub fn diff_remove_gutter_bg() -> Rgba {
    theme_hex(0xf0dfdc, 0x2a1d1f)
}

pub fn diff_remove_emphasis_bg() -> Rgba {
    theme_hex_alpha((0xa1524f, 0.18), (0xd28a86, 0.26))
}

pub fn diff_remove_border() -> Rgba {
    transparent()
}

pub fn fg_default() -> Rgba {
    theme_hex(SLATE, MIST)
}

pub fn fg_muted() -> Rgba {
    theme_hex(0x66717f, 0xa7adb5)
}

pub fn fg_subtle() -> Rgba {
    theme_hex(0x8f969d, 0x737c88)
}

pub fn fg_emphasis() -> Rgba {
    theme_hex(INK, PAPER)
}

pub fn success() -> Rgba {
    theme_hex(0x3f7a56, 0x8fb98e)
}

pub fn success_muted() -> Rgba {
    theme_hex_alpha((0x3f7a56, 0.12), (0x8fb98e, 0.14))
}

pub fn danger() -> Rgba {
    theme_hex(0xa1524f, 0xd28a86)
}

pub fn danger_muted() -> Rgba {
    theme_hex_alpha((0xa1524f, 0.12), (0xd28a86, 0.14))
}

pub fn purple() -> Rgba {
    ochre()
}

pub fn waypoint_bg() -> Rgba {
    ochre_muted()
}

pub fn waypoint_active_bg() -> Rgba {
    theme_hex_alpha((OCHRE, 0.22), (0xd3b36f, 0.26))
}

pub fn waypoint_border() -> Rgba {
    theme_hex_alpha((OCHRE, 0.32), (0xd3b36f, 0.34))
}

pub fn waypoint_fg() -> Rgba {
    theme_hex(0x866736, 0xf0d58a)
}

pub fn waypoint_icon_bg() -> Rgba {
    theme_hex(0xf2eadb, 0x2b2418)
}

pub fn waypoint_icon_border() -> Rgba {
    theme_hex_alpha((OCHRE, 0.42), (0xd3b36f, 0.40))
}

pub fn waypoint_icon_core() -> Rgba {
    ochre()
}

pub fn hover_bg() -> Rgba {
    theme_hex(0xedece8, 0x202a36)
}

pub fn palette_backdrop() -> Rgba {
    theme_rgba((0.06, 0.09, 0.14, 0.16), (0.02, 0.03, 0.05, 0.66))
}

pub fn topbar_height() -> Pixels {
    px(48.0)
}

pub fn sidebar_width() -> Pixels {
    px(260.0)
}

pub fn file_tree_width() -> Pixels {
    px(252.0)
}

pub fn detail_side_width() -> Pixels {
    px(280.0)
}

pub fn radius() -> Pixels {
    px(8.0)
}

pub fn radius_sm() -> Pixels {
    px(4.0)
}

pub fn lane_accent_color(repo: &str) -> Rgba {
    let hash: u32 = repo.bytes().fold(5381u32, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as u32)
    });
    let palette = match active_theme() {
        ActiveTheme::Light => [
            hex(SAGE),
            hex(OCHRE),
            hex(0x557f87),
            hex(0x6e6f53),
            hex(0x9a675d),
            hex(0x586a8a),
            hex(0x7b6d55),
            hex(SLATE),
        ],
        ActiveTheme::Dark => [
            hex(0x8ba99d),
            hex(0xd3b36f),
            hex(0x80aab0),
            hex(0x9b9c72),
            hex(0xc28a7f),
            hex(0x8fa0c1),
            hex(0xb2a07e),
            hex(0xa7adb5),
        ],
    };
    palette[(hash as usize) % palette.len()]
}
