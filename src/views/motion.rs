use gpui::{px, Pixels, Rgba};

pub(crate) fn lerp_f32(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

pub(crate) fn lerp_px(from: f32, to: f32, progress: f32) -> Pixels {
    px(lerp_f32(from, to, progress))
}

pub(crate) fn lerp_rgba(from: Rgba, to: Rgba, progress: f32) -> Rgba {
    Rgba {
        r: lerp_f32(from.r, to.r, progress),
        g: lerp_f32(from.g, to.g, progress),
        b: lerp_f32(from.b, to.b, progress),
        a: lerp_f32(from.a, to.a, progress),
    }
}

#[cfg(test)]
mod tests {
    use super::{lerp_f32, lerp_px, lerp_rgba};

    #[test]
    fn interpolates_scalar_and_pixel_values() {
        assert_eq!(lerp_f32(10.0, 20.0, 0.25), 12.5);
        assert_eq!(f32::from(lerp_px(-8.0, 0.0, 0.5)), -4.0);
    }

    #[test]
    fn interpolates_color_channels() {
        let from = gpui::rgba(0x10203040);
        let to = gpui::rgba(0x50607080);
        let color = lerp_rgba(from, to, 0.5);

        assert!((color.r - 0.1882353).abs() < f32::EPSILON);
        assert!((color.g - 0.2509804).abs() < f32::EPSILON);
        assert!((color.b - 0.3137255).abs() < f32::EPSILON);
        assert!((color.a - 0.3764706).abs() < f32::EPSILON);
    }
}
