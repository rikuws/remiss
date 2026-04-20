use gpui::{px, Pixels, Rgba};

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

// Charcoal palette sampled from the provided reference UI.
pub fn bg_canvas() -> Rgba {
    hex(0x191b1d)
}

pub fn bg_surface() -> Rgba {
    hex(0x1b1a1d)
}

pub fn bg_overlay() -> Rgba {
    hex(0x1d1f20)
}

pub fn bg_inset() -> Rgba {
    hex(0x161819)
}

pub fn bg_subtle() -> Rgba {
    hex(0x1e2021)
}

pub fn bg_emphasis() -> Rgba {
    hex(0x232526)
}

pub fn bg_selected() -> Rgba {
    hex(0x202224)
}

pub fn accent() -> Rgba {
    hex(0x8fa3b8)
}

pub fn accent_muted() -> Rgba {
    hex_alpha(0x8fa3b8, 0.18)
}

pub fn border_default() -> Rgba {
    hex_alpha(0x828488, 0.24)
}

pub fn border_muted() -> Rgba {
    hex_alpha(0x828488, 0.14)
}

pub fn diff_hunk_bg() -> Rgba {
    hex(0x1a2026)
}

pub fn diff_hunk_fg() -> Rgba {
    hex(0xa4b6c8)
}

pub fn diff_context_bg() -> Rgba {
    hex(0x191b1d)
}

pub fn diff_context_gutter_bg() -> Rgba {
    hex(0x1b1a1d)
}

pub fn diff_meta_bg() -> Rgba {
    hex(0x1d1f20)
}

// Subtle diff tints so syntax highlighting still dominates.
pub fn diff_add_bg() -> Rgba {
    hex(0x16201a)
}

pub fn diff_add_gutter_bg() -> Rgba {
    hex(0x1a251e)
}

pub fn diff_add_border() -> Rgba {
    transparent()
}

pub fn diff_remove_bg() -> Rgba {
    hex(0x22181b)
}

pub fn diff_remove_gutter_bg() -> Rgba {
    hex(0x2a1d20)
}

pub fn diff_remove_border() -> Rgba {
    transparent()
}

// Foreground colors
pub fn fg_default() -> Rgba {
    hex(0xc7cbcf)
}

pub fn fg_muted() -> Rgba {
    hex(0x9a9ea3)
}

pub fn fg_subtle() -> Rgba {
    hex(0x828488)
}

pub fn fg_emphasis() -> Rgba {
    hex(0xf2f3f5)
}

pub fn success() -> Rgba {
    hex(0x79be84)
}

pub fn success_muted() -> Rgba {
    hex_alpha(0x79be84, 0.14)
}

pub fn danger() -> Rgba {
    hex(0xe1848d)
}

pub fn danger_muted() -> Rgba {
    hex_alpha(0xe1848d, 0.14)
}

pub fn purple() -> Rgba {
    hex(0xb396df)
}

pub fn waypoint_bg() -> Rgba {
    hex_alpha(0xb396df, 0.16)
}

pub fn waypoint_active_bg() -> Rgba {
    hex_alpha(0xb396df, 0.24)
}

pub fn waypoint_border() -> Rgba {
    hex_alpha(0xb396df, 0.34)
}

pub fn waypoint_fg() -> Rgba {
    hex(0xe3d4fb)
}

pub fn hover_bg() -> Rgba {
    hex(0x232526)
}

pub fn palette_backdrop() -> Rgba {
    color(0.02, 0.02, 0.03, 0.58)
}

// Sizes
pub fn topbar_height() -> Pixels {
    px(48.0)
}

pub fn sidebar_width() -> Pixels {
    px(260.0)
}

pub fn file_tree_width() -> Pixels {
    px(240.0)
}

pub fn detail_side_width() -> Pixels {
    px(280.0)
}

pub fn radius() -> Pixels {
    px(6.0)
}

pub fn radius_sm() -> Pixels {
    px(4.0)
}

pub fn lane_accent_color(repo: &str) -> Rgba {
    let hash: u32 = repo.bytes().fold(5381u32, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as u32)
    });
    let palette = [
        hex(0x7fbe89), // green
        hex(0xd19a68), // orange
        hex(0xb494e0), // purple
        hex(0x88a8c3), // blue
        hex(0xc98ca5), // pink
        hex(0xbaa3d8), // lavender
        hex(0x7fb5bb), // teal
        hex(0xd3bf6f), // yellow
    ];
    palette[(hash as usize) % palette.len()]
}
