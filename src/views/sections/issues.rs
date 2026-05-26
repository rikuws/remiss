use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;
use crate::theme::*;

use super::{eyebrow, meta_row, nested_panel, panel};

pub(super) fn render_issues(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);

    div()
        .p(px(40.0))
        .px(px(48.0))
        .flex_grow()
        .min_h_0()
        .id("issues-scroll")
        .overflow_y_scroll()
        .max_w(px(960.0))
        .child(
            panel().child(
                div()
                    .p(px(28.0))
                    .px(px(32.0))
                    .child(eyebrow("Deferred"))
                    .child(
                        div()
                            .text_size(px(24.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Issues"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .mt(px(6.0))
                            .max_w(px(480.0))
                            .child("Issues remain intentionally secondary while the MVP concentrates on review flow, PR detail, and write actions."),
                    )
                    .child(
                        nested_panel()
                            .mt(px(16.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child("Backend status"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.0))
                                    .mt(px(12.0))
                                    .child(meta_row(
                                        "gh",
                                        if s.gh_available {
                                            "available"
                                        } else {
                                            "missing"
                                        },
                                    ))
                                    .child(meta_row("Cache", &s.cache_path)),
                            ),
                    ),
            ),
        )
}
