use std::{collections::BTreeSet, time::Duration};

use gpui::prelude::*;
use gpui::*;

use crate::icons::LucideIcon;
use crate::state::AppState;
use crate::theme::*;
use crate::views::motion::lerp_px;

use super::{chrome_icon_button, APP_CHROME_HEIGHT, NOTIFICATION_DRAWER_ANIMATION_MS};

pub(super) fn render_notification_drawer(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let active_detail = s.active_detail();
    let unread_ids = active_detail
        .map(|detail| s.unread_review_comment_ids_for_detail(detail))
        .unwrap_or_default();
    let unread_id_set = unread_ids.iter().cloned().collect::<BTreeSet<_>>();
    let unread_items = active_detail
        .map(|detail| {
            detail
                .review_threads
                .iter()
                .flat_map(|thread| {
                    thread.comments.iter().filter_map(|comment| {
                        unread_id_set.contains(&comment.id).then(|| {
                            (
                                comment.id.clone(),
                                comment.author_login.clone(),
                                comment.path.clone(),
                                comment.line.or(comment.original_line),
                                truncate_drawer_text(&comment.body, 96),
                            )
                        })
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let state_for_close = state.clone();
    let state_for_mark_read = state.clone();
    let comment_label = if unread_items.len() == 1 {
        "comment"
    } else {
        "comments"
    };

    div()
        .absolute()
        .top(px(APP_CHROME_HEIGHT))
        .right(px(16.0))
        .w(px(360.0))
        .max_h(px(520.0))
        .rounded(radius())
        .border_1()
        .border_color(transparent())
        .bg(bg_overlay())
        .shadow(popover_shadow())
        .occlude()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .border_b(px(1.0))
                .border_color(border_muted())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Unread review activity"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(fg_muted())
                                .child(format!("{} unread {comment_label}", unread_items.len())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .when(!unread_ids.is_empty(), |el| {
                            let unread_ids = unread_ids.clone();
                            el.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(radius_sm())
                                    .text_size(px(11.0))
                                    .text_color(fg_muted())
                                    .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        state_for_mark_read.update(cx, |state, cx| {
                                            state.mark_review_comments_read(unread_ids.clone());
                                            state.notification_drawer_open = false;
                                            cx.notify();
                                        });
                                    })
                                    .child("Mark read"),
                            )
                        })
                        .child(chrome_icon_button(
                            "notification-drawer-close",
                            LucideIcon::X,
                            "Close notifications",
                            false,
                            move |_, _, cx| {
                                state_for_close.update(cx, |state, cx| {
                                    state.notification_drawer_open = false;
                                    cx.notify();
                                });
                            },
                        )),
                ),
        )
        .child(
            div()
                .id("notification-drawer-scroll")
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(px(10.0))
                .gap(px(8.0))
                .when(unread_items.is_empty(), |el| {
                    el.child(
                        div()
                            .px(px(10.0))
                            .py(px(18.0))
                            .rounded(radius_sm())
                            .border_1()
                            .border_color(transparent())
                            .bg(bg_surface())
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .child("No unread review comments."),
                    )
                })
                .children(
                    unread_items
                        .into_iter()
                        .map(|(_id, author, path, line, body)| {
                            div()
                                .rounded(radius_sm())
                                .border_1()
                                .border_color(transparent())
                                .bg(bg_surface())
                                .px(px(10.0))
                                .py(px(9.0))
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child(author),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .font_family(mono_font_family())
                                                .text_color(fg_muted())
                                                .child(
                                                    line.map(|line| format!("L{line}"))
                                                        .unwrap_or_default(),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_muted())
                                        .overflow_x_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(path),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .line_height(px(17.0))
                                        .text_color(fg_default())
                                        .child(body),
                                )
                        }),
                ),
        )
        .with_animation(
            "notification-drawer-open",
            Animation::new(Duration::from_millis(NOTIFICATION_DRAWER_ANIMATION_MS))
                .with_easing(ease_in_out),
            move |el, delta| {
                el.mt(lerp_px(-8.0, 0.0, delta))
                    .opacity(delta.clamp(0.0, 1.0))
            },
        )
}

fn truncate_drawer_text(text: &str, limit: usize) -> String {
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() <= limit {
        trimmed
    } else {
        let mut out = trimmed.chars().take(limit).collect::<String>();
        out.push('…');
        out
    }
}
