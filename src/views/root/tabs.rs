use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use crate::icons::{lucide_icon, LucideIcon};
use crate::theme::*;

use super::super::tooltips::build_chrome_tooltip as build_static_tooltip;

pub(super) fn pr_tab(
    repository: &str,
    number: i64,
    title: &str,
    additions: i64,
    deletions: i64,
    pr_state: &str,
    is_draft: bool,
    is_local: bool,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "pr-tab-{repository}-{number}-{}",
        usize::from(active)
    ));
    let close_id = SharedString::from(format!("pr-tab-close-{repository}-{number}"));
    let tab_bg = if active { bg_emphasis() } else { transparent() };
    let tab_hover_bg = if active { bg_emphasis() } else { bg_selected() };
    let icon_color = pr_tab_state_color(pr_state, is_draft);
    let state_badge = pr_tab_state_badge(pr_state, is_draft);
    let repo_short = repository
        .split('/')
        .last()
        .unwrap_or(repository)
        .to_string();
    let pr_number = format!("#{number}");
    let title = if is_local {
        local_review_tab_title(title)
    } else {
        title.to_string()
    };
    let additions_label = format!("+{additions}");
    let deletions_label = format!("-{deletions}");

    div()
        .relative()
        .h(px(32.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(transparent())
        .bg(tab_bg)
        .text_size(px(12.0))
        .max_w(px(320.0))
        .min_w_0()
        .hover(move |style| style.bg(tab_hover_bg).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            if is_local {
                LucideIcon::GitBranch
            } else {
                LucideIcon::GitPullRequest
            },
            13.0,
            icon_color,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w_0()
                .flex_grow()
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_family(mono_font_family())
                        .text_color(if active { fg_default() } else { fg_subtle() })
                        .flex_shrink_0()
                        .child(repo_short),
                )
                .when(!is_local, |el| {
                    el.child(
                        div()
                            .text_size(px(11.0))
                            .font_family(mono_font_family())
                            .text_color(if active { fg_default() } else { fg_subtle() })
                            .flex_shrink_0()
                            .child(pr_number),
                    )
                })
                .child(
                    div()
                        .min_w_0()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { fg_emphasis() } else { fg_default() })
                        .child(title),
                ),
        )
        .when_some(state_badge, |el, badge| el.child(badge))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .font_family(mono_font_family())
                .text_size(px(11.0))
                .flex_shrink_0()
                .child(div().text_color(success()).child(additions_label))
                .child(
                    div()
                        .text_color(if deletions > 0 { danger() } else { fg_subtle() })
                        .child(deletions_label),
                ),
        )
        .child(compact_close_button(close_id, "Close tab", on_close))
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
            },
        )
}

fn local_review_tab_title(title: &str) -> String {
    let title = title.trim();
    for prefix in ["Local review blocked: ", "Local review: "] {
        if let Some(stripped) = title.strip_prefix(prefix) {
            return stripped.to_string();
        }
    }
    title.to_string()
}

pub(super) fn compact_close_button(
    id: SharedString,
    tooltip: &'static str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(20.0))
        .h(px(20.0))
        .flex_shrink_0()
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(fg_subtle())
        .hover(|style| style.bg(bg_selected()).text_color(fg_emphasis()))
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(lucide_icon(LucideIcon::X, 12.0, fg_subtle()))
}

fn pr_tab_state_color(pr_state: &str, is_draft: bool) -> Rgba {
    if is_draft {
        return fg_muted();
    }

    match pr_state {
        "LOCAL" => accent(),
        "MERGED" => info(),
        "CLOSED" => danger(),
        _ => success(),
    }
}

fn pr_tab_state_badge(pr_state: &str, is_draft: bool) -> Option<AnyElement> {
    if is_draft {
        return Some(pr_tab_badge("draft", fg_muted(), bg_subtle()).into_any_element());
    }

    match pr_state {
        "LOCAL" => Some(pr_tab_badge("local", accent(), accent_muted()).into_any_element()),
        "MERGED" => Some(pr_tab_badge("merged", info(), info_muted()).into_any_element()),
        "CLOSED" => Some(pr_tab_badge("closed", danger(), danger_muted()).into_any_element()),
        _ => None,
    }
}

fn pr_tab_badge(label: &str, fg: Rgba, bg: Rgba) -> impl IntoElement {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(999.0))
        .bg(bg)
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .flex_shrink_0()
        .child(label.to_string())
}
