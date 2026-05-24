use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use crate::theme::*;

pub fn surface_tab(
    label: &str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!("surface-tab-{label}-{}", usize::from(active)));

    div()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .text_size(px(12.0))
        .border_1()
        .border_color(transparent())
        .when(active, |el| el.bg(bg_emphasis()).text_color(fg_emphasis()))
        .when(!active, |el| el.text_color(fg_muted()))
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
                    .text_color(mix_rgba(fg_muted(), fg_emphasis(), progress))
            },
        )
}

pub(super) fn markdown_editor_tab_label(label: &str, active: bool) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(3.0))
        .rounded(radius_sm())
        .text_size(px(12.0))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .child(label.to_string())
}
