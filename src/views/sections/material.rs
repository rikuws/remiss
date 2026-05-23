use gpui::*;

use crate::shader_surface::{
    opengl_shader_surface_variant_with_corner_mask, OverviewShaderVariant, ShaderCornerMask,
};
use crate::theme::*;

pub fn material_surface(seed: &str) -> Div {
    shader_material_surface(
        seed,
        0,
        ShaderCornerMask::default(),
        transparent(),
        radius(),
    )
}

fn shader_material_surface(
    seed: &str,
    variant_offset: usize,
    corners: ShaderCornerMask,
    mask_color: Rgba,
    corner_radius: Pixels,
) -> Div {
    let seed = seed.to_string();
    let variant = material_shader_variant(&seed, variant_offset);
    shader_material_surface_variant(&seed, variant, corners, mask_color, corner_radius, true)
}

pub(super) fn shader_material_surface_variant(
    seed: &str,
    variant: OverviewShaderVariant,
    corners: ShaderCornerMask,
    mask_color: Rgba,
    corner_radius: Pixels,
    use_shader_canvas: bool,
) -> Div {
    if use_shader_canvas {
        let shader_seed = format!("review-material-{seed}");
        return opengl_shader_surface_variant_with_corner_mask(
            shader_seed,
            variant,
            corner_radius,
            mask_color,
            corners,
        );
    }

    static_material_surface_variant(seed, variant, corner_radius)
}

fn static_material_surface_variant(
    seed: &str,
    variant: OverviewShaderVariant,
    corner_radius: Pixels,
) -> Div {
    div()
        .relative()
        .overflow_hidden()
        .rounded(corner_radius)
        .bg(material_surface_base(variant))
        .child(
            div()
                .absolute()
                .inset_0()
                .bg(material_surface_wash(variant)),
        )
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(material_surface_accent_width(seed)))
                .bg(material_surface_accent(variant)),
        )
        .child(
            div()
                .absolute()
                .right(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(1.0))
                .bg(material_surface_edge(variant)),
        )
}

fn material_surface_base(variant: OverviewShaderVariant) -> Rgba {
    match active_theme() {
        ActiveTheme::Light => match variant {
            OverviewShaderVariant::Bands => rgb(0xf3f6f8),
            OverviewShaderVariant::Ember => rgb(0xf8f4f1),
            OverviewShaderVariant::Ribbon => rgb(0xf3f7f3),
            OverviewShaderVariant::Interference => rgb(0xf5f3f8),
        },
        ActiveTheme::Dark => match variant {
            OverviewShaderVariant::Bands => rgb(0x17202a),
            OverviewShaderVariant::Ember => rgb(0x241a17),
            OverviewShaderVariant::Ribbon => rgb(0x172116),
            OverviewShaderVariant::Interference => rgb(0x1e1a26),
        },
    }
}

fn material_surface_wash(variant: OverviewShaderVariant) -> Rgba {
    let alpha = match active_theme() {
        ActiveTheme::Light => 0.10,
        ActiveTheme::Dark => 0.18,
    };
    with_alpha(material_surface_accent(variant), alpha)
}

fn material_surface_accent(variant: OverviewShaderVariant) -> Rgba {
    match variant {
        OverviewShaderVariant::Bands => rgb(0x4f7d95),
        OverviewShaderVariant::Ember => rgb(0xb6684c),
        OverviewShaderVariant::Ribbon => rgb(0x5f8b5d),
        OverviewShaderVariant::Interference => rgb(0x7a6aa7),
    }
}

fn material_surface_edge(variant: OverviewShaderVariant) -> Rgba {
    let alpha = match active_theme() {
        ActiveTheme::Light => 0.18,
        ActiveTheme::Dark => 0.28,
    };
    with_alpha(material_surface_accent(variant), alpha)
}

fn material_surface_accent_width(seed: &str) -> f32 {
    3.0 + material_seed_index(seed) as f32
}

fn material_shader_variant(seed: &str, offset: usize) -> OverviewShaderVariant {
    let variants = OverviewShaderVariant::ALL;
    variants[(material_seed_index(seed) + offset) % variants.len()]
}

fn material_seed_index(seed: &str) -> usize {
    let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
        acc.wrapping_mul(16777619) ^ byte as u32
    });
    (hash as usize) % OverviewShaderVariant::ALL.len()
}
