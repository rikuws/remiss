use gpui::prelude::*;
use gpui::*;

pub use lucide_icons::Icon as LucideIcon;

const LUCIDE_FONT_FAMILY: &str = "lucide";

pub fn lucide_icon(icon: LucideIcon, size: f32, color: Rgba) -> impl IntoElement {
    div()
        .w(px(size))
        .h(px(size))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .font_family(LUCIDE_FONT_FAMILY)
        .text_size(px(size))
        .line_height(px(size))
        .text_color(color)
        .child(icon.unicode().to_string())
}
