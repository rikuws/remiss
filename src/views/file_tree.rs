use std::{sync::Arc, time::Duration};

use gpui::prelude::*;
use gpui::*;

use crate::{
    icons::{lucide_icon, LucideIcon},
    review_session::{ReviewCenterMode, ReviewSourceTarget},
    state::AppState,
    theme::*,
};

use super::tooltips::{build_static_tooltip, build_text_tooltip};

pub(crate) const REVIEW_FILE_TREE_ROW_HEIGHT: f32 = 30.0;

const REVIEW_FILE_TREE_INDENT_STEP: f32 = 12.0;

pub(crate) type ReviewFileRowOpenHandler =
    Arc<dyn Fn(&Entity<AppState>, ReviewFileRowOpenMode, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy)]
pub(crate) enum ReviewFileRowOpenMode {
    Diff,
    Structural,
    Stack,
    Source,
}

pub(crate) fn render_file_tree_header(
    state: Entity<AppState>,
    file_tree_label: &str,
    visible_file_count: usize,
    diff_totals: Option<(i64, i64)>,
) -> impl IntoElement {
    let state_for_close = state.clone();

    div()
        .px(px(12.0))
        .py(px(10.0))
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child(file_tree_label.to_string()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family(mono_font_family())
                        .flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .text_color(fg_muted())
                                .child(visible_file_count.to_string()),
                        )
                        .when_some(diff_totals, |el, (visible_additions, visible_deletions)| {
                            el.child(div().text_color(fg_subtle()).child("\u{2022}"))
                                .child(
                                    div()
                                        .text_color(success())
                                        .child(format!("+{visible_additions}")),
                                )
                                .child(div().text_color(fg_subtle()).child("/"))
                                .child(
                                    div()
                                        .text_color(danger())
                                        .child(format!("-{visible_deletions}")),
                                )
                        }),
                )
                .child(
                    div()
                        .id("review-file-tree-close")
                        .w(px(22.0))
                        .h(px(22.0))
                        .rounded(radius_sm())
                        .border_1()
                        .border_color(transparent())
                        .bg(transparent())
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .tooltip(|_, cx| build_static_tooltip("Hide file tree", cx))
                        .hover(|style| style.bg(bg_selected()))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            state_for_close.update(cx, |state, cx| {
                                state.set_review_file_tree_visible(false);
                                state.persist_active_review_session();
                                cx.notify();
                            });
                        })
                        .child(lucide_icon(LucideIcon::PanelLeftClose, 14.0, fg_muted()))
                        .with_animation(
                            "review-file-tree-close",
                            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS))
                                .with_easing(ease_in_out),
                            |el, delta| {
                                let progress = selected_reveal_progress(false, delta);
                                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
                            },
                        ),
                ),
        )
}

pub(crate) fn render_structural_warmup_status(status: String) -> impl IntoElement {
    div()
        .px(px(12.0))
        .py(px(7.0))
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .text_size(px(10.0))
        .font_family(mono_font_family())
        .text_color(fg_muted())
        .text_ellipsis()
        .whitespace_nowrap()
        .overflow_x_hidden()
        .child(status)
}

pub(crate) fn render_file_tree_state_message(message: String, is_error: bool) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(8.0))
        .text_size(px(11.0))
        .line_height(px(16.0))
        .text_color(if is_error { danger() } else { fg_muted() })
        .child(message)
}

pub(crate) fn render_file_tree_directory_row(name: String, depth: usize) -> impl IntoElement {
    let name_for_tooltip = name.clone();
    let directory_name_id = name.bytes().fold(depth, |acc, byte| {
        acc.wrapping_mul(33).wrapping_add(byte as usize)
    });

    div()
        .w_full()
        .flex_shrink_0()
        .mb(px(1.0))
        .px(px(6.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .hover(|style| style.bg(hover_bg()))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_grow()
                        .min_w_0()
                        .gap(px(4.0))
                        .pl(review_file_tree_indent(depth))
                        .child(render_file_tree_directory_icon())
                        .child(
                            div()
                                .id(("file-tree-directory-name", directory_name_id))
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_default())
                                .min_w_0()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .tooltip(move |_, cx| {
                                    build_text_tooltip(
                                        SharedString::from(name_for_tooltip.clone()),
                                        cx,
                                    )
                                })
                                .child(name),
                        ),
                ),
        )
}

pub(crate) fn render_file_tree_file_row(
    state: Entity<AppState>,
    path: String,
    file_name: String,
    additions: i64,
    deletions: i64,
    depth: usize,
    selected_path: Option<&str>,
    open_mode: ReviewFileRowOpenMode,
    is_reviewed: bool,
    on_open: ReviewFileRowOpenHandler,
) -> impl IntoElement {
    let is_active = selected_path == Some(path.as_str());
    let file_name_for_tooltip = file_name.clone();
    let file_name_id = path.bytes().fold(5381usize, |acc, byte| {
        acc.wrapping_mul(33).wrapping_add(byte as usize)
    });
    let state_for_open = state.clone();
    let indent = review_file_tree_indent(depth);

    div()
        .w_full()
        .flex_shrink_0()
        .mb(px(1.0))
        .px(px(6.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if is_active {
            bg_emphasis()
        } else {
            transparent()
        })
        .cursor_pointer()
        .hover(move |style| {
            style.bg(if is_active {
                bg_emphasis()
            } else {
                bg_selected()
            })
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let on_open = on_open.clone();
            state_for_open.update(cx, |state, cx| {
                state.selected_file_path = Some(path.clone());
                state.selected_diff_anchor = None;
                match open_mode {
                    ReviewFileRowOpenMode::Diff => {
                        state.set_review_center_mode(ReviewCenterMode::SemanticDiff);
                    }
                    ReviewFileRowOpenMode::Structural => {
                        state.set_review_center_mode(ReviewCenterMode::StructuralDiff);
                    }
                    ReviewFileRowOpenMode::Stack => {
                        state.set_review_center_mode(ReviewCenterMode::GuidedReview);
                    }
                    ReviewFileRowOpenMode::Source => {
                        state.set_review_source_target(ReviewSourceTarget {
                            path: path.clone(),
                            line: None,
                            reason: Some("Selected from file tree".to_string()),
                        });
                    }
                }
                state.persist_active_review_session();
                cx.notify();
            });
            on_open(&state_for_open, open_mode, window, cx);
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .min_w_0()
                        .pl(indent)
                        .when(is_reviewed, |el| {
                            el.child(lucide_icon(LucideIcon::Check, 11.0, success()))
                        })
                        .child(
                            div()
                                .id(("file-tree-file-name", file_name_id))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if is_active {
                                    fg_emphasis()
                                } else if is_reviewed {
                                    fg_muted()
                                } else {
                                    fg_default()
                                })
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .tooltip(move |_, cx| {
                                    build_text_tooltip(
                                        SharedString::from(file_name_for_tooltip.clone()),
                                        cx,
                                    )
                                })
                                .child(file_name),
                        ),
                )
                .when(additions != 0 || deletions != 0, |el| {
                    el.child(render_file_tree_diff_summary(additions, deletions))
                }),
        )
}

fn review_file_tree_indent(depth: usize) -> Pixels {
    px(depth as f32 * REVIEW_FILE_TREE_INDENT_STEP)
}

fn render_file_tree_diff_summary(additions: i64, deletions: i64) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .flex_shrink_0()
        .child(
            div()
                .text_size(px(9.0))
                .font_family(mono_font_family())
                .text_color(success())
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_size(px(9.0))
                .font_family(mono_font_family())
                .text_color(fg_subtle())
                .child("/"),
        )
        .child(
            div()
                .text_size(px(9.0))
                .font_family(mono_font_family())
                .text_color(danger())
                .child(format!("-{deletions}")),
        )
}

fn render_file_tree_directory_icon() -> impl IntoElement {
    lucide_icon(LucideIcon::Folder, 12.0, fg_subtle())
}
