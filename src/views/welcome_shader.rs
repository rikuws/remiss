use std::f32::consts::TAU;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::*;
use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, px, size, Background, Bounds,
    ColorSpace, IntoElement, PathBuilder, Pixels, Point, Rgba, Window,
};

use crate::theme::bg_canvas;

const PERIOD_SECONDS: f32 = 5.0;
const MESH_CELL_SIZE: f32 = 4.0;
pub(super) const WELCOME_SHADER_RADIUS: f32 = 8.0;

#[derive(Clone, Copy)]
struct MeshStop {
    color: Rgba,
    base: (f32, f32),
    drift: (f32, f32),
    radius: f32,
    strength: f32,
    phase: f32,
    // Integer phase harmonic. Non-integer values make the mesh jump at the loop boundary.
    speed: f32,
}

pub(super) fn render_welcome_shader() -> impl IntoElement {
    div()
        .id("overview-welcome-shader")
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .left(px(0.0))
        .w_full()
        .h_full()
        .rounded(px(WELCOME_SHADER_RADIUS))
        .overflow_hidden()
        .child(
            canvas(
                |_bounds, window, _cx| {
                    window.request_animation_frame();
                    shader_time()
                },
                |bounds, time, window, _cx| paint_welcome_mesh_gradient(bounds, time, window),
            )
            .size_full(),
        )
}

fn paint_welcome_mesh_gradient(bounds: Bounds<Pixels>, time: f32, window: &mut Window) {
    let w = bounds.size.width / px(1.0);
    let h = bounds.size.height / px(1.0);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let phase = (time / PERIOD_SECONDS).fract();
    let t = phase * TAU;
    let aspect = w / h.max(1.0);
    let stops = mesh_stops();
    let cell = MESH_CELL_SIZE;
    let cols = (w / cell).ceil() as usize;
    let rows = (h / cell).ceil() as usize;

    for row in 0..rows {
        let y = row as f32 * cell;
        let cell_h = (h - y).min(cell) + 0.75;
        for col in 0..cols {
            let x = col as f32 * cell;
            let cell_w = (w - x).min(cell) + 0.75;
            let u = (x + cell_w * 0.5) / w;
            let v = (y + cell_h * 0.5) / h;

            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                    size(px(cell_w), px(cell_h)),
                ),
                sample_mesh(u, v, aspect, t, &stops),
            ));
        }
    }

    paint_mesh_finish(bounds, window);
    paint_corner_masks(bounds, WELCOME_SHADER_RADIUS, window);
}

fn mesh_stops() -> [MeshStop; 8] {
    [
        MeshStop {
            color: rgba_hex(0x02040a, 0xff),
            base: (-0.10, 0.02),
            drift: (0.06, 0.05),
            radius: 0.72,
            strength: 1.65,
            phase: 0.2,
            speed: 1.0,
        },
        MeshStop {
            color: rgba_hex(0x08172c, 0xff),
            base: (0.15, 0.56),
            drift: (0.12, 0.15),
            radius: 0.48,
            strength: 1.20,
            phase: 1.3,
            speed: 2.0,
        },
        MeshStop {
            color: rgba_hex(0x203a78, 0xff),
            base: (0.42, 0.48),
            drift: (0.18, 0.12),
            radius: 0.36,
            strength: 1.05,
            phase: 3.9,
            speed: 1.0,
        },
        MeshStop {
            color: rgba_hex(0x73a8c5, 0xff),
            base: (0.72, 0.78),
            drift: (0.16, 0.10),
            radius: 0.30,
            strength: 0.86,
            phase: 4.7,
            speed: 2.0,
        },
        MeshStop {
            color: rgba_hex(0xf3eee0, 0xff),
            base: (0.38, 0.90),
            drift: (0.18, 0.08),
            radius: 0.28,
            strength: 1.28,
            phase: 2.6,
            speed: 1.0,
        },
        MeshStop {
            color: rgba_hex(0xff5b24, 0xff),
            base: (0.93, 0.10),
            drift: (0.20, 0.12),
            radius: 0.44,
            strength: 1.35,
            phase: 0.8,
            speed: 2.0,
        },
        MeshStop {
            color: rgba_hex(0x7a2419, 0xff),
            base: (0.72, 0.28),
            drift: (0.20, 0.18),
            radius: 0.38,
            strength: 1.08,
            phase: 5.4,
            speed: 3.0,
        },
        MeshStop {
            color: rgba_hex(0x000105, 0xff),
            base: (1.02, 0.58),
            drift: (0.08, 0.14),
            radius: 0.50,
            strength: 1.42,
            phase: 3.1,
            speed: 1.0,
        },
    ]
}

fn sample_mesh(u: f32, v: f32, aspect: f32, t: f32, stops: &[MeshStop]) -> Rgba {
    let (u, v) = domain_warp(u, v, t);
    let mut r = 0.012;
    let mut g = 0.016;
    let mut b = 0.027;
    let mut total = 0.62;

    for stop in stops {
        let (su, sv) = stop_position(*stop, t);
        let dx = (u - su) * aspect;
        let dy = v - sv;
        let dist2 = dx * dx + dy * dy;
        let radius2 = stop.radius * stop.radius;
        let weight = stop.strength * (-dist2 / radius2).exp();

        r += stop.color.r * weight;
        g += stop.color.g * weight;
        b += stop.color.b * weight;
        total += weight;
    }

    r /= total;
    g /= total;
    b /= total;

    let shade = 1.0 - 0.72 * vignette(u, v);
    let light_sweep = 0.07 * (u * 8.0 - v * 5.2 + t).sin().max(0.0);
    let grain = (hash_noise(u, v) - 0.5) * 0.006;

    Rgba {
        r: (r * shade + light_sweep + grain).clamp(0.0, 1.0),
        g: (g * shade + light_sweep * 0.92 + grain).clamp(0.0, 1.0),
        b: (b * shade + light_sweep * 0.75 + grain).clamp(0.0, 1.0),
        a: 1.0,
    }
}

fn domain_warp(u: f32, v: f32, t: f32) -> (f32, f32) {
    let x = u + 0.035 * (v * 5.0 + t).sin() + 0.020 * ((u + v) * 7.0 - t * 2.0).sin();
    let y = v + 0.030 * (u * 4.5 - t).cos() + 0.018 * ((u - v) * 8.0 + t * 2.0).sin();
    (x, y)
}

fn stop_position(stop: MeshStop, t: f32) -> (f32, f32) {
    let x = stop.base.0
        + stop.drift.0 * (t * stop.speed + stop.phase).sin()
        + stop.drift.0 * 0.38 * (t * (stop.speed + 1.0) - stop.phase).cos();
    let y = stop.base.1
        + stop.drift.1 * (t * stop.speed + stop.phase * 1.37).cos()
        + stop.drift.1 * 0.33 * (t * (stop.speed + 2.0) + stop.phase).sin();
    (x, y)
}

fn vignette(u: f32, v: f32) -> f32 {
    let dx = (u - 0.50).abs() * 1.55;
    let dy = (v - 0.50).abs() * 1.20;
    (dx * dx + dy * dy).clamp(0.0, 1.0)
}

fn hash_noise(u: f32, v: f32) -> f32 {
    let seed = (u * 231.17 + v * 417.31).sin() * 43_758.547;
    seed.fract().abs()
}

fn paint_mesh_finish(bounds: Bounds<Pixels>, window: &mut Window) {
    window.paint_quad(fill(
        bounds,
        gradient(0.0, rgba_hex(0x000000, 0xb2), rgba_hex(0x000000, 0x00)),
    ));
    window.paint_quad(fill(
        bounds,
        gradient(180.0, rgba_hex(0x000000, 0x00), rgba_hex(0x000000, 0xaa)),
    ));
    window.paint_quad(fill(
        bounds,
        gradient(270.0, rgba_hex(0x000000, 0xa8), rgba_hex(0x000000, 0x00)),
    ));
    window.paint_quad(fill(
        bounds,
        gradient(40.0, rgba_hex(0xffffff, 0x16), rgba_hex(0xffffff, 0x00)),
    ));
}

fn paint_corner_masks(bounds: Bounds<Pixels>, radius: f32, window: &mut Window) {
    let w = bounds.size.width / px(1.0);
    let h = bounds.size.height / px(1.0);
    let r = radius.min(w * 0.5).min(h * 0.5);
    let background = bg_canvas();

    paint_corner_mask(
        window,
        &[
            point_xy(bounds, 0.0, 0.0),
            point_xy(bounds, r, 0.0),
            point_xy(bounds, r - r * QUARTER_ARC_KAPPA, 0.0),
            point_xy(bounds, 0.0, r - r * QUARTER_ARC_KAPPA),
            point_xy(bounds, 0.0, r),
            point_xy(bounds, 0.0, 0.0),
        ],
        background,
    );
    paint_corner_mask(
        window,
        &[
            point_xy(bounds, w, 0.0),
            point_xy(bounds, w - r, 0.0),
            point_xy(bounds, w - r + r * QUARTER_ARC_KAPPA, 0.0),
            point_xy(bounds, w, r - r * QUARTER_ARC_KAPPA),
            point_xy(bounds, w, r),
            point_xy(bounds, w, 0.0),
        ],
        background,
    );
    paint_corner_mask(
        window,
        &[
            point_xy(bounds, w, h),
            point_xy(bounds, w, h - r),
            point_xy(bounds, w, h - r + r * QUARTER_ARC_KAPPA),
            point_xy(bounds, w - r + r * QUARTER_ARC_KAPPA, h),
            point_xy(bounds, w - r, h),
            point_xy(bounds, w, h),
        ],
        background,
    );
    paint_corner_mask(
        window,
        &[
            point_xy(bounds, 0.0, h),
            point_xy(bounds, 0.0, h - r),
            point_xy(bounds, 0.0, h - r + r * QUARTER_ARC_KAPPA),
            point_xy(bounds, r - r * QUARTER_ARC_KAPPA, h),
            point_xy(bounds, r, h),
            point_xy(bounds, 0.0, h),
        ],
        background,
    );
}

const QUARTER_ARC_KAPPA: f32 = 0.552_284_8;

fn paint_corner_mask(window: &mut Window, points: &[Point<Pixels>; 6], background: Rgba) {
    let mut builder = PathBuilder::fill();
    builder.move_to(points[0]);
    builder.line_to(points[1]);
    builder.cubic_bezier_to(points[4], points[2], points[3]);
    builder.line_to(points[5]);
    builder.close();

    if let Ok(path) = builder.build() {
        window.paint_path(path, background);
    }
}

fn point_xy(bounds: Bounds<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(bounds.origin.x + px(x), bounds.origin.y + px(y))
}

fn gradient(angle: f32, from: Rgba, to: Rgba) -> Background {
    linear_gradient(
        angle,
        linear_color_stop(from, 0.0),
        linear_color_stop(to, 1.0),
    )
    .color_space(ColorSpace::Oklab)
}

fn rgba_hex(rgb: u32, alpha: u8) -> Rgba {
    gpui::rgba((rgb << 8) | alpha as u32)
}

fn shader_time() -> f32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            let seconds = (duration.as_secs() % 10_000) as f32;
            seconds + duration.subsec_nanos() as f32 / 1_000_000_000.0
        })
        .unwrap_or_default()
}
