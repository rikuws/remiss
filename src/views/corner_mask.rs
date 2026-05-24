use gpui::*;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct CornerMask {
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_right: bool,
    pub bottom_left: bool,
}

impl CornerMask {
    pub(crate) const ALL: Self = Self {
        top_left: true,
        top_right: true,
        bottom_right: true,
        bottom_left: true,
    };
}

pub(crate) fn render_corner_mask(
    radius: Pixels,
    mask_color: Rgba,
    corners: CornerMask,
) -> impl IntoElement {
    canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            paint_corner_mask(window, bounds, radius, mask_color, corners);
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn paint_corner_mask(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    radius: Pixels,
    color: Rgba,
    corners: CornerMask,
) {
    let radius = f32::from(radius)
        .min(f32::from(bounds.size.width) / 2.0)
        .min(f32::from(bounds.size.height) / 2.0);
    if radius <= 0.0 {
        return;
    }

    let radius = px(radius);
    let control = px(f32::from(radius) * 0.552_284_8);
    let left = bounds.left();
    let right = bounds.right();
    let top = bounds.top();
    let bottom = bounds.bottom();

    if corners.top_left {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(left, top));
        builder.line_to(point(left + radius, top));
        builder.cubic_bezier_to(
            point(left, top + radius),
            point(left + radius - control, top),
            point(left, top + radius - control),
        );
        builder.line_to(point(left, top));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.top_right {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(right, top));
        builder.line_to(point(right - radius, top));
        builder.cubic_bezier_to(
            point(right, top + radius),
            point(right - radius + control, top),
            point(right, top + radius - control),
        );
        builder.line_to(point(right, top));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.bottom_right {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(right, bottom));
        builder.line_to(point(right, bottom - radius));
        builder.cubic_bezier_to(
            point(right - radius, bottom),
            point(right, bottom - radius + control),
            point(right - radius + control, bottom),
        );
        builder.line_to(point(right, bottom));
        builder.close();
        paint_mask_path(window, builder, color);
    }

    if corners.bottom_left {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(left, bottom));
        builder.line_to(point(left, bottom - radius));
        builder.cubic_bezier_to(
            point(left + radius, bottom),
            point(left, bottom - radius + control),
            point(left + radius - control, bottom),
        );
        builder.line_to(point(left, bottom));
        builder.close();
        paint_mask_path(window, builder, color);
    }
}

fn paint_mask_path(window: &mut Window, builder: PathBuilder, color: Rgba) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}
