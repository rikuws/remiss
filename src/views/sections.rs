use gpui::prelude::*;
use gpui::*;

use crate::icons::{lucide_icon, LucideIcon};
use crate::review_session::{load_review_session, location_label};
use crate::shader_surface::{
    opengl_shader_surface_variant_with_corner_mask, OverviewShaderVariant, ShaderCornerMask,
};
use crate::state::*;
use crate::theme::*;
use crate::{github, notifications};

use super::diff_view::{ensure_structural_diff_warmup_started, warm_structural_diffs_flow};
use super::settings::render_settings_view;
use super::workspace_sync::trigger_sync_workspace;
use std::{collections::BTreeMap, time::Duration};

const DETAIL_AUTO_REFRESH_TTL_MS: i64 = 5 * 60 * 1000;
const OVERVIEW_CONTENT_MAX_WIDTH: f32 = 1440.0;
const KANBAN_LANE_WIDTH: f32 = 320.0;
const KANBAN_LANE_SCROLLBAR_WIDTH: f32 = 8.0;

pub fn render_section_workspace(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    match s.active_section {
        SectionId::Overview => render_overview(state, cx).into_any_element(),
        SectionId::Pulls | SectionId::Reviews => render_pull_list(state, cx).into_any_element(),
        SectionId::Issues => render_issues(state, cx).into_any_element(),
        SectionId::Settings => render_settings_view(state, cx).into_any_element(),
    }
}

fn render_overview(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let viewer_name = s.viewer_name().to_string();
    let is_auth = s.is_authenticated();
    let review_count = s.review_queue().map(|q| q.total_count).unwrap_or(0);
    let pull_count = s.section_count(SectionId::Pulls);
    let issue_count = s.section_count(SectionId::Issues);
    let review_items: Vec<_> = s
        .review_queue()
        .map(|q| q.items.clone())
        .unwrap_or_default();
    let workspace_loading = s.workspace_loading;
    let workspace_error = s.workspace_error.clone();
    let authored_comment_items =
        overview_pull_request_comment_items(&s, OverviewCommentBucket::Authored);
    let other_comment_items = overview_pull_request_comment_items(&s, OverviewCommentBucket::Other);

    let welcome_greeting = overview_welcome_greeting(&viewer_name, is_auth);
    let state_for_pull_requests = state.clone();
    let state_for_review_requests = state.clone();
    let state_for_items = state.clone();
    let state_for_authored_comments = state.clone();
    let state_for_other_comments = state.clone();
    let show_empty_state = is_auth
        && !workspace_loading
        && workspace_error.is_none()
        && review_items.is_empty()
        && authored_comment_items.is_empty()
        && other_comment_items.is_empty();

    div()
        .p(px(28.0))
        .px(px(40.0))
        .flex()
        .flex_col()
        .gap(px(18.0))
        .flex_grow()
        .min_h_0()
        .h_full()
        .overflow_hidden()
        .child(
            div().w_full().flex().justify_center().child(
                overview_content_shell()
                    .flex_shrink_0()
                    .child(overview_header(welcome_greeting)),
            ),
        )
        .child(
            div()
                .id("overview-scroll")
                .w_full()
                .flex()
                .justify_center()
                .flex_grow()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    overview_content_shell()
                        .flex()
                        .flex_col()
                        .gap(px(18.0))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_wrap()
                                .gap(px(12.0))
                                .child(overview_metric_card(
                                    LucideIcon::GitPullRequest,
                                    "Open Pull Requests",
                                    pull_count,
                                    is_auth,
                                    {
                                        let state = state_for_pull_requests.clone();
                                        move |_, _, cx| {
                                            activate_queue(
                                                &state,
                                                SectionId::Pulls,
                                                "authored",
                                                cx,
                                            );
                                        }
                                    },
                                ))
                                .child(overview_metric_card(
                                    LucideIcon::Inbox,
                                    "Open Issues",
                                    issue_count,
                                    false,
                                    |_, _, _| {},
                                ))
                                .child(overview_metric_card(
                                    LucideIcon::MessageSquareCheck,
                                    "Review Requests",
                                    review_count,
                                    is_auth,
                                    {
                                        let state = state_for_review_requests.clone();
                                        move |_, _, cx| {
                                            activate_queue(
                                                &state,
                                                SectionId::Reviews,
                                                "reviewRequested",
                                                cx,
                                            );
                                        }
                                    },
                                )),
                        )
                        .when(show_empty_state, |el| {
                            el.child(overview_empty_state_panel())
                        })
                        .when(!show_empty_state, |el| {
                            el.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_wrap()
                                    .items_start()
                                    .gap(px(18.0))
                                    .child(div().flex_1().min_w(px(640.0)).child(
                                        overview_review_requests_panel(
                                            review_items,
                                            review_count,
                                            workspace_loading,
                                            workspace_error.clone(),
                                            is_auth,
                                            state_for_items.clone(),
                                        ),
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(380.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(18.0))
                                            .child(overview_pull_request_comment_panel(
                                                OverviewCommentBucket::Authored,
                                                authored_comment_items.clone(),
                                                workspace_loading,
                                                workspace_error.clone(),
                                                is_auth,
                                                state_for_authored_comments.clone(),
                                            ))
                                            .child(overview_pull_request_comment_panel(
                                                OverviewCommentBucket::Other,
                                                other_comment_items.clone(),
                                                workspace_loading,
                                                workspace_error.clone(),
                                                is_auth,
                                                state_for_other_comments.clone(),
                                            )),
                                    ),
                            )
                        }),
                ),
        )
}

fn overview_content_shell() -> Div {
    div().w_full().max_w(px(OVERVIEW_CONTENT_MAX_WIDTH))
}

#[derive(Clone, Copy)]
enum OverviewCommentBucket {
    Authored,
    Other,
}

#[derive(Clone)]
struct OverviewReviewCommentItem {
    summary: github::PullRequestSummary,
    author_login: String,
    author_avatar_url: Option<String>,
    location: String,
    preview: String,
    timestamp: String,
    unread: bool,
    is_resolved: bool,
    is_outdated: bool,
}

fn overview_header(welcome_greeting: String) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .flex_shrink_0()
        .py(px(4.0))
        .min_h(px(40.0))
        .child(
            div()
                .min_w_0()
                .text_size(px(27.0))
                .line_height(px(32.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .line_clamp(1)
                .child(welcome_greeting),
        )
}
fn overview_metric_card(
    icon: LucideIcon,
    label: &str,
    count: i64,
    interactive: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(220.0))
        .min_h(px(70.0))
        .flex()
        .items_center()
        .gap(px(14.0))
        .rounded(radius())
        .bg(bg_overlay())
        .px(px(18.0))
        .py(px(14.0))
        .when(interactive, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(bg_emphasis()).text_color(fg_emphasis()))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(lucide_icon(icon, 22.0, fg_muted())),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_grow()
                .child(
                    div()
                        .text_size(px(16.0))
                        .line_height(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(count.to_string()),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .line_height(px(17.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_muted())
                        .line_clamp(1)
                        .child(label.to_string()),
                ),
        )
}

fn overview_empty_state_panel() -> impl IntoElement {
    panel().child(
        div()
            .p(px(28.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(20.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(eyebrow("All clear"))
                    .child(
                        div()
                            .text_size(px(22.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("No review work needs attention."),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(fg_muted())
                            .child(
                                "No comments from others or review requests were found in the current workspace snapshot.",
                            ),
                    ),
            )
    )
}

fn overview_pull_request_comment_panel(
    bucket: OverviewCommentBucket,
    items: Vec<OverviewReviewCommentItem>,
    workspace_loading: bool,
    workspace_error: Option<String>,
    is_auth: bool,
    state: Entity<AppState>,
) -> impl IntoElement {
    let has_unread = items.iter().any(|item| item.unread);
    let (eyebrow_text, title, loading, error, unread, latest, empty, unauthenticated) = match bucket
    {
        OverviewCommentBucket::Authored => (
            "Your PR comments",
            "Comments on Your Pull Requests",
            "Checking comments on pull requests you opened.",
            "Workspace sync needs attention before comments on your pull requests can refresh.",
            "Unread review comments from others on pull requests you opened.",
            "Latest comments from others on pull requests you opened.",
            "No comments from others on your pull requests are waiting in the current workspace.",
            "Authenticate with gh to populate comments on your pull requests.",
        ),
        OverviewCommentBucket::Other => (
            "Other PR comments",
            "Comments on Other Pull Requests",
            "Checking comments on other pull requests in your workspace.",
            "Workspace sync needs attention before other pull request comments can refresh.",
            "Unread review comments from others on pull requests opened by other people.",
            "Latest comments from others on pull requests opened by other people.",
            "No comments from others on other pull requests are waiting in the current workspace.",
            "Authenticate with gh to populate comments on other pull requests.",
        ),
    };
    let copy = if workspace_loading {
        loading
    } else if workspace_error.is_some() {
        error
    } else if has_unread {
        unread
    } else {
        latest
    };

    panel().child(
        div()
            .p(px(20.0))
            .px(px(22.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .min_w_0()
                    .child(eyebrow(eyebrow_text))
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(fg_muted())
                            .child(copy),
                    ),
            )
            .when(workspace_loading, |el| el.child(panel_state_text(loading)))
            .when_some(workspace_error.clone(), |el, err| {
                el.child(error_text(&err))
            })
            .when(
                !workspace_loading && workspace_error.is_none() && items.is_empty(),
                |el| {
                    el.child(panel_state_text(if is_auth {
                        empty
                    } else {
                        unauthenticated
                    }))
                },
            )
            .child(
                div().flex().flex_col().gap(px(10.0)).children(
                    items
                        .into_iter()
                        .map(|item| overview_review_comment_row(item, state.clone())),
                ),
            ),
    )
}

fn overview_review_requests_panel(
    review_items: Vec<github::PullRequestSummary>,
    review_count: i64,
    workspace_loading: bool,
    workspace_error: Option<String>,
    is_auth: bool,
    state: Entity<AppState>,
) -> impl IntoElement {
    let visible_count = review_items.len().min(8);
    let remaining_count = review_count.saturating_sub(visible_count as i64);
    let visible_items = review_items.into_iter().take(8).collect::<Vec<_>>();

    panel().child(
        div()
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(overview_request_group_header(
                "Review requested",
                review_count,
            ))
            .when(workspace_loading, |el| {
                el.child(
                    div()
                        .px(px(8.0))
                        .child(panel_state_text("Loading review requests...")),
                )
            })
            .when_some(workspace_error.clone(), |el, err| {
                el.child(div().px(px(8.0)).child(error_text(&err)))
            })
            .when(
                !workspace_loading && workspace_error.is_none() && visible_items.is_empty(),
                |el| {
                    el.child(div().px(px(8.0)).child(panel_state_text(if is_auth {
                        "No pull requests are currently requesting your review."
                    } else {
                        "Authenticate with gh to populate the review queue."
                    })))
                },
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(visible_items.into_iter().map(|item| {
                        let state = state.clone();
                        overview_review_request_row(item, move |summary, window, cx| {
                            open_pull_request(&state, summary, window, cx);
                        })
                    })),
            )
            .when(remaining_count > 0, |el| {
                el.child(
                    div()
                        .px(px(12.0))
                        .pt(px(4.0))
                        .text_size(px(12.0))
                        .text_color(fg_subtle())
                        .child(format!("{remaining_count} more in the review board")),
                )
            }),
    )
}

fn overview_request_group_header(label: &str, count: i64) -> impl IntoElement {
    div()
        .min_h(px(44.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .rounded(radius_sm())
        .bg(bg_surface())
        .text_color(fg_emphasis())
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(lucide_icon(LucideIcon::ListChecks, 15.0, fg_muted()))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .line_clamp(1)
                        .child(label.to_string()),
                ),
        )
        .child(
            div()
                .font_family(mono_font_family())
                .text_size(px(16.0))
                .text_color(fg_subtle())
                .child(count.to_string()),
        )
}

fn overview_review_request_row(
    item: github::PullRequestSummary,
    on_click: impl Fn(github::PullRequestSummary, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = item.title.clone();
    let repo_ref = format!("{} #{}", item.repository, item.number);
    let author_login = item.author_login.clone();
    let author_avatar_url = item.author_avatar_url.clone();
    let updated = format_relative_time(&item.updated_at);
    let summary = item.clone();

    div()
        .w_full()
        .min_h(px(74.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .flex()
        .items_center()
        .gap(px(14.0))
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(bg_surface())
                .border_color(border_muted())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(summary.clone(), window, cx)
        })
        .child(lucide_icon(LucideIcon::GitBranch, 15.0, success()))
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .line_height(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .line_clamp(1)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(fg_muted())
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(repo_ref),
                ),
        )
        .child(
            div()
                .w(px(148.0))
                .flex_shrink_0()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(user_avatar(
                    &author_login,
                    author_avatar_url.as_deref(),
                    17.0,
                    false,
                ))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(author_login),
                ),
        )
        .child(
            div()
                .w(px(64.0))
                .flex_shrink_0()
                .text_align(TextAlign::Right)
                .text_size(px(12.0))
                .text_color(fg_subtle())
                .child(updated),
        )
}

fn overview_review_comment_row(
    item: OverviewReviewCommentItem,
    state: Entity<AppState>,
) -> impl IntoElement {
    let summary = item.summary.clone();
    let repo_ref = format!("{} #{}", summary.repository, summary.number);
    let title = summary.title.clone();
    let author_login = item.author_login.clone();
    let author_avatar_url = item.author_avatar_url.clone();
    let meta = format!("commented {}", format_relative_time(&item.timestamp));
    let location = item.location.clone();
    let preview = item.preview.clone();

    div()
        .w_full()
        .relative()
        .pl(px(34.0))
        .pr(px(10.0))
        .py(px(12.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_pull_request(&state, summary.clone(), window, cx);
        })
        .child(
            div()
                .absolute()
                .left(px(4.0))
                .top(px(14.0))
                .child(user_avatar(
                    &author_login,
                    author_avatar_url.as_deref(),
                    22.0,
                    false,
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(14.0))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(repo_ref),
                                )
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .text_size(px(14.0))
                                        .line_clamp(1)
                                        .child(title),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .flex_wrap()
                                .justify_end()
                                .when(item.unread, |el| {
                                    el.child(pill_badge("new", accent(), accent_muted(), accent()))
                                })
                                .when(item.is_resolved, |el| el.child(subtle_pill("resolved")))
                                .when(item.is_outdated, |el| el.child(subtle_pill("outdated"))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .child(author_login),
                        )
                        .child(meta),
                )
                .child(
                    div()
                        .text_size(px(14.0))
                        .line_height(px(22.0))
                        .text_color(fg_default())
                        .line_clamp(2)
                        .child(preview),
                )
                .child(
                    div()
                        .min_w_0()
                        .font_family(mono_font_family())
                        .text_size(px(10.0))
                        .text_color(fg_subtle())
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(location),
                ),
        )
}

fn overview_pull_request_comment_items(
    state: &AppState,
    bucket: OverviewCommentBucket,
) -> Vec<OverviewReviewCommentItem> {
    let mut summaries = BTreeMap::new();
    if let Some(workspace) = state.workspace.as_ref() {
        for item in workspace.queues.iter().flat_map(|queue| &queue.items) {
            summaries
                .entry(pr_key(&item.repository, item.number))
                .or_insert_with(|| item.clone());
        }
    }

    let viewer_login = state.viewer_login().unwrap_or_default();
    let mut unread_items = Vec::new();
    let mut latest_items = Vec::new();

    for (key, detail_state) in &state.detail_states {
        let Some(summary) = summaries.get(key) else {
            continue;
        };
        let Some(detail) = detail_state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.detail.as_ref())
        else {
            continue;
        };

        if !overview_detail_matches_comment_bucket(summary, detail, viewer_login, bucket) {
            continue;
        }

        for comment in &detail.comments {
            if comment.body.trim().is_empty() || comment.author_login == viewer_login {
                continue;
            }

            latest_items.push(overview_item_for_pull_request_comment(summary, comment));
        }

        for thread in &detail.review_threads {
            let mut latest_foreign_comment = None;
            for comment in &thread.comments {
                if comment.body.trim().is_empty() || comment.author_login == viewer_login {
                    continue;
                }

                if state.unread_review_comment_ids.contains(&comment.id) {
                    unread_items.push(overview_comment_item_for_comment(
                        summary, thread, comment, true,
                    ));
                } else {
                    latest_foreign_comment = Some(comment);
                }
            }

            if let Some(comment) = latest_foreign_comment {
                latest_items.push(overview_comment_item_for_comment(
                    summary, thread, comment, false,
                ));
            }
        }
    }

    let mut items = if unread_items.is_empty() {
        latest_items
    } else {
        unread_items
    };
    items.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.location.cmp(&right.location))
    });
    items.truncate(5);
    items
}

fn overview_detail_matches_comment_bucket(
    summary: &github::PullRequestSummary,
    detail: &github::PullRequestDetail,
    viewer_login: &str,
    bucket: OverviewCommentBucket,
) -> bool {
    if viewer_login.is_empty() {
        return false;
    }

    let authored_by_viewer =
        summary.author_login == viewer_login || detail.author_login == viewer_login;
    match bucket {
        OverviewCommentBucket::Authored => authored_by_viewer,
        OverviewCommentBucket::Other => !authored_by_viewer,
    }
}

fn overview_comment_item_for_comment(
    summary: &github::PullRequestSummary,
    thread: &github::PullRequestReviewThread,
    comment: &github::PullRequestReviewComment,
    unread: bool,
) -> OverviewReviewCommentItem {
    OverviewReviewCommentItem {
        summary: summary.clone(),
        author_login: comment.author_login.clone(),
        author_avatar_url: comment.author_avatar_url.clone(),
        location: overview_comment_location(thread, comment),
        preview: summarize_overview_comment(&comment.body),
        timestamp: comment
            .published_at
            .clone()
            .unwrap_or_else(|| comment.updated_at.clone()),
        unread,
        is_resolved: thread.is_resolved,
        is_outdated: thread.is_outdated,
    }
}

fn overview_item_for_pull_request_comment(
    summary: &github::PullRequestSummary,
    comment: &github::PullRequestComment,
) -> OverviewReviewCommentItem {
    OverviewReviewCommentItem {
        summary: summary.clone(),
        author_login: comment.author_login.clone(),
        author_avatar_url: comment.author_avatar_url.clone(),
        location: "Conversation".to_string(),
        preview: summarize_overview_comment(&comment.body),
        timestamp: comment.updated_at.clone(),
        unread: false,
        is_resolved: false,
        is_outdated: false,
    }
}

fn overview_comment_location(
    thread: &github::PullRequestReviewThread,
    comment: &github::PullRequestReviewComment,
) -> String {
    let path = if comment.path.trim().is_empty() {
        thread.path.as_str()
    } else {
        comment.path.as_str()
    };
    let line = comment
        .line
        .or(comment.original_line)
        .or(thread.line)
        .or(thread.original_line)
        .and_then(|line| usize::try_from(line).ok());

    location_label(path, line)
}

fn summarize_overview_comment(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "No comment body.".to_string();
    }

    let limit = 180usize;
    let mut preview = collapsed.chars().take(limit).collect::<String>();
    if collapsed.chars().count() > limit {
        preview.push_str("...");
    }
    preview
}

fn render_pull_list(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let is_reviews = s.active_section == SectionId::Reviews;
    let workspace_loading = s.workspace_loading;
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let is_auth = s.is_authenticated();

    let available_queues: Vec<_> = if is_reviews {
        s.workspace
            .as_ref()
            .map(|w| {
                w.queues
                    .iter()
                    .filter(|q| q.id == "reviewRequested")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    } else {
        s.workspace
            .as_ref()
            .map(|w| w.queues.clone())
            .unwrap_or_default()
    };

    let current_queue = if is_reviews {
        available_queues.first().cloned()
    } else {
        available_queues
            .iter()
            .find(|q| q.id == s.active_queue_id)
            .or(available_queues.first())
            .cloned()
    };

    let queue_items: Vec<_> = current_queue
        .as_ref()
        .map(|q| q.items.clone())
        .unwrap_or_default();
    let queue_label = current_queue
        .as_ref()
        .map(|q| q.label.clone())
        .unwrap_or_else(|| "Pull Requests".to_string());
    let queue_truncation_message = current_queue.as_ref().and_then(|queue| {
        if queue.is_complete {
            None
        } else {
            Some(queue.truncated_reason.clone().unwrap_or_else(|| {
                format!(
                    "Loaded {} of {} pull requests.",
                    queue.items.len(),
                    queue.total_count
                )
            }))
        }
    });
    let loaded_from_cache = s
        .workspace
        .as_ref()
        .map(|w| w.loaded_from_cache)
        .unwrap_or(false);
    let shader_picker = s.project_shader_picker.clone();
    let shader_settings_error = s.project_shader_settings_error.clone();
    let project_shader_settings = s.project_shader_settings.clone();

    let sync_state = state.clone();
    let state_for_lanes = state.clone();

    // Viewer login for mine/others split
    let viewer_login = s
        .workspace
        .as_ref()
        .and_then(|w| w.viewer.as_ref())
        .map(|v| v.login.clone())
        .unwrap_or_default();
    let muted_repos = s.muted_repos.clone();
    let is_authored_queue = current_queue
        .as_ref()
        .map(|q| q.id == "authored")
        .unwrap_or(false);

    // Group items into kanban lanes by repository
    let mut my_items: Vec<github::PullRequestSummary> = Vec::new();
    let mut repo_groups: BTreeMap<String, Vec<github::PullRequestSummary>> = BTreeMap::new();
    for item in &queue_items {
        if muted_repos.contains(&item.repository) {
            continue;
        }
        if !is_authored_queue && !viewer_login.is_empty() && item.author_login == viewer_login {
            my_items.push(item.clone());
        } else {
            repo_groups
                .entry(item.repository.clone())
                .or_default()
                .push(item.clone());
        }
    }

    let has_my_items = !my_items.is_empty();
    let has_any_lanes = has_my_items || !repo_groups.is_empty();
    let muted_list: Vec<String> = muted_repos.iter().cloned().collect::<Vec<_>>();
    let has_muted = !muted_list.is_empty();

    div()
        .relative()
        .flex()
        .min_h_0()
        .flex_grow()
        // Sidebar
        .child(
            div()
                .w(sidebar_width())
                .bg(bg_overlay())
                .border_r(px(1.0))
                .border_color(border_muted())
                .p(px(24.0))
                .px(px(28.0))
                .flex()
                .flex_col()
                .flex_shrink_0()
                .min_h_0()
                .id("pull-sidebar-scroll")
                .overflow_y_scroll()
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(if is_reviews {
                            "Reviews"
                        } else {
                            "Pull Requests"
                        }),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(fg_muted())
                        .mt(px(6.0))
                        .max_w(px(200.0))
                        .child(if is_reviews {
                            "Review requests grouped by repository."
                        } else {
                            "Pull requests grouped into repo lanes."
                        }),
                )
                .child(div().flex().flex_col().gap(px(6.0)).mt(px(22.0)).children(
                    available_queues.iter().map(|queue| {
                        let is_active = current_queue
                            .as_ref()
                            .map(|c| c.id == queue.id)
                            .unwrap_or(false);
                        let queue_id = queue.id.clone();
                        let state = state.clone();
                        filter_pill(
                            &queue.label,
                            queue.total_count,
                            is_active,
                            move |_, _, cx| {
                                state.update(cx, |s, cx| {
                                    s.active_queue_id = queue_id.clone();
                                    cx.notify();
                                });
                            },
                        )
                    }),
                ))
                .when(has_muted, |el| {
                    el.child(
                        div()
                            .mt(px(24.0))
                            .flex()
                            .flex_col()
                            .child(eyebrow("Muted Repos"))
                            .child(div().flex().flex_col().gap(px(4.0)).children(
                                muted_list.into_iter().map(|repo| {
                                    let state = state.clone();
                                    let repo_for_unmute = repo.clone();
                                    muted_repo_pill(&repo, move |_, _, cx| {
                                        let r = repo_for_unmute.clone();
                                        state.update(cx, |s, cx| {
                                            s.muted_repos.remove(&r);
                                            cx.notify();
                                        });
                                    })
                                }),
                            )),
                    )
                }),
        )
        // Kanban board
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                // Board header
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(28.0))
                        .pt(px(24.0))
                        .pb(px(16.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(eyebrow(if loaded_from_cache {
                                    "Cached data"
                                } else {
                                    "Live data"
                                }))
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(if is_reviews {
                                            "Review Board".to_string()
                                        } else {
                                            queue_label
                                        }),
                                ),
                        )
                        .child(div().flex().items_center().gap(px(8.0)).child(ghost_button(
                            if workspace_syncing {
                                "Syncing..."
                            } else {
                                "Refresh"
                            },
                            {
                                let state = sync_state.clone();
                                move |_, window, cx| trigger_sync_workspace(&state, window, cx)
                            },
                        ))),
                )
                .when(workspace_loading, |el| {
                    el.child(
                        div()
                            .px(px(28.0))
                            .child(panel_state_text("Loading queue...")),
                    )
                })
                .when_some(workspace_error, |el, err| {
                    el.child(div().px(px(28.0)).child(error_text(&err)))
                })
                .when_some(queue_truncation_message, |el, message| {
                    el.child(div().px(px(28.0)).pb(px(12.0)).child(error_text(&message)))
                })
                .when(!workspace_loading && !has_any_lanes, |el| {
                    el.child(div().px(px(28.0)).child(panel_state_text(if has_muted {
                        "All repositories in this queue are muted."
                    } else if is_auth {
                        "No pull requests matched this queue."
                    } else {
                        "Authenticate with gh to load live pull request queues."
                    })))
                })
                // Swim lanes
                .child(
                    div()
                        .flex_grow()
                        .min_h_0()
                        .id("kanban-board-hscroll")
                        .overflow_x_scroll()
                        .overflow_y_hidden()
                        .px(px(24.0))
                        .pb(px(24.0))
                        .child(
                            div()
                                .flex()
                                .gap(px(16.0))
                                .h_full()
                                .when(has_my_items, |el| {
                                    let state = state_for_lanes.clone();
                                    let shader_variant =
                                        project_shader_settings.shader_for_project("__mine__");
                                    el.child(kanban_lane(
                                        "__mine__",
                                        "My Pull Requests",
                                        &format!("{} open", my_items.len()),
                                        my_items,
                                        accent(),
                                        true,
                                        shader_variant,
                                        state,
                                    ))
                                })
                                .children(repo_groups.into_iter().map(|(repo, items)| {
                                    let short_name =
                                        repo.split('/').last().unwrap_or(&repo).to_string();
                                    let count = items.len();
                                    let subtitle = repo_lane_subtitle(&repo, count);
                                    let accent_color = lane_accent_color(&repo);
                                    let state = state_for_lanes.clone();
                                    let shader_variant =
                                        project_shader_settings.shader_for_project(&repo);
                                    kanban_lane(
                                        &repo,
                                        &short_name,
                                        &subtitle,
                                        items,
                                        accent_color,
                                        false,
                                        shader_variant,
                                        state,
                                    )
                                })),
                        ),
                ),
        )
        .when_some(shader_picker, |el, picker| {
            el.child(render_project_shader_picker(
                state,
                picker,
                shader_settings_error,
                cx,
            ))
        })
}

fn render_project_shader_picker(
    state: &Entity<AppState>,
    picker: ProjectShaderPickerState,
    settings_error: Option<String>,
    cx: &App,
) -> impl IntoElement {
    let selected = state.read(cx).shader_for_project(&picker.project);
    let close_state = state.clone();
    let project_display = if picker.project == "__mine__" {
        picker.label.clone()
    } else {
        picker.project.clone()
    };

    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(82.0))
        .pb(px(28.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    close_state.update(cx, |s, cx| {
                        s.close_project_shader_picker();
                        cx.notify();
                    });
                }),
        )
        .child(
            div()
                .relative()
                .w(px(460.0))
                .rounded(radius_lg())
                .border_1()
                .border_color(border_default())
                .bg(bg_overlay())
                .shadow_sm()
                .occlude()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(20.0))
                        .py(px(15.0))
                        .border_b(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child("Project shader"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(project_display),
                                ),
                        )
                        .child(
                            div()
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded(radius_sm())
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(hover_bg()))
                                .on_mouse_down(MouseButton::Left, {
                                    let state = state.clone();
                                    move |_, _, cx| {
                                        cx.stop_propagation();
                                        state.update(cx, |s, cx| {
                                            s.close_project_shader_picker();
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(lucide_icon(LucideIcon::X, 17.0, fg_muted())),
                        ),
                )
                .child(
                    div()
                        .p(px(14.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children(OverviewShaderVariant::ALL.into_iter().map(|variant| {
                            project_shader_choice_row(&picker.project, variant, selected, state)
                        }))
                        .when_some(settings_error, |el, error| {
                            el.child(div().pt(px(4.0)).child(error_text(&error)))
                        }),
                ),
        )
}

fn project_shader_choice_row(
    project: &str,
    variant: OverviewShaderVariant,
    selected: OverviewShaderVariant,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let is_selected = variant == selected;
    let project = project.to_string();
    let label = variant.label();
    let sample_seed = format!("shader-choice-{project}-{label}");
    let state = state.clone();

    div()
        .w_full()
        .rounded(radius())
        .border_1()
        .border_color(if is_selected {
            focus_border()
        } else {
            border_muted()
        })
        .bg(if is_selected {
            control_selected_bg()
        } else {
            bg_surface()
        })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).border_color(border_default()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            state.update(cx, |s, cx| {
                s.set_project_shader(&project, variant);
                cx.notify();
            });
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(8.0))
                .child(
                    shader_material_surface_variant(
                        &sample_seed,
                        variant,
                        ShaderCornerMask::ALL,
                        bg_surface(),
                        radius_sm(),
                    )
                    .w(px(76.0))
                    .h(px(40.0))
                    .flex_shrink_0(),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(label),
                        ),
                )
                .when(is_selected, |el| {
                    el.child(lucide_icon(LucideIcon::Check, 16.0, focus()))
                }),
        )
}

fn render_issues(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
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

// --- Shared components ---

pub fn material_surface(seed: &str) -> Div {
    shader_material_surface(
        seed,
        0,
        ShaderCornerMask::default(),
        transparent(),
        radius(),
    )
}

fn shader_material_surface(
    seed: &str,
    variant_offset: usize,
    corners: ShaderCornerMask,
    mask_color: Rgba,
    corner_radius: Pixels,
) -> Div {
    let seed = seed.to_string();
    let variant = material_shader_variant(&seed, variant_offset);
    shader_material_surface_variant(&seed, variant, corners, mask_color, corner_radius)
}

fn shader_material_surface_variant(
    seed: &str,
    variant: OverviewShaderVariant,
    corners: ShaderCornerMask,
    mask_color: Rgba,
    corner_radius: Pixels,
) -> Div {
    let shader_seed = format!("review-material-{seed}");
    opengl_shader_surface_variant_with_corner_mask(
        shader_seed,
        variant,
        corner_radius,
        mask_color,
        corners,
    )
}

fn material_shader_variant(seed: &str, offset: usize) -> OverviewShaderVariant {
    let variants = OverviewShaderVariant::ALL;
    variants[(material_seed_index(seed) + offset) % variants.len()]
}

fn material_seed_index(seed: &str) -> usize {
    let hash = seed.bytes().fold(2166136261u32, |acc, byte| {
        acc.wrapping_mul(16777619) ^ byte as u32
    });
    (hash as usize) % OverviewShaderVariant::ALL.len()
}

pub fn panel() -> Div {
    div().rounded(radius()).bg(bg_overlay()).overflow_hidden()
}

pub fn nested_panel() -> Div {
    div().p(px(20.0)).rounded(radius()).bg(bg_overlay())
}

pub fn eyebrow(text: &str) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg_subtle())
        .mb(px(8.0))
        .child(text.to_string().to_uppercase())
}

pub fn ghost_button(
    label: &str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(7.0))
        .rounded(radius_sm())
        .bg(control_button_bg())
        .border_1()
        .border_color(border_muted())
        .text_color(fg_default())
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(control_button_hover_bg())
                .border_color(border_default())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
}

pub fn review_button(
    label: &str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(primary_action_bg())
        .text_color(fg_on_primary_action())
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.bg(primary_action_hover()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
}

pub fn badge(text: &str) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg_muted())
        .child(text.to_string())
}

pub fn user_avatar(
    login: &str,
    avatar_url: Option<&str>,
    size: f32,
    emphasized: bool,
) -> AnyElement {
    let login = login.to_string();
    let avatar_url = avatar_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);

    match avatar_url {
        Some(url) => {
            let url = avatar_image_url(&url, size);
            let inner_size = avatar_inner_size(size);
            let loading_login = login.clone();
            let fallback_login = login.clone();
            div()
                .w(px(size))
                .h(px(size))
                .rounded(px(size / 2.0))
                .overflow_hidden()
                .border_1()
                .border_color(if emphasized { accent() } else { border_muted() })
                .bg(if emphasized {
                    accent_muted()
                } else {
                    bg_emphasis()
                })
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    img(url)
                        .size(px(inner_size))
                        .rounded(px(inner_size / 2.0))
                        .overflow_hidden()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(move || {
                            avatar_placeholder(&loading_login, inner_size, emphasized)
                                .into_any_element()
                        })
                        .with_fallback(move || {
                            avatar_placeholder(&fallback_login, inner_size, emphasized)
                                .into_any_element()
                        }),
                )
                .into_any_element()
        }
        None => avatar_placeholder(&login, size, emphasized).into_any_element(),
    }
}

fn avatar_inner_size(size: f32) -> f32 {
    (size - 2.0).max(1.0)
}

fn avatar_image_url(url: &str, display_size: f32) -> String {
    if !url.contains("avatars.githubusercontent.com") {
        return url.to_string();
    }

    let image_size = ((display_size * 3.0).ceil() as usize).clamp(96, 256);
    let (url_without_fragment, fragment) = url
        .split_once('#')
        .map(|(url, fragment)| (url, Some(fragment)))
        .unwrap_or((url, None));
    let (base, query) = url_without_fragment
        .split_once('?')
        .unwrap_or((url_without_fragment, ""));
    let mut params = query
        .split('&')
        .filter(|param| !param.is_empty() && !param.starts_with("s="))
        .map(str::to_string)
        .collect::<Vec<_>>();
    params.push(format!("s={image_size}"));

    let mut output = if params.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", params.join("&"))
    };
    if let Some(fragment) = fragment {
        output.push('#');
        output.push_str(fragment);
    }
    output
}

fn avatar_placeholder(login: &str, size: f32, emphasized: bool) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .border_1()
        .border_color(if emphasized { accent() } else { border_muted() })
        .bg(if emphasized {
            accent_muted()
        } else {
            bg_emphasis()
        })
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .text_size(px((size * 0.38).max(9.0)))
        .font_family(mono_font_family())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if emphasized { accent() } else { fg_emphasis() })
        .child(login_monogram(login))
}

fn login_monogram(login: &str) -> String {
    let mut monogram = login
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if monogram.is_empty() {
        monogram.push('?');
    }
    monogram
}

pub fn badge_success(text: &str) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(success_muted())
        .border_1()
        .border_color(diff_add_border())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(success())
        .child(text.to_string())
}

pub fn panel_state_text(text: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(fg_muted())
        .child(text.to_string())
}

pub fn error_text(text: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(danger())
        .child(text.to_string())
}

pub fn success_text(text: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(success())
        .child(text.to_string())
}

pub fn meta_row(label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .w(px(88.0))
                .flex_shrink_0()
                .text_color(fg_subtle())
                .font_family(mono_font_family())
                .text_size(px(10.0))
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .px(px(10.0))
                .py(px(8.0))
                .rounded(radius_sm())
                .bg(bg_inset())
                .border_1()
                .border_color(border_muted())
                .text_color(fg_emphasis())
                .font_weight(FontWeight::MEDIUM)
                .font_family(mono_font_family())
                .text_size(px(11.0))
                .whitespace_normal()
                .child(value.to_string()),
        )
}

fn overview_welcome_greeting(viewer_name: &str, is_authenticated: bool) -> String {
    let viewer_name = viewer_name.trim();
    let viewer_name = if viewer_name.is_empty() {
        "there"
    } else {
        viewer_name
    };

    if is_authenticated {
        format!("Welcome back, {viewer_name}")
    } else {
        "Connect GitHub".to_string()
    }
}

fn filter_pill(
    label: &str,
    count: i64,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!("filter-pill-{label}-{}", usize::from(active)));

    div()
        .flex()
        .justify_between()
        .items_center()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(active, |el| el.bg(bg_emphasis()).text_color(fg_emphasis()))
        .when(!active, |el| el.text_color(fg_muted()))
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
        .child(
            div()
                .text_color(if active { fg_default() } else { fg_subtle() })
                .font_family(mono_font_family())
                .text_size(px(12.0))
                .child(count.to_string()),
        )
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

fn pill_badge(label: &str, fg: Rgba, bg: Rgba, border: Rgba) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.to_string())
}

fn subtle_pill(label: &str) -> impl IntoElement {
    pill_badge(label, fg_muted(), bg_emphasis(), border_muted())
}

fn pull_request_state_badge(item: &github::PullRequestSummary) -> AnyElement {
    if item.is_draft {
        return pill_badge("Draft", fg_muted(), bg_emphasis(), border_muted()).into_any_element();
    }

    match item.state.as_str() {
        "MERGED" => pill_badge("Merged", info(), info_muted(), info()).into_any_element(),
        "CLOSED" => {
            pill_badge("Closed", danger(), danger_muted(), diff_remove_border()).into_any_element()
        }
        _ => pill_badge("Open", success(), success_muted(), diff_add_border()).into_any_element(),
    }
}

fn review_decision_badge(decision: &str) -> AnyElement {
    match decision {
        "APPROVED" => {
            pill_badge("Approved", success(), success_muted(), diff_add_border()).into_any_element()
        }
        "CHANGES_REQUESTED" => {
            pill_badge("Changes", danger(), danger_muted(), diff_remove_border()).into_any_element()
        }
        "REVIEW_REQUIRED" => {
            pill_badge("Needs review", fg_muted(), bg_emphasis(), border_muted()).into_any_element()
        }
        "COMMENTED" => {
            pill_badge("Commented", accent(), accent_muted(), accent()).into_any_element()
        }
        _ => subtle_pill(decision).into_any_element(),
    }
}

fn render_diff_summary(additions: i64, deletions: i64) -> impl IntoElement {
    let additions = additions.max(0);
    let deletions = deletions.max(0);
    let total = additions + deletions;
    let segments = 8usize;
    let add_segments = if total > 0 {
        ((additions as f32 / total as f32) * segments as f32)
            .round()
            .clamp(0.0, segments as f32) as usize
    } else {
        0
    };
    let delete_segments = if total > 0 {
        segments.saturating_sub(add_segments)
    } else {
        0
    };

    div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .gap(px(4.0))
                .text_size(px(11.0))
                .font_family(mono_font_family())
                .child(div().text_color(success()).child(format!("+{additions}")))
                .child(div().text_color(danger()).child(format!("-{deletions}"))),
        )
        .child(
            div()
                .flex()
                .gap(px(2.0))
                .children((0..segments).map(move |ix| {
                    let bg = if ix < add_segments {
                        success()
                    } else if ix < add_segments + delete_segments {
                        danger()
                    } else {
                        border_muted()
                    };

                    div().w(px(8.0)).h(px(4.0)).rounded(px(2.0)).bg(bg)
                })),
        )
}

fn lane_header_scrim() -> Rgba {
    match active_theme() {
        ActiveTheme::Light => with_alpha(white().into(), 0.03),
        ActiveTheme::Dark => with_alpha(bg_canvas(), 0.18),
    }
}

fn lane_header_control_bg() -> Rgba {
    match active_theme() {
        ActiveTheme::Light => with_alpha(white().into(), 0.62),
        ActiveTheme::Dark => with_alpha(bg_canvas(), 0.56),
    }
}

fn lane_header_control_hover_bg() -> Rgba {
    match active_theme() {
        ActiveTheme::Light => with_alpha(white().into(), 0.78),
        ActiveTheme::Dark => with_alpha(bg_overlay(), 0.92),
    }
}

fn lane_header_control_border() -> Rgba {
    match active_theme() {
        ActiveTheme::Light => with_alpha(white().into(), 0.42),
        ActiveTheme::Dark => with_alpha(white().into(), 0.18),
    }
}

fn kanban_lane(
    lane_id: &str,
    label: &str,
    subtitle: &str,
    items: Vec<github::PullRequestSummary>,
    _accent: Rgba,
    is_mine: bool,
    shader_variant: OverviewShaderVariant,
    state: Entity<AppState>,
) -> impl IntoElement {
    let label = label.to_string();
    let subtitle = subtitle.to_string();
    let count = items.len();
    let mute_state = state.clone();
    let mute_repo = lane_id.to_string();
    let picker_project = lane_id.to_string();
    let picker_label = label.clone();
    let picker_state = state.clone();
    let show_repo_in_card_meta = is_mine;
    let lane_radius = radius_lg();
    let shader_visible_height = px(70.0);
    let shader_backplate_height = shader_visible_height + lane_radius;

    div()
        .w(px(KANBAN_LANE_WIDTH))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .min_h_0()
        .child(
            div()
                .flex()
                .flex_col()
                .min_h_0()
                .flex_grow()
                .rounded(lane_radius)
                .bg(transparent())
                .shadow_md()
                .child(
                    shader_material_surface_variant(
                        lane_id,
                        shader_variant,
                        ShaderCornerMask::TOP,
                        bg_canvas(),
                        lane_radius,
                    )
                    .h(shader_backplate_height)
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(16.0))
                    .pt(px(16.0))
                    .pb(px(16.0) + lane_radius)
                    .text_color(fg_emphasis())
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        picker_state.update(cx, |s, cx| {
                            s.open_project_shader_picker(&picker_project, &picker_label);
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .size_full()
                            .bg(lane_header_scrim()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(radius_sm())
                                    .bg(lane_header_control_bg())
                                    .border_1()
                                    .border_color(lane_header_control_border())
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child(label),
                            )
                            .child(
                                div()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(999.0))
                                    .bg(lane_header_control_bg())
                                    .border_1()
                                    .border_color(lane_header_control_border())
                                    .text_size(px(11.0))
                                    .font_family(mono_font_family())
                                    .text_color(fg_emphasis())
                                    .child(count.to_string()),
                            ),
                    )
                    .when(!is_mine, |el| {
                        el.child(
                            div()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(radius_sm())
                                .text_size(px(11.0))
                                .text_color(fg_emphasis())
                                .cursor_pointer()
                                .bg(lane_header_control_bg())
                                .border_1()
                                .border_color(lane_header_control_border())
                                .hover(|s| {
                                    s.bg(lane_header_control_hover_bg()).text_color(danger())
                                })
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.stop_propagation();
                                    mute_state.update(cx, |s, cx| {
                                        s.muted_repos.insert(mute_repo.clone());
                                        cx.notify();
                                    });
                                })
                                .child("Mute"),
                        )
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_grow()
                        .min_h_0()
                        .mt(px(-f32::from(lane_radius)))
                        .bg(bg_overlay())
                        .rounded(lane_radius)
                        .overflow_hidden()
                        .child(
                            div()
                                .px(px(14.0))
                                .py(px(10.0))
                                .text_size(px(11.0))
                                .text_color(fg_subtle())
                                .font_family(mono_font_family())
                                .child(subtitle),
                        )
                        .child(
                            div()
                                .flex_grow()
                                .min_h_0()
                                .id(SharedString::from(format!("lane-scroll-{lane_id}")))
                                .overflow_y_scroll()
                                .scrollbar_width(px(KANBAN_LANE_SCROLLBAR_WIDTH))
                                .px(px(10.0))
                                .pb(px(10.0))
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .children(items.into_iter().map(|item| {
                                            let state = state.clone();
                                            kanban_card(
                                                item,
                                                show_repo_in_card_meta,
                                                move |summary, window, cx| {
                                                    open_pull_request(&state, summary, window, cx);
                                                },
                                            )
                                        })),
                                ),
                        ),
                ),
        )
}

fn kanban_card(
    item: github::PullRequestSummary,
    show_repo_in_meta: bool,
    on_click: impl Fn(github::PullRequestSummary, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = item.title.clone();
    let repo_label = item
        .repository
        .split('/')
        .last()
        .unwrap_or(&item.repository)
        .to_string();
    let author_login = item.author_login.clone();
    let author_avatar_url = item.author_avatar_url.clone();
    let meta = if show_repo_in_meta {
        format!(
            "{} #{} \u{00b7} {}",
            repo_label,
            item.number,
            format_relative_time(&item.updated_at)
        )
    } else {
        format!(
            "#{} \u{00b7} {}",
            item.number,
            format_relative_time(&item.updated_at)
        )
    };
    let additions = item.additions;
    let deletions = item.deletions;
    let comments = item.comments_count;
    let changed_files = item.changed_files;
    let review_decision = item.review_decision.clone();
    let summary = item.clone();

    div()
        .w_full()
        .min_w_0()
        .rounded(radius())
        .bg(bg_overlay())
        .p(px(14.0))
        .cursor_pointer()
        .hover(|s| s.bg(bg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(summary.clone(), window, cx)
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_grow()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .line_clamp(2)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .min_w_0()
                                        .text_size(px(10.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_muted())
                                        .child(user_avatar(
                                            &author_login,
                                            author_avatar_url.as_deref(),
                                            16.0,
                                            false,
                                        ))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(fg_emphasis())
                                                .child(author_login),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .overflow_x_hidden()
                                                .child(format!("\u{00b7} {meta}")),
                                        ),
                                ),
                        )
                        .child(render_diff_summary(additions, deletions)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(pull_request_state_badge(&item))
                        .when_some(review_decision, |el, decision| {
                            el.child(review_decision_badge(&decision))
                        })
                        .when(comments > 0, |el| {
                            el.child(subtle_pill(&format!("{comments} comments")))
                        })
                        .child(subtle_pill(&format!("{changed_files} files"))),
                ),
        )
}

fn repo_lane_subtitle(repo: &str, count: usize) -> String {
    let count_label = format!("{count} open");
    match repo.split_once('/') {
        Some((owner, _)) if !owner.trim().is_empty() => {
            format!("{owner} \u{00b7} {count_label}")
        }
        _ => count_label,
    }
}

fn muted_repo_pill(
    repo: &str,
    on_unmute: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let short_name = repo.split('/').last().unwrap_or(repo).to_string();

    div()
        .flex()
        .justify_between()
        .items_center()
        .px(px(14.0))
        .py(px(6.0))
        .rounded(radius_sm())
        .text_size(px(12.0))
        .text_color(fg_subtle())
        .child(
            div()
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_x_hidden()
                .child(short_name),
        )
        .child(
            div()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(radius_sm())
                .text_size(px(11.0))
                .text_color(fg_subtle())
                .cursor_pointer()
                .hover(|s| s.bg(hover_bg()).text_color(success()))
                .on_mouse_down(MouseButton::Left, on_unmute)
                .child("Unmute"),
        )
}

fn activate_queue(state: &Entity<AppState>, section: SectionId, queue_id: &str, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.set_active_section(section);
        s.active_surface = PullRequestSurface::Overview;
        s.active_queue_id = queue_id.to_string();
        s.active_pr_key = None;
        s.palette_open = false;
        s.palette_selected_index = 0;
        s.pr_header_compact = false;
        cx.notify();
    });
}

pub fn open_pull_request(
    state: &Entity<AppState>,
    summary: github::PullRequestSummary,
    window: &mut Window,
    cx: &mut App,
) {
    let key = pr_key(&summary.repository, summary.number);
    let repository = summary.repository.clone();
    let number = summary.number;
    let opens_new_tab = {
        let s = state.read(cx);
        !s.open_tabs
            .iter()
            .any(|t| pr_key(&t.repository, t.number) == key)
    };
    let initial_surface = if opens_new_tab && summary.local_key.is_none() {
        PullRequestSurface::Overview
    } else {
        PullRequestSurface::Files
    };
    let cached_review_session = {
        let cache = state.read(cx).cache.clone();
        load_review_session(cache.as_ref(), &key).ok().flatten()
    };
    let load_plan = {
        let s = state.read(cx);
        plan_pull_request_open(&s, &key)
    };

    state.update(cx, |s, cx| {
        if !s
            .open_tabs
            .iter()
            .any(|t| pr_key(&t.repository, t.number) == key)
        {
            s.open_tabs.insert(0, summary);
        }
        s.set_active_section(SectionId::Pulls);
        s.active_surface = initial_surface;
        s.active_pr_key = Some(key.clone());
        s.palette_open = false;
        s.palette_selected_index = 0;
        s.review_body.clear();
        s.review_editor_active = false;
        s.review_message = None;
        s.review_success = false;
        s.pr_header_compact = false;

        s.detail_states.entry(key.clone()).or_default();
        s.apply_review_session_document(&key, cached_review_session.clone());
        s.ensure_active_selected_file_is_valid();
        let detail_state = s.detail_states.entry(key.clone()).or_default();
        detail_state.loading = load_plan.show_loading;
        if load_plan.load_cached_snapshot || load_plan.sync_live {
            detail_state.error = None;
        }
        cx.notify();
    });

    ensure_structural_diff_warmup_started(state, window, cx);
    if initial_surface == PullRequestSurface::Overview {
        crate::review_intelligence::refresh_active_review_brief(state, window, cx, true);
        crate::review_intelligence::refresh_active_review_partner(state, window, cx, true);
    }

    if !load_plan.load_cached_snapshot && !load_plan.sync_live {
        return;
    }

    // Load PR detail in background
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let detail_key = pr_key(&repository, number);
            let mut should_sync = load_plan.sync_live;

            if load_plan.load_cached_snapshot {
                let cached_result = cx
                    .background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = repository.clone();
                        async move { github::load_pull_request_detail(&cache, &repository, number) }
                    })
                    .await;

                should_sync = match &cached_result {
                    Ok(snapshot) => detail_snapshot_needs_background_refresh(snapshot),
                    Err(_) => true,
                };

                model
                    .update(cx, |s, cx| {
                        let ds = s.detail_states.entry(detail_key.clone()).or_default();
                        match &cached_result {
                            Ok(snapshot) => {
                                ds.snapshot = Some(snapshot.clone());
                                ds.loading = snapshot.detail.is_none() && should_sync;
                                ds.error = None;
                            }
                            Err(error) => {
                                ds.loading = should_sync;
                                ds.error = Some(error.clone());
                            }
                        }
                        s.ensure_active_selected_file_is_valid();
                        cx.notify();
                    })
                    .ok();

                warm_structural_diffs_flow(model.clone(), cx).await;
                refresh_brief_if_active_overview(model.clone(), &detail_key, cx).await;
            }

            if !should_sync {
                return;
            }

            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = ds
                        .snapshot
                        .as_ref()
                        .and_then(|sn| sn.detail.as_ref())
                        .is_none();
                    ds.syncing = true;
                    ds.error = None;
                    cx.notify();
                })
                .ok();

            let sync_result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let repository = repository.clone();
                    async move {
                        notifications::sync_pull_request_detail_with_read_state(
                            &cache,
                            &repository,
                            number,
                        )
                    }
                })
                .await;

            model
                .update(cx, |s, cx| {
                    let mut next_unread_ids = None;
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = false;
                    ds.syncing = false;
                    match sync_result {
                        Ok((snapshot, unread_ids)) => {
                            ds.snapshot = Some(snapshot);
                            ds.error = None;
                            next_unread_ids = Some(unread_ids);
                        }
                        Err(e) => {
                            ds.error = Some(e);
                        }
                    }
                    s.ensure_active_selected_file_is_valid();
                    if let Some(unread_ids) = next_unread_ids {
                        s.unread_review_comment_ids = unread_ids;
                    }
                    cx.notify();
                })
                .ok();

            warm_structural_diffs_flow(model.clone(), cx).await;
            refresh_brief_if_active_overview(model.clone(), &detail_key, cx).await;
        })
        .detach();
}

async fn refresh_brief_if_active_overview(
    model: Entity<AppState>,
    detail_key: &str,
    cx: &mut AsyncWindowContext,
) {
    let should_refresh_brief = model
        .read_with(cx, |state, _| {
            state.active_surface == PullRequestSurface::Overview
                && state.active_pr_key.as_deref() == Some(detail_key)
        })
        .ok()
        .unwrap_or(false);

    if should_refresh_brief {
        crate::review_intelligence::refresh_active_review_brief_flow(model.clone(), true, cx).await;
        crate::review_intelligence::refresh_active_review_partner_flow(model, true, cx).await;
    }
}

#[derive(Clone, Copy)]
struct PullRequestOpenPlan {
    load_cached_snapshot: bool,
    sync_live: bool,
    show_loading: bool,
}

fn plan_pull_request_open(state: &AppState, key: &str) -> PullRequestOpenPlan {
    let Some(detail_state) = state.detail_states.get(key) else {
        return PullRequestOpenPlan {
            load_cached_snapshot: true,
            sync_live: false,
            show_loading: true,
        };
    };

    let has_detail = detail_state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.detail.as_ref())
        .is_some();

    if detail_state.loading || detail_state.syncing {
        return PullRequestOpenPlan {
            load_cached_snapshot: false,
            sync_live: false,
            show_loading: !has_detail,
        };
    }

    if !has_detail {
        return PullRequestOpenPlan {
            load_cached_snapshot: true,
            sync_live: false,
            show_loading: true,
        };
    }

    PullRequestOpenPlan {
        load_cached_snapshot: false,
        sync_live: detail_state
            .snapshot
            .as_ref()
            .map(detail_snapshot_needs_background_refresh)
            .unwrap_or(true),
        show_loading: false,
    }
}

fn detail_snapshot_needs_background_refresh(snapshot: &github::PullRequestDetailSnapshot) -> bool {
    if snapshot.detail.is_none() {
        return true;
    }

    let Some(fetched_at_ms) = snapshot.fetched_at_ms else {
        return true;
    };

    current_time_ms().saturating_sub(fetched_at_ms) > DETAIL_AUTO_REFRESH_TTL_MS
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn format_relative_time(value: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(ts) = parse_iso_timestamp(value) {
        let diff = now.saturating_sub(ts);
        let minutes = diff / 60;
        let hours = diff / 3600;
        let days = diff / 86400;

        if minutes < 1 {
            return "just now".to_string();
        }
        if minutes < 60 {
            return format!("{minutes}m ago");
        }
        if hours < 24 {
            return format!("{hours}h ago");
        }
        if days < 30 {
            return format!("{days}d ago");
        }
    }

    if value.len() > 10 {
        value[..10].to_string()
    } else {
        value.to_string()
    }
}

fn parse_iso_timestamp(value: &str) -> Option<u64> {
    let parts: Vec<&str> = value.split('T').collect();
    if parts.len() < 2 {
        return None;
    }
    let date_parts: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let time_str = parts[1].trim_end_matches('Z');
    let time_parts: Vec<u64> = time_str.split(':').filter_map(|p| p.parse().ok()).collect();
    if time_parts.len() < 2 {
        return None;
    }

    let year = date_parts[0];
    let month = date_parts[1];
    let day = date_parts[2];
    let hour = time_parts[0];
    let minute = time_parts[1];
    let second = if time_parts.len() > 2 {
        time_parts[2]
    } else {
        0
    };

    let mut days_total: u64 = 0;
    for y in 1970..year {
        days_total += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    for m in 1..month {
        days_total += month_days[m as usize];
        if m == 2 && is_leap {
            days_total += 1;
        }
    }
    days_total += day - 1;

    Some(days_total * 86400 + hour * 3600 + minute * 60 + second)
}
