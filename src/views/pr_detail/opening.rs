use gpui::prelude::*;
use gpui::*;

use crate::github;
use crate::icons::{lucide_icon, LucideIcon};
use crate::theme::*;

use super::{pull_request_state_badge, PR_OVERVIEW_CONTENT_MAX_WIDTH};
use crate::views::sections::format_relative_time;

pub(super) fn render_pull_request_opening_state(
    summary: &github::PullRequestSummary,
) -> impl IntoElement {
    let metadata_ready = !is_placeholder_pull_request_summary(summary);
    let title = if metadata_ready {
        summary.title.clone()
    } else {
        format!("{} #{}", summary.repository, summary.number)
    };
    let status = if metadata_ready {
        "Loading review data"
    } else {
        "Opening pull request"
    };

    div()
        .flex_grow()
        .min_h_0()
        .h_full()
        .bg(bg_canvas())
        .px(px(32.0))
        .py(px(18.0))
        .flex()
        .items_start()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(PR_OVERVIEW_CONTENT_MAX_WIDTH))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(16.0))
                        .pb(px(12.0))
                        .border_b(px(1.0))
                        .border_color(border_muted())
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(format!(
                                            "PULL REQUEST / {} / #{}",
                                            summary.repository, summary.number
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(22.0))
                                        .line_height(px(28.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(title),
                                )
                                .when(metadata_ready, |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(8.0))
                                            .text_size(px(12.0))
                                            .text_color(fg_muted())
                                            .child(pull_request_state_badge(
                                                &summary.state,
                                                summary.is_draft,
                                            ))
                                            .child(format!("by {}", summary.author_login))
                                            .child(format_relative_time(&summary.updated_at)),
                                    )
                                }),
                        )
                        .child(render_compact_opening_status(status)),
                )
                .when(metadata_ready, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(10.0))
                            .child(opening_metric(
                                "Files",
                                summary.changed_files.max(0).to_string(),
                            ))
                            .child(opening_metric(
                                "Diff",
                                format!(
                                    "+{} / -{}",
                                    summary.additions.max(0),
                                    summary.deletions.max(0)
                                ),
                            ))
                            .child(opening_metric(
                                "Comments",
                                summary.comments_count.max(0).to_string(),
                            )),
                    )
                })
                .when(!metadata_ready, |el| {
                    el.child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(fg_muted())
                            .child("Fetching title and PR metadata."),
                    )
                }),
        )
}

fn render_compact_opening_status(label: &'static str) -> AnyElement {
    div()
        .mt(px(2.0))
        .flex_shrink_0()
        .rounded(px(6.0))
        .border_1()
        .border_color(border_muted())
        .bg(bg_inset())
        .px(px(9.0))
        .py(px(6.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .child(lucide_icon(LucideIcon::RefreshCw, 12.0, fg_subtle()))
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(fg_muted())
                .child(label),
        )
        .into_any_element()
}

fn is_placeholder_pull_request_summary(summary: &github::PullRequestSummary) -> bool {
    summary.state == "LOADING" && summary.title == format!("Pull request #{}", summary.number)
}

fn opening_metric(label: &'static str, value: String) -> AnyElement {
    div()
        .rounded(px(6.0))
        .border_1()
        .border_color(border_muted())
        .bg(bg_subtle())
        .px(px(9.0))
        .py(px(6.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(10.0))
                .font_family(mono_font_family())
                .text_color(fg_subtle())
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child(value),
        )
        .into_any_element()
}
