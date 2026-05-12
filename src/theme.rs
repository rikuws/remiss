use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{linear_color_stop, linear_gradient, px, Background, Pixels, Rgba, WindowAppearance};
use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;

const THEME_SETTINGS_CACHE_KEY: &str = "theme-settings-v1";
const UI_FONT_FAMILY: &str = ".AppleSystemUIFont";
const MONO_FONT_FAMILY: &str = "Fira Code";
const DISPLAY_SERIF_FONT_FAMILY: &str = "Instrument Serif";
pub const TOGGLE_ANIMATION_MS: u64 = 140;

const LIGHT_CANVAS: u32 = 0xf6f8fb;
const LIGHT_SURFACE: u32 = 0xffffff;
const LIGHT_ELEVATED: u32 = 0xffffff;
const LIGHT_INSET: u32 = 0xedf2f7;
const LIGHT_SUBTLE: u32 = 0xf1f5f9;
const LIGHT_EMPHASIS: u32 = 0xe7edf5;
const LIGHT_SELECTED: u32 = 0xf0f3f7;
const LIGHT_TEXT_EMPHASIS: u32 = 0x101828;
const LIGHT_TEXT: u32 = 0x344054;
const LIGHT_TEXT_MUTED: u32 = 0x667085;
const LIGHT_TEXT_SUBTLE: u32 = 0x98a2b3;
const LIGHT_BORDER: u32 = 0xd6dee8;
const LIGHT_BORDER_MUTED: u32 = 0xe5ebf2;

const DARK_CANVAS: u32 = 0x0c0c0c;
const DARK_SURFACE: u32 = 0x101112;
const DARK_ELEVATED: u32 = 0x131313;
const DARK_INSET: u32 = 0x0b0d10;
const DARK_SUBTLE: u32 = 0x171717;
const DARK_EMPHASIS: u32 = 0x242424;
const DARK_SELECTED: u32 = 0x1d1d1d;
const DARK_TEXT_EMPHASIS: u32 = 0xf3f4f6;
const DARK_TEXT: u32 = 0xd4d4d4;
const DARK_TEXT_MUTED: u32 = 0xa3a3a3;
const DARK_TEXT_SUBTLE: u32 = 0x737373;
const DARK_BORDER: u32 = 0x303030;
const DARK_BORDER_MUTED: u32 = 0x252525;

const LIGHT_FOCUS: u32 = 0x2563eb;
const DARK_FOCUS: u32 = 0x7db4ff;
const LIGHT_BRAND_ACCENT: u32 = 0x006b5b;
const DARK_BRAND_ACCENT: u32 = 0x39dcc7;
const LIGHT_SUCCESS: u32 = 0x16a34a;
const DARK_SUCCESS: u32 = 0x6ee7a5;
const LIGHT_WARNING: u32 = 0xd97706;
const DARK_WARNING: u32 = 0xfbbf24;
const LIGHT_DANGER: u32 = 0xdc2626;
const DARK_DANGER: u32 = 0xfca5a5;
const LIGHT_INFO: u32 = 0x0891b2;
const DARK_INFO: u32 = 0x67e8f9;

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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodeFontSizePreference {
    Compact = 0,
    #[default]
    Default = 1,
    Large = 2,
    ExtraLarge = 3,
}

impl CodeFontSizePreference {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Default => "Default",
            Self::Large => "Large",
            Self::ExtraLarge => "Extra large",
        }
    }

    pub fn all() -> &'static [CodeFontSizePreference] {
        &[
            CodeFontSizePreference::Compact,
            CodeFontSizePreference::Default,
            CodeFontSizePreference::Large,
            CodeFontSizePreference::ExtraLarge,
        ]
    }

    pub fn smaller(self) -> Self {
        match self {
            Self::Compact => Self::Compact,
            Self::Default => Self::Compact,
            Self::Large => Self::Default,
            Self::ExtraLarge => Self::Large,
        }
    }

    pub fn larger(self) -> Self {
        match self {
            Self::Compact => Self::Default,
            Self::Default => Self::Large,
            Self::Large => Self::ExtraLarge,
            Self::ExtraLarge => Self::ExtraLarge,
        }
    }

    pub fn scale(&self) -> f32 {
        match self {
            Self::Compact => 0.93,
            Self::Default => 1.0,
            Self::Large => 1.12,
            Self::ExtraLarge => 1.24,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffColorThemePreference {
    #[default]
    Graphite = 0,
    GitHub = 1,
    VsCode = 2,
    Solarized = 3,
    Nord = 4,
    Gruvbox = 5,
    Monokai = 6,
    HighContrast = 7,
}

impl DiffColorThemePreference {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Graphite => "Graphite",
            Self::GitHub => "GitHub",
            Self::VsCode => "VS Code",
            Self::Solarized => "Solarized",
            Self::Nord => "Nord",
            Self::Gruvbox => "Gruvbox",
            Self::Monokai => "Monokai",
            Self::HighContrast => "High contrast",
        }
    }

    pub fn all() -> &'static [DiffColorThemePreference] {
        &[
            DiffColorThemePreference::Graphite,
            DiffColorThemePreference::GitHub,
            DiffColorThemePreference::VsCode,
            DiffColorThemePreference::Solarized,
            DiffColorThemePreference::Nord,
            DiffColorThemePreference::Gruvbox,
            DiffColorThemePreference::Monokai,
            DiffColorThemePreference::HighContrast,
        ]
    }

    pub fn next(self) -> Self {
        let themes = Self::all();
        let current_index = themes
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        themes[(current_index + 1) % themes.len()]
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
    #[serde(default, alias = "fontSize")]
    pub code_font_size: CodeFontSizePreference,
    #[serde(default)]
    pub diff_color_theme: DiffColorThemePreference,
}

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(ActiveTheme::Light as u8);
static ACTIVE_CODE_FONT_SIZE: AtomicU8 = AtomicU8::new(CodeFontSizePreference::Default as u8);
static ACTIVE_DIFF_COLOR_THEME: AtomicU8 = AtomicU8::new(DiffColorThemePreference::Graphite as u8);

fn color(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

pub fn transparent() -> Rgba {
    color(0.0, 0.0, 0.0, 0.0)
}

pub fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha;
    color
}

pub fn mix_rgba(from: Rgba, to: Rgba, progress: f32) -> Rgba {
    Rgba {
        r: from.r + (to.r - from.r) * progress,
        g: from.g + (to.g - from.g) * progress,
        b: from.b + (to.b - from.b) * progress,
        a: from.a + (to.a - from.a) * progress,
    }
}

pub fn selected_transition_progress(selected: bool, delta: f32) -> f32 {
    if selected {
        delta
    } else {
        1.0 - delta
    }
}

pub fn selected_reveal_progress(selected: bool, delta: f32) -> f32 {
    if selected {
        delta
    } else {
        0.0
    }
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

fn theme_pair_color(pair: (u32, u32)) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => hex(pair.0),
        ActiveTheme::Dark => hex(pair.1),
    }
}

fn theme_pair_alpha(pair: ((u32, f32), (u32, f32))) -> Rgba {
    let ((light_hex, light_alpha), (dark_hex, dark_alpha)) = pair;
    match active_theme() {
        ActiveTheme::Light => hex_alpha(light_hex, light_alpha),
        ActiveTheme::Dark => hex_alpha(dark_hex, dark_alpha),
    }
}

type ThemeColorPair = (u32, u32);
type ThemeAlphaPair = ((u32, f32), (u32, f32));

#[derive(Clone, Copy)]
struct DiffThemePalette {
    editor_bg: ThemeColorPair,
    editor_chrome: ThemeColorPair,
    editor_surface: ThemeColorPair,
    annotation_bg: ThemeColorPair,
    annotation_border: ThemeAlphaPair,
    line_hover_bg: ThemeAlphaPair,
    selected_edge: ThemeAlphaPair,
    gutter_separator: ThemeAlphaPair,
    hunk_bg: ThemeColorPair,
    hunk_fg: ThemeColorPair,
    context_bg: ThemeColorPair,
    context_gutter_bg: ThemeColorPair,
    meta_bg: ThemeColorPair,
    add_bg: ThemeColorPair,
    add_gutter_bg: ThemeColorPair,
    add_emphasis_bg: ThemeAlphaPair,
    add_border: ThemeAlphaPair,
    remove_bg: ThemeColorPair,
    remove_gutter_bg: ThemeColorPair,
    remove_emphasis_bg: ThemeAlphaPair,
    remove_border: ThemeAlphaPair,
}

const GRAPHITE_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xf8fafc, DARK_CANVAS),
    editor_chrome: (0xffffff, DARK_ELEVATED),
    editor_surface: (0xffffff, DARK_INSET),
    annotation_bg: (0xf2f6fb, 0x101417),
    annotation_border: ((LIGHT_BORDER_MUTED, 0.72), (0x2a3035, 0.58)),
    line_hover_bg: ((0xe6edf6, 0.58), (0xffffff, 0.045)),
    selected_edge: ((LIGHT_FOCUS, 0.58), (DARK_FOCUS, 0.72)),
    gutter_separator: ((LIGHT_BORDER_MUTED, 0.64), (0x163022, 0.82)),
    hunk_bg: (0xf1f6fb, 0x0f1518),
    hunk_fg: (0x315f8f, 0x7f8d9f),
    context_bg: (0xffffff, 0x0d1110),
    context_gutter_bg: (0xf4f7fa, 0x111818),
    meta_bg: (0xf1f6fb, 0x0f1518),
    add_bg: (0xeaf9ef, 0x12221c),
    add_gutter_bg: (0xd7f0df, 0x174530),
    add_emphasis_bg: ((LIGHT_SUCCESS, 0.18), (DARK_SUCCESS, 0.24)),
    add_border: ((LIGHT_SUCCESS, 0.18), (0x1f6f48, 0.42)),
    remove_bg: (0xfff0f0, 0x231616),
    remove_gutter_bg: (0xf9dddd, 0x4a1e24),
    remove_emphasis_bg: ((LIGHT_DANGER, 0.17), (DARK_DANGER, 0.23)),
    remove_border: ((LIGHT_DANGER, 0.18), (0x8b3038, 0.42)),
};

const GITHUB_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xffffff, 0x0d1117),
    editor_chrome: (0xf6f8fa, 0x161b22),
    editor_surface: (0xffffff, 0x0d1117),
    annotation_bg: (0xf6f8fa, 0x161b22),
    annotation_border: ((0xd0d7de, 0.72), (0x30363d, 0.74)),
    line_hover_bg: ((0xd8ecff, 0.52), (0x58a6ff, 0.10)),
    selected_edge: ((0x0969da, 0.62), (0x58a6ff, 0.72)),
    gutter_separator: ((0xd0d7de, 0.64), (0x30363d, 0.82)),
    hunk_bg: (0xddf4ff, 0x1f2d3d),
    hunk_fg: (0x0969da, 0x79c0ff),
    context_bg: (0xffffff, 0x0d1117),
    context_gutter_bg: (0xf6f8fa, 0x161b22),
    meta_bg: (0xddf4ff, 0x1f2d3d),
    add_bg: (0xdafbe1, 0x0f2a1d),
    add_gutter_bg: (0xaceebb, 0x173a28),
    add_emphasis_bg: ((0x1a7f37, 0.18), (0x3fb950, 0.24)),
    add_border: ((0x1a7f37, 0.22), (0x3fb950, 0.40)),
    remove_bg: (0xffebe9, 0x2d171a),
    remove_gutter_bg: (0xffd7d5, 0x4d1f24),
    remove_emphasis_bg: ((0xcf222e, 0.17), (0xf85149, 0.24)),
    remove_border: ((0xcf222e, 0.22), (0xf85149, 0.40)),
};

const VSCODE_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xffffff, 0x1e1e1e),
    editor_chrome: (0xf3f3f3, 0x252526),
    editor_surface: (0xffffff, 0x1e1e1e),
    annotation_bg: (0xf5f5f5, 0x252526),
    annotation_border: ((0xd4d4d4, 0.72), (0x3c3c3c, 0.78)),
    line_hover_bg: ((0xe8f2ff, 0.55), (0x2a2d2e, 0.88)),
    selected_edge: ((0x007acc, 0.68), (0x007acc, 0.82)),
    gutter_separator: ((0xd4d4d4, 0.58), (0x3c3c3c, 0.82)),
    hunk_bg: (0xeaf4ff, 0x263238),
    hunk_fg: (0x006ab1, 0x4fc1ff),
    context_bg: (0xffffff, 0x1e1e1e),
    context_gutter_bg: (0xf3f3f3, 0x252526),
    meta_bg: (0xeaf4ff, 0x263238),
    add_bg: (0xe6ffed, 0x16301f),
    add_gutter_bg: (0xcdffd8, 0x1f4d2b),
    add_emphasis_bg: ((0x22863a, 0.18), (0x4ec981, 0.25)),
    add_border: ((0x22863a, 0.22), (0x4ec981, 0.42)),
    remove_bg: (0xffebe9, 0x351b1f),
    remove_gutter_bg: (0xffd5d1, 0x5a2228),
    remove_emphasis_bg: ((0xcb2431, 0.17), (0xf48771, 0.25)),
    remove_border: ((0xcb2431, 0.22), (0xf48771, 0.42)),
};

const SOLARIZED_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xfdf6e3, 0x002b36),
    editor_chrome: (0xeee8d5, 0x073642),
    editor_surface: (0xfdf6e3, 0x002b36),
    annotation_bg: (0xeee8d5, 0x073642),
    annotation_border: ((0x93a1a1, 0.42), (0x586e75, 0.58)),
    line_hover_bg: ((0xeee8d5, 0.72), (0x839496, 0.12)),
    selected_edge: ((0x268bd2, 0.68), (0x268bd2, 0.76)),
    gutter_separator: ((0x93a1a1, 0.38), (0x586e75, 0.62)),
    hunk_bg: (0xeee8d5, 0x073642),
    hunk_fg: (0x268bd2, 0x2aa198),
    context_bg: (0xfdf6e3, 0x002b36),
    context_gutter_bg: (0xeee8d5, 0x073642),
    meta_bg: (0xeee8d5, 0x073642),
    add_bg: (0xe7f2d0, 0x163a35),
    add_gutter_bg: (0xd7e8b2, 0x1f4a40),
    add_emphasis_bg: ((0x859900, 0.20), (0x859900, 0.28)),
    add_border: ((0x859900, 0.22), (0x859900, 0.44)),
    remove_bg: (0xf6ded8, 0x3b242c),
    remove_gutter_bg: (0xecc9c1, 0x573038),
    remove_emphasis_bg: ((0xdc322f, 0.18), (0xdc322f, 0.28)),
    remove_border: ((0xdc322f, 0.22), (0xdc322f, 0.44)),
};

const NORD_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xf8fafc, 0x2e3440),
    editor_chrome: (0xeceff4, 0x3b4252),
    editor_surface: (0xffffff, 0x2e3440),
    annotation_bg: (0xeceff4, 0x3b4252),
    annotation_border: ((0xd8dee9, 0.70), (0x4c566a, 0.78)),
    line_hover_bg: ((0xd8dee9, 0.62), (0x434c5e, 0.64)),
    selected_edge: ((0x5e81ac, 0.70), (0x88c0d0, 0.82)),
    gutter_separator: ((0xd8dee9, 0.58), (0x4c566a, 0.82)),
    hunk_bg: (0xe6eef8, 0x344052),
    hunk_fg: (0x5e81ac, 0x88c0d0),
    context_bg: (0xffffff, 0x2e3440),
    context_gutter_bg: (0xeceff4, 0x3b4252),
    meta_bg: (0xe6eef8, 0x344052),
    add_bg: (0xedf7ed, 0x26392f),
    add_gutter_bg: (0xd8ead7, 0x2f4f3b),
    add_emphasis_bg: ((0x6a8f42, 0.18), (0xa3be8c, 0.24)),
    add_border: ((0x6a8f42, 0.22), (0xa3be8c, 0.42)),
    remove_bg: (0xf8eeee, 0x3f2a31),
    remove_gutter_bg: (0xebdada, 0x5a3039),
    remove_emphasis_bg: ((0xbf616a, 0.18), (0xbf616a, 0.28)),
    remove_border: ((0xbf616a, 0.22), (0xbf616a, 0.44)),
};

const GRUVBOX_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xfbf1c7, 0x282828),
    editor_chrome: (0xebdbb2, 0x3c3836),
    editor_surface: (0xfdf4c1, 0x282828),
    annotation_bg: (0xebdbb2, 0x3c3836),
    annotation_border: ((0xd5c4a1, 0.62), (0x665c54, 0.78)),
    line_hover_bg: ((0xebdbb2, 0.64), (0x504945, 0.68)),
    selected_edge: ((0x458588, 0.70), (0x83a598, 0.82)),
    gutter_separator: ((0xd5c4a1, 0.54), (0x665c54, 0.82)),
    hunk_bg: (0xebdbb2, 0x3c3836),
    hunk_fg: (0x458588, 0x83a598),
    context_bg: (0xfbf1c7, 0x282828),
    context_gutter_bg: (0xebdbb2, 0x3c3836),
    meta_bg: (0xebdbb2, 0x3c3836),
    add_bg: (0xe6efd1, 0x30371f),
    add_gutter_bg: (0xd5dfb8, 0x3c4b23),
    add_emphasis_bg: ((0x98971a, 0.20), (0xb8bb26, 0.28)),
    add_border: ((0x98971a, 0.24), (0xb8bb26, 0.44)),
    remove_bg: (0xf4ded8, 0x402020),
    remove_gutter_bg: (0xe6c2b8, 0x5c2a25),
    remove_emphasis_bg: ((0xcc241d, 0.18), (0xfb4934, 0.28)),
    remove_border: ((0xcc241d, 0.24), (0xfb4934, 0.44)),
};

const MONOKAI_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xf8f8f2, 0x272822),
    editor_chrome: (0xefefe8, 0x1f201b),
    editor_surface: (0xfffffb, 0x272822),
    annotation_bg: (0xefefe8, 0x1f201b),
    annotation_border: ((0xd8d8ce, 0.66), (0x49483e, 0.78)),
    line_hover_bg: ((0xe8e8df, 0.66), (0x3e3d32, 0.68)),
    selected_edge: ((0x008aa1, 0.70), (0x66d9ef, 0.82)),
    gutter_separator: ((0xd8d8ce, 0.56), (0x49483e, 0.84)),
    hunk_bg: (0xeef0e8, 0x3e3d32),
    hunk_fg: (0x008aa1, 0x66d9ef),
    context_bg: (0xf8f8f2, 0x272822),
    context_gutter_bg: (0xefefe8, 0x1f201b),
    meta_bg: (0xeef0e8, 0x3e3d32),
    add_bg: (0xeaffd9, 0x253b2f),
    add_gutter_bg: (0xd8f7b8, 0x365327),
    add_emphasis_bg: ((0x6f9a00, 0.20), (0xa6e22e, 0.28)),
    add_border: ((0x6f9a00, 0.24), (0xa6e22e, 0.44)),
    remove_bg: (0xffe7ef, 0x45242e),
    remove_gutter_bg: (0xffccd9, 0x622b3c),
    remove_emphasis_bg: ((0xd51b62, 0.18), (0xf92672, 0.30)),
    remove_border: ((0xd51b62, 0.24), (0xf92672, 0.46)),
};

const HIGH_CONTRAST_DIFF_THEME: DiffThemePalette = DiffThemePalette {
    editor_bg: (0xffffff, 0x000000),
    editor_chrome: (0xf5f5f5, 0x0a0a0a),
    editor_surface: (0xffffff, 0x000000),
    annotation_bg: (0xf2f2f2, 0x111111),
    annotation_border: ((0x000000, 0.28), (0xffffff, 0.34)),
    line_hover_bg: ((0x005cc5, 0.12), (0xffff00, 0.16)),
    selected_edge: ((0x005cc5, 0.86), (0xffff00, 0.92)),
    gutter_separator: ((0x000000, 0.26), (0xffffff, 0.34)),
    hunk_bg: (0xe7f0ff, 0x001a3d),
    hunk_fg: (0x003f8c, 0x79b8ff),
    context_bg: (0xffffff, 0x000000),
    context_gutter_bg: (0xf2f2f2, 0x111111),
    meta_bg: (0xe7f0ff, 0x001a3d),
    add_bg: (0xddffdd, 0x002b12),
    add_gutter_bg: (0xbaffba, 0x00441f),
    add_emphasis_bg: ((0x008000, 0.24), (0x00ff66, 0.28)),
    add_border: ((0x008000, 0.36), (0x00ff66, 0.58)),
    remove_bg: (0xffe0e0, 0x3a0000),
    remove_gutter_bg: (0xffb8b8, 0x5a0000),
    remove_emphasis_bg: ((0xcc0000, 0.24), (0xff4d4d, 0.32)),
    remove_border: ((0xcc0000, 0.36), (0xff4d4d, 0.58)),
};

fn active_diff_palette() -> &'static DiffThemePalette {
    match active_diff_color_theme() {
        DiffColorThemePreference::Graphite => &GRAPHITE_DIFF_THEME,
        DiffColorThemePreference::GitHub => &GITHUB_DIFF_THEME,
        DiffColorThemePreference::VsCode => &VSCODE_DIFF_THEME,
        DiffColorThemePreference::Solarized => &SOLARIZED_DIFF_THEME,
        DiffColorThemePreference::Nord => &NORD_DIFF_THEME,
        DiffColorThemePreference::Gruvbox => &GRUVBOX_DIFF_THEME,
        DiffColorThemePreference::Monokai => &MONOKAI_DIFF_THEME,
        DiffColorThemePreference::HighContrast => &HIGH_CONTRAST_DIFF_THEME,
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

pub fn set_active_code_font_size(preference: CodeFontSizePreference) {
    ACTIVE_CODE_FONT_SIZE.store(preference as u8, Ordering::Relaxed);
}

pub fn set_active_diff_color_theme(preference: DiffColorThemePreference) {
    ACTIVE_DIFF_COLOR_THEME.store(preference as u8, Ordering::Relaxed);
}

pub fn active_theme() -> ActiveTheme {
    match ACTIVE_THEME.load(Ordering::Relaxed) {
        value if value == ActiveTheme::Light as u8 => ActiveTheme::Light,
        _ => ActiveTheme::Dark,
    }
}

pub fn active_code_font_size() -> CodeFontSizePreference {
    match ACTIVE_CODE_FONT_SIZE.load(Ordering::Relaxed) {
        value if value == CodeFontSizePreference::Compact as u8 => CodeFontSizePreference::Compact,
        value if value == CodeFontSizePreference::Large as u8 => CodeFontSizePreference::Large,
        value if value == CodeFontSizePreference::ExtraLarge as u8 => {
            CodeFontSizePreference::ExtraLarge
        }
        _ => CodeFontSizePreference::Default,
    }
}

pub fn active_diff_color_theme() -> DiffColorThemePreference {
    match ACTIVE_DIFF_COLOR_THEME.load(Ordering::Relaxed) {
        value if value == DiffColorThemePreference::GitHub as u8 => {
            DiffColorThemePreference::GitHub
        }
        value if value == DiffColorThemePreference::VsCode as u8 => {
            DiffColorThemePreference::VsCode
        }
        value if value == DiffColorThemePreference::Solarized as u8 => {
            DiffColorThemePreference::Solarized
        }
        value if value == DiffColorThemePreference::Nord as u8 => DiffColorThemePreference::Nord,
        value if value == DiffColorThemePreference::Gruvbox as u8 => {
            DiffColorThemePreference::Gruvbox
        }
        value if value == DiffColorThemePreference::Monokai as u8 => {
            DiffColorThemePreference::Monokai
        }
        value if value == DiffColorThemePreference::HighContrast as u8 => {
            DiffColorThemePreference::HighContrast
        }
        _ => DiffColorThemePreference::Graphite,
    }
}

pub fn code_text_size(base: f32) -> Pixels {
    px(base * active_code_font_size().scale())
}

pub fn code_line_height(base: f32) -> Pixels {
    px((base * active_code_font_size().scale()).ceil())
}

pub fn code_row_height(base: f32) -> Pixels {
    px((base * active_code_font_size().scale()).ceil())
}

pub fn code_measure_width(base: f32) -> f32 {
    base * active_code_font_size().scale()
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

pub fn mono_font_family() -> &'static str {
    MONO_FONT_FAMILY
}

pub fn display_serif_font_family() -> &'static str {
    DISPLAY_SERIF_FONT_FAMILY
}

pub fn bg_canvas() -> Rgba {
    theme_hex(LIGHT_CANVAS, DARK_CANVAS)
}

pub fn bg_surface() -> Rgba {
    theme_hex(LIGHT_SURFACE, DARK_SURFACE)
}

pub fn bg_overlay() -> Rgba {
    theme_hex(LIGHT_ELEVATED, DARK_ELEVATED)
}

pub fn bg_inset() -> Rgba {
    theme_hex(LIGHT_INSET, DARK_INSET)
}

pub fn bg_subtle() -> Rgba {
    theme_hex(LIGHT_SUBTLE, DARK_SUBTLE)
}

pub fn bg_emphasis() -> Rgba {
    theme_hex(LIGHT_EMPHASIS, DARK_EMPHASIS)
}

pub fn bg_selected() -> Rgba {
    theme_hex(LIGHT_SELECTED, DARK_SELECTED)
}

pub fn control_track_bg() -> Rgba {
    match active_theme() {
        ActiveTheme::Light => bg_inset(),
        ActiveTheme::Dark => bg_overlay(),
    }
}

pub fn control_selected_bg() -> Rgba {
    match active_theme() {
        ActiveTheme::Light => bg_surface(),
        ActiveTheme::Dark => bg_inset(),
    }
}

pub fn control_button_bg() -> Rgba {
    bg_subtle()
}

pub fn control_button_hover_bg() -> Rgba {
    bg_emphasis()
}

pub fn focus() -> Rgba {
    theme_hex(LIGHT_FOCUS, DARK_FOCUS)
}

pub fn focus_muted() -> Rgba {
    theme_hex_alpha((LIGHT_FOCUS, 0.12), (DARK_FOCUS, 0.18))
}

pub fn focus_border() -> Rgba {
    theme_hex_alpha((LIGHT_FOCUS, 0.42), (DARK_FOCUS, 0.54))
}

pub fn fg_on_focus() -> Rgba {
    theme_hex(0xffffff, 0x07111f)
}

pub fn primary_action_bg() -> Rgba {
    theme_hex(0x05070c, 0xf3f7fb)
}

pub fn primary_action_hover() -> Rgba {
    theme_hex(0x1f2937, 0xd6dee8)
}

pub fn fg_on_primary_action() -> Rgba {
    theme_hex(0xffffff, 0x07111f)
}

pub fn accent() -> Rgba {
    theme_hex(LIGHT_BRAND_ACCENT, DARK_BRAND_ACCENT)
}

pub fn accent_muted() -> Rgba {
    theme_hex_alpha((LIGHT_BRAND_ACCENT, 0.12), (DARK_BRAND_ACCENT, 0.18))
}

pub fn warning() -> Rgba {
    theme_hex(LIGHT_WARNING, DARK_WARNING)
}

pub fn warning_muted() -> Rgba {
    theme_hex_alpha((LIGHT_WARNING, 0.13), (DARK_WARNING, 0.18))
}

pub fn info() -> Rgba {
    theme_hex(LIGHT_INFO, DARK_INFO)
}

pub fn info_muted() -> Rgba {
    theme_hex_alpha((LIGHT_INFO, 0.12), (DARK_INFO, 0.18))
}

pub fn brand_accent() -> Rgba {
    accent()
}

pub fn brand_accent_muted() -> Rgba {
    accent_muted()
}

pub fn border_default() -> Rgba {
    theme_hex_alpha((LIGHT_BORDER, 0.92), (DARK_BORDER, 0.92))
}

pub fn border_muted() -> Rgba {
    theme_hex_alpha((LIGHT_BORDER_MUTED, 0.90), (DARK_BORDER_MUTED, 0.92))
}

pub fn diff_editor_bg() -> Rgba {
    theme_pair_color(active_diff_palette().editor_bg)
}

pub fn diff_editor_chrome() -> Rgba {
    theme_pair_color(active_diff_palette().editor_chrome)
}

pub fn diff_editor_surface() -> Rgba {
    theme_pair_color(active_diff_palette().editor_surface)
}

pub fn diff_annotation_bg() -> Rgba {
    theme_pair_color(active_diff_palette().annotation_bg)
}

pub fn diff_annotation_border() -> Rgba {
    theme_pair_alpha(active_diff_palette().annotation_border)
}

pub fn diff_line_hover_bg() -> Rgba {
    theme_pair_alpha(active_diff_palette().line_hover_bg)
}

pub fn diff_selected_edge() -> Rgba {
    theme_pair_alpha(active_diff_palette().selected_edge)
}

pub fn diff_gutter_separator() -> Rgba {
    theme_pair_alpha(active_diff_palette().gutter_separator)
}

pub fn diff_hunk_bg() -> Rgba {
    theme_pair_color(active_diff_palette().hunk_bg)
}

pub fn diff_hunk_fg() -> Rgba {
    theme_pair_color(active_diff_palette().hunk_fg)
}

pub fn diff_context_bg() -> Rgba {
    theme_pair_color(active_diff_palette().context_bg)
}

pub fn diff_context_gutter_bg() -> Rgba {
    theme_pair_color(active_diff_palette().context_gutter_bg)
}

pub fn diff_meta_bg() -> Rgba {
    theme_pair_color(active_diff_palette().meta_bg)
}

pub fn diff_add_bg() -> Rgba {
    theme_pair_color(active_diff_palette().add_bg)
}

pub fn diff_add_gutter_bg() -> Rgba {
    theme_pair_color(active_diff_palette().add_gutter_bg)
}

pub fn diff_add_emphasis_bg() -> Rgba {
    theme_pair_alpha(active_diff_palette().add_emphasis_bg)
}

pub fn diff_add_border() -> Rgba {
    theme_pair_alpha(active_diff_palette().add_border)
}

pub fn diff_remove_bg() -> Rgba {
    theme_pair_color(active_diff_palette().remove_bg)
}

pub fn diff_remove_gutter_bg() -> Rgba {
    theme_pair_color(active_diff_palette().remove_gutter_bg)
}

pub fn diff_remove_emphasis_bg() -> Rgba {
    theme_pair_alpha(active_diff_palette().remove_emphasis_bg)
}

pub fn diff_remove_border() -> Rgba {
    theme_pair_alpha(active_diff_palette().remove_border)
}

pub fn fg_default() -> Rgba {
    theme_hex(LIGHT_TEXT, DARK_TEXT)
}

pub fn fg_muted() -> Rgba {
    theme_hex(LIGHT_TEXT_MUTED, DARK_TEXT_MUTED)
}

pub fn fg_subtle() -> Rgba {
    theme_hex(LIGHT_TEXT_SUBTLE, DARK_TEXT_SUBTLE)
}

pub fn fg_emphasis() -> Rgba {
    theme_hex(LIGHT_TEXT_EMPHASIS, DARK_TEXT_EMPHASIS)
}

pub fn success() -> Rgba {
    theme_hex(LIGHT_SUCCESS, DARK_SUCCESS)
}

pub fn success_muted() -> Rgba {
    theme_hex_alpha((LIGHT_SUCCESS, 0.12), (DARK_SUCCESS, 0.16))
}

pub fn danger() -> Rgba {
    theme_hex(LIGHT_DANGER, DARK_DANGER)
}

pub fn danger_muted() -> Rgba {
    theme_hex_alpha((LIGHT_DANGER, 0.12), (DARK_DANGER, 0.16))
}

pub fn waypoint_bg() -> Rgba {
    accent_muted()
}

pub fn waypoint_active_bg() -> Rgba {
    theme_hex_alpha((LIGHT_BRAND_ACCENT, 0.16), (DARK_BRAND_ACCENT, 0.22))
}

pub fn waypoint_border() -> Rgba {
    theme_hex_alpha((LIGHT_BRAND_ACCENT, 0.30), (DARK_BRAND_ACCENT, 0.40))
}

pub fn waypoint_fg() -> Rgba {
    accent()
}

pub fn waypoint_icon_bg() -> Rgba {
    accent_muted()
}

pub fn waypoint_icon_border() -> Rgba {
    waypoint_border()
}

pub fn waypoint_icon_core() -> Rgba {
    accent()
}

pub fn hover_bg() -> Rgba {
    theme_hex(0xf2f5f8, 0x202020)
}

pub fn material_gradient(seed: &str) -> Background {
    match material_index(seed) {
        0 => linear_gradient(
            126.0,
            linear_color_stop(theme_hex(0xd8d1ff, 0x352c72), 0.0),
            linear_color_stop(theme_hex(0x28f3e3, 0x04c4d7), 1.0),
        ),
        1 => linear_gradient(
            132.0,
            linear_color_stop(theme_hex(0xf6b6ff, 0x642c84), 0.0),
            linear_color_stop(theme_hex(0xff4f59, 0xe94b58), 1.0),
        ),
        _ => linear_gradient(
            102.0,
            linear_color_stop(theme_hex(0x37c9ff, 0x0b6fb6), 0.0),
            linear_color_stop(theme_hex(0xffb15c, 0xe57731), 1.0),
        ),
    }
}

pub fn material_glow(seed: &str) -> Background {
    match material_index(seed) {
        0 => linear_gradient(
            58.0,
            linear_color_stop(theme_hex_alpha((0x7cffd5, 0.84), (0x7cffd5, 0.46)), 0.0),
            linear_color_stop(theme_hex_alpha((0xf2ff7a, 0.72), (0xf2ff7a, 0.36)), 1.0),
        ),
        1 => linear_gradient(
            58.0,
            linear_color_stop(theme_hex_alpha((0xffd4f5, 0.76), (0xff8fd8, 0.36)), 0.0),
            linear_color_stop(theme_hex_alpha((0xff7b8a, 0.72), (0xff7b8a, 0.38)), 1.0),
        ),
        _ => linear_gradient(
            58.0,
            linear_color_stop(theme_hex_alpha((0x00f0ff, 0.70), (0x00c2ff, 0.42)), 0.0),
            linear_color_stop(theme_hex_alpha((0xffe173, 0.76), (0xffbd4a, 0.42)), 1.0),
        ),
    }
}

pub fn material_mark(seed: &str) -> Rgba {
    match material_index(seed) {
        0 => theme_hex(0x33f2d8, 0x6ee7f9),
        1 => theme_hex(0xff5d7c, 0xff8fb3),
        _ => theme_hex(0xffa43b, 0xffbd63),
    }
}

fn material_index(seed: &str) -> usize {
    let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
        acc.wrapping_mul(16777619) ^ byte as u32
    });
    (hash as usize) % 3
}

pub fn palette_backdrop() -> Rgba {
    theme_rgba((0.05, 0.09, 0.16, 0.18), (0.0, 0.0, 0.0, 0.72))
}

pub fn topbar_height() -> Pixels {
    px(48.0)
}

pub fn sidebar_width() -> Pixels {
    px(248.0)
}

pub fn file_tree_width() -> Pixels {
    px(292.0)
}

pub fn detail_side_width() -> Pixels {
    px(312.0)
}

pub fn radius() -> Pixels {
    px(12.0)
}

pub fn radius_sm() -> Pixels {
    px(8.0)
}

pub fn radius_lg() -> Pixels {
    px(18.0)
}

pub fn lane_accent_color(repo: &str) -> Rgba {
    let hash: u32 = repo.bytes().fold(5381u32, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as u32)
    });
    let palette = match active_theme() {
        ActiveTheme::Light => [
            hex(LIGHT_FOCUS),
            hex(LIGHT_INFO),
            hex(LIGHT_SUCCESS),
            hex(LIGHT_WARNING),
            hex(0x7c3aed),
            hex(0xdb2777),
            hex(0x4f46e5),
            hex(0x0f766e),
        ],
        ActiveTheme::Dark => [
            hex(DARK_FOCUS),
            hex(DARK_INFO),
            hex(DARK_SUCCESS),
            hex(DARK_WARNING),
            hex(0xc4b5fd),
            hex(0xf9a8d4),
            hex(0xaaa6ff),
            hex(0x5eead4),
        ],
    };
    palette[(hash as usize) % palette.len()]
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_cache() -> CacheStore {
        CacheStore::new(unique_test_path("theme-settings-cache.sqlite3"))
            .expect("failed to create temp cache")
    }

    fn unique_test_path(file_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "remiss-theme-settings-{nanos}-{test_id}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("failed to create temp directory");
        dir.join(file_name)
    }

    #[test]
    fn missing_cache_returns_default_theme_settings() {
        let cache = temp_cache();

        assert_eq!(
            load_theme_settings(&cache).unwrap(),
            ThemeSettings::default()
        );
    }

    #[test]
    fn saved_theme_settings_preserve_review_appearance() {
        let cache = temp_cache();
        let settings = ThemeSettings {
            preference: ThemePreference::Dark,
            code_font_size: CodeFontSizePreference::Large,
            diff_color_theme: DiffColorThemePreference::Monokai,
        };

        save_theme_settings(&cache, &settings).expect("failed to save theme settings");

        assert_eq!(load_theme_settings(&cache).unwrap(), settings);
    }

    #[test]
    fn diff_color_theme_cycle_visits_all_themes() {
        let themes = DiffColorThemePreference::all();
        let mut current = themes[0];

        for expected in themes.iter().copied().skip(1) {
            current = current.next();
            assert_eq!(current, expected);
        }

        assert_eq!(current.next(), themes[0]);
    }

    #[test]
    fn old_theme_settings_without_review_appearance_default_cleanly() {
        let cache = temp_cache();
        cache
            .put(
                THEME_SETTINGS_CACHE_KEY,
                &serde_json::json!({ "preference": "light" }),
                1,
            )
            .expect("failed to save legacy theme settings");

        assert_eq!(
            load_theme_settings(&cache).unwrap(),
            ThemeSettings {
                preference: ThemePreference::Light,
                code_font_size: CodeFontSizePreference::Default,
                diff_color_theme: DiffColorThemePreference::Graphite,
            }
        );
    }

    #[test]
    fn previous_font_size_field_migrates_to_code_font_size() {
        let cache = temp_cache();
        cache
            .put(
                THEME_SETTINGS_CACHE_KEY,
                &serde_json::json!({ "preference": "dark", "fontSize": "extraLarge" }),
                1,
            )
            .expect("failed to save previous theme settings");

        assert_eq!(
            load_theme_settings(&cache).unwrap(),
            ThemeSettings {
                preference: ThemePreference::Dark,
                code_font_size: CodeFontSizePreference::ExtraLarge,
                diff_color_theme: DiffColorThemePreference::Graphite,
            }
        );
    }
}
