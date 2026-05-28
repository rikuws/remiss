use std::collections::BTreeMap;

use gpui::prelude::*;
use gpui::*;

use crate::github::{self, IssueDetail, IssueSummary, PullRequestComment};
use crate::icons::{lucide_icon, LucideIcon};
use crate::state::{issue_key, AppState};
use crate::theme::*;

use super::super::tooltips::build_static_tooltip;
use super::super::workspace_sync::trigger_sync_workspace;
use super::{
    error_text, eyebrow, filter_pill, format_relative_time, ghost_button, panel, panel_state_text,
    restrict_scroll_to_axis, subtle_pill, user_avatar,
};

const ISSUE_GROUP_MAX_WIDTH: f32 = 1040.0;
const ISSUE_SCROLLBAR_WIDTH: f32 = 8.0;

pub(super) fn render_issues(state: &Entity<AppState>, cx: &App) -> AnyElement {
    let s = state.read(cx);
    if let Some(active_issue_key) = s.active_issue_key.clone() {
        return render_issue_detail(state, active_issue_key, cx).into_any_element();
    }

    let workspace_loading = s.workspace_loading;
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let is_auth = s.is_authenticated();
    let loaded_from_cache = s
        .workspace
        .as_ref()
        .map(|w| w.loaded_from_cache)
        .unwrap_or(false);
    let available_queues = s
        .workspace
        .as_ref()
        .map(|w| w.issue_queues.clone())
        .unwrap_or_default();
    let current_queue = available_queues
        .iter()
        .find(|queue| queue.id == s.active_issue_queue_id)
        .or(available_queues.first())
        .cloned();
    let queue_items = current_queue
        .as_ref()
        .map(|queue| queue.items.clone())
        .unwrap_or_default();
    let queue_label = current_queue
        .as_ref()
        .map(|queue| queue.label.clone())
        .unwrap_or_else(|| "Issues".to_string());
    let queue_truncation_message = current_queue.as_ref().and_then(|queue| {
        if queue.is_complete {
            None
        } else {
            Some(queue.truncated_reason.clone().unwrap_or_else(|| {
                format!(
                    "Loaded {} of {} issues.",
                    queue.items.len(),
                    queue.total_count
                )
            }))
        }
    });
    let mut repo_groups: BTreeMap<String, Vec<IssueSummary>> = BTreeMap::new();
    for issue in queue_items {
        repo_groups
            .entry(issue.repository.clone())
            .or_default()
            .push(issue);
    }
    let has_issues = !repo_groups.is_empty();
    let sync_state = state.clone();

    div()
        .relative()
        .flex()
        .min_w_0()
        .min_h_0()
        .flex_grow()
        .child(
            div()
                .w(sidebar_width())
                .p(px(24.0))
                .px(px(28.0))
                .flex()
                .flex_col()
                .flex_shrink_0()
                .min_h_0()
                .id("issues-sidebar-scroll")
                .overflow_y_scroll()
                .track_scroll(&s.issues_sidebar_scroll_handle)
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Issues"),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(fg_muted())
                        .mt(px(6.0))
                        .max_w(px(200.0))
                        .child("Open GitHub issues grouped by repository."),
                )
                .child(div().flex().flex_col().gap(px(6.0)).mt(px(22.0)).children(
                    available_queues.iter().map(|queue| {
                        let is_active = current_queue
                            .as_ref()
                            .map(|current| current.id == queue.id)
                            .unwrap_or(false);
                        let queue_id = queue.id.clone();
                        let state = state.clone();
                        filter_pill(
                            &queue.label,
                            queue.total_count,
                            is_active,
                            move |_, _, cx| {
                                state.update(cx, |state, cx| {
                                    state.active_issue_queue_id = queue_id.clone();
                                    state.active_issue_key = None;
                                    cx.notify();
                                });
                            },
                        )
                    }),
                )),
        )
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
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
                                        .child(queue_label),
                                ),
                        )
                        .child(div().flex().items_center().gap(px(8.0)).child(ghost_button(
                            if workspace_syncing {
                                "Syncing..."
                            } else {
                                "Refresh"
                            },
                            move |_, window, cx| trigger_sync_workspace(&sync_state, window, cx),
                        ))),
                )
                .when(workspace_loading, |el| {
                    el.child(
                        div()
                            .px(px(28.0))
                            .child(panel_state_text("Loading issues...")),
                    )
                })
                .when_some(workspace_error, |el, err| {
                    el.child(div().px(px(28.0)).child(error_text(&err)))
                })
                .when_some(queue_truncation_message, |el, message| {
                    el.child(div().px(px(28.0)).pb(px(12.0)).child(error_text(&message)))
                })
                .when(!workspace_loading && !has_issues, |el| {
                    el.child(div().px(px(28.0)).child(panel_state_text(if is_auth {
                        "No open issues matched this queue."
                    } else {
                        "Authenticate with gh to load live issue queues."
                    })))
                })
                .child(
                    restrict_scroll_to_axis(div())
                        .w_full()
                        .min_w_0()
                        .flex_grow()
                        .min_h_0()
                        .id("issues-list-scroll")
                        .overflow_y_scroll()
                        .scrollbar_width(px(ISSUE_SCROLLBAR_WIDTH))
                        .track_scroll(&s.issues_list_scroll_handle)
                        .px(px(28.0))
                        .pb(px(28.0))
                        .child(
                            div()
                                .w_full()
                                .max_w(px(ISSUE_GROUP_MAX_WIDTH))
                                .flex()
                                .flex_col()
                                .gap(px(14.0))
                                .children(
                                    repo_groups
                                        .into_iter()
                                        .map(|group| issue_repository_group(group, state.clone())),
                                ),
                        ),
                ),
        )
        .into_any_element()
}

fn open_issue(state: &Entity<AppState>, summary: IssueSummary, window: &mut Window, cx: &mut App) {
    let key = issue_key(&summary.repository, summary.number);
    let repository = summary.repository.clone();
    let number = summary.number;

    state.update(cx, |state, cx| {
        state.active_issue_key = Some(key.clone());
        state.active_pr_key = None;
        state.palette_open = false;
        state.palette_selected_index = 0;
        let detail_state = state.issue_detail_states.entry(key.clone()).or_default();
        detail_state.summary = Some(summary.clone());
        detail_state.loading = detail_state.snapshot.is_none();
        detail_state.syncing = true;
        detail_state.error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
            let Some(cache) = cache else { return };

            let cached = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let repository = repository.clone();
                    async move { github::load_issue_detail(&cache, &repository, number) }
                })
                .await;

            if let Ok(snapshot) = cached {
                if snapshot.detail.is_some() {
                    model
                        .update(cx, |state, cx| {
                            let detail_state =
                                state.issue_detail_states.entry(key.clone()).or_default();
                            detail_state.snapshot = Some(snapshot);
                            detail_state.loading = false;
                            detail_state.error = None;
                            cx.notify();
                        })
                        .ok();
                }
            }

            let synced = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let repository = repository.clone();
                    async move { github::sync_issue_detail(&cache, &repository, number) }
                })
                .await;

            model
                .update(cx, |state, cx| {
                    let detail_state = state.issue_detail_states.entry(key.clone()).or_default();
                    detail_state.loading = false;
                    detail_state.syncing = false;
                    match synced {
                        Ok(snapshot) => {
                            detail_state.snapshot = Some(snapshot);
                            detail_state.error = None;
                        }
                        Err(error) => {
                            detail_state.error = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

fn render_issue_detail(state: &Entity<AppState>, active_issue_key: String, cx: &App) -> AnyElement {
    let detail_state = state
        .read(cx)
        .issue_detail_states
        .get(&active_issue_key)
        .cloned()
        .unwrap_or_default();
    let detail = detail_state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.detail.as_ref())
        .cloned();
    let summary = detail_state.summary.clone();
    let title = detail
        .as_ref()
        .map(issue_detail_title)
        .or_else(|| summary.as_ref().map(issue_title))
        .unwrap_or_else(|| "Issue".to_string());
    let repository = detail
        .as_ref()
        .map(|detail| detail.repository.clone())
        .or_else(|| summary.as_ref().map(|summary| summary.repository.clone()))
        .unwrap_or_default();
    let number = detail
        .as_ref()
        .map(|detail| detail.number)
        .or_else(|| summary.as_ref().map(|summary| summary.number))
        .unwrap_or_default();
    let url = detail
        .as_ref()
        .map(|detail| detail.url.clone())
        .or_else(|| summary.as_ref().map(|summary| summary.url.clone()));
    let refreshed = detail_state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.fetched_at_ms)
        .map(|_| {
            if detail_state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.loaded_from_cache)
                .unwrap_or(false)
            {
                "Cached data"
            } else {
                "Live data"
            }
        })
        .unwrap_or("Issue detail");
    let state_for_back = state.clone();
    let state_for_refresh = state.clone();
    let refresh_summary = summary
        .clone()
        .or_else(|| detail.as_ref().map(summary_for_issue_detail));

    div()
        .flex_grow()
        .min_h_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .px(px(40.0))
                .pt(px(24.0))
                .pb(px(16.0))
                .child(
                    div()
                        .min_w_0()
                        .flex_col()
                        .gap(px(3.0))
                        .child(eyebrow(refreshed))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(issue_detail_back_button(move |_, _, cx| {
                                    state_for_back.update(cx, |state, cx| {
                                        if state.navigate_issue_back() {
                                            cx.notify();
                                        }
                                    });
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(17.0))
                                        .line_height(px(22.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .line_clamp(1)
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(title.clone()),
                                ),
                        )
                        .child(
                            div()
                                .pl(px(30.0))
                                .font_family(mono_font_family())
                                .text_size(px(11.0))
                                .text_color(fg_subtle())
                                .child(format!("{repository} #{number}")),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .when_some(refresh_summary, |el, summary| {
                            el.child(ghost_button(
                                if detail_state.syncing {
                                    "Refreshing..."
                                } else {
                                    "Refresh"
                                },
                                move |_, window, cx| {
                                    open_issue(&state_for_refresh, summary.clone(), window, cx);
                                },
                            ))
                        })
                        .when_some(url, |el, url| {
                            el.child(ghost_button("Open in GitHub", move |_, _, cx| {
                                cx.open_url(&url);
                            }))
                        }),
                ),
        )
        .when_some(detail_state.error.clone(), |el, error| {
            el.child(div().px(px(40.0)).pb(px(12.0)).child(error_text(&error)))
        })
        .when(detail_state.loading && detail.is_none(), |el| {
            el.child(
                div()
                    .px(px(40.0))
                    .child(panel_state_text("Loading issue detail...")),
            )
        })
        .child(
            restrict_scroll_to_axis(div())
                .id("issue-detail-scroll")
                .overflow_y_scroll()
                .scrollbar_width(px(ISSUE_SCROLLBAR_WIDTH))
                .flex_grow()
                .min_h_0()
                .px(px(40.0))
                .pb(px(32.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(ISSUE_GROUP_MAX_WIDTH))
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(issue_detail_summary_panel(
                            detail.as_ref(),
                            summary.as_ref(),
                        ))
                        .child(issue_timeline_panel(detail.as_ref())),
                ),
        )
        .into_any_element()
}

fn issue_detail_back_button(
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("issue-detail-back")
        .w(px(22.0))
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .tooltip(|_, cx| build_static_tooltip("Back to issues", cx))
        .hover(|style| style.text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(LucideIcon::ArrowLeft, 14.0, fg_muted()))
}

fn issue_detail_summary_panel(
    detail: Option<&IssueDetail>,
    summary: Option<&IssueSummary>,
) -> impl IntoElement {
    let author = detail
        .map(|detail| detail.author_login.as_str())
        .or_else(|| summary.map(|summary| summary.author_login.as_str()))
        .unwrap_or("unknown");
    let avatar = detail
        .and_then(|detail| detail.author_avatar_url.as_deref())
        .or_else(|| summary.and_then(|summary| summary.author_avatar_url.as_deref()));
    let state = detail
        .map(|detail| detail.state.as_str())
        .or_else(|| summary.map(|summary| summary.state.as_str()))
        .unwrap_or("OPEN");
    let created = detail
        .map(|detail| detail.created_at.as_str())
        .or_else(|| summary.map(|summary| summary.updated_at.as_str()))
        .unwrap_or_default();
    let updated = detail
        .map(|detail| detail.updated_at.as_str())
        .or_else(|| summary.map(|summary| summary.updated_at.as_str()))
        .unwrap_or_default();
    let labels = detail
        .map(|detail| detail.labels.clone())
        .or_else(|| summary.map(|summary| summary.labels.clone()))
        .unwrap_or_default();
    let assignees = detail
        .map(|detail| detail.assignees.clone())
        .or_else(|| summary.map(|summary| summary.assignees.clone()))
        .unwrap_or_default();
    let comments_count = detail
        .map(|detail| detail.comments_count)
        .or_else(|| summary.map(|summary| summary.comments_count))
        .unwrap_or(0);

    panel().child(
        div()
            .p(px(18.0))
            .flex()
            .items_start()
            .justify_between()
            .gap(px(18.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .child(user_avatar(author, avatar, 28.0, false))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child(author.to_string()),
                            )
                            .child(
                                div()
                                    .font_family(mono_font_family())
                                    .text_size(px(11.0))
                                    .text_color(fg_muted())
                                    .child(format!(
                                        "{} · opened {} · updated {}",
                                        issue_state_label(state),
                                        format_relative_time(created),
                                        format_relative_time(updated)
                                    )),
                            )
                            .child(issue_badge_row(labels, assignees, 0)),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.0))
                    .text_color(fg_subtle())
                    .child(format!("{comments_count} comments")),
            ),
    )
}

fn issue_timeline_panel(detail: Option<&IssueDetail>) -> impl IntoElement {
    panel().child(
        div()
            .p(px(18.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(eyebrow("Timeline"))
            .when_some(detail, |el, detail| {
                el.child(issue_timeline_item(
                    &detail.author_login,
                    detail.author_avatar_url.as_deref(),
                    "Description",
                    &detail.created_at,
                    issue_body_text(&detail.body),
                ))
                .when(detail.comments.is_empty(), |el| {
                    el.child(panel_state_text("No comments yet."))
                })
                .children(
                    detail
                        .comments
                        .iter()
                        .map(|comment| issue_comment_timeline_item(comment)),
                )
            })
            .when(detail.is_none(), |el| {
                el.child(panel_state_text("Timeline loads with issue detail."))
            }),
    )
}

fn issue_comment_timeline_item(comment: &PullRequestComment) -> impl IntoElement {
    issue_timeline_item(
        &comment.author_login,
        comment.author_avatar_url.as_deref(),
        "Comment",
        &comment.updated_at,
        issue_body_text(&comment.body),
    )
}

fn issue_timeline_item(
    author: &str,
    avatar_url: Option<&str>,
    kind: &str,
    timestamp: &str,
    body: String,
) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(user_avatar(author, avatar_url, 26.0, false))
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(12.0))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(author.to_string()),
                        )
                        .child(
                            div()
                                .font_family(mono_font_family())
                                .text_color(fg_subtle())
                                .child(format!("{kind} · {}", format_relative_time(timestamp))),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .line_height(px(20.0))
                        .text_color(fg_default())
                        .whitespace_normal()
                        .child(body),
                ),
        )
}

fn issue_repository_group(
    (repository, issues): (String, Vec<IssueSummary>),
    state: Entity<AppState>,
) -> impl IntoElement {
    let owner = repository
        .split_once('/')
        .map(|(owner, _)| owner.to_string())
        .unwrap_or_else(|| repository.clone());
    let short_name = repository
        .split('/')
        .last()
        .unwrap_or(&repository)
        .to_string();

    panel().child(
        div()
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .min_h(px(44.0))
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .rounded(radius_sm())
                    .bg(bg_surface())
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(lucide_icon(LucideIcon::Inbox, 15.0, fg_muted()))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(fg_emphasis())
                                            .line_clamp(1)
                                            .child(short_name),
                                    )
                                    .child(
                                        div()
                                            .font_family(mono_font_family())
                                            .text_size(px(10.0))
                                            .text_color(fg_subtle())
                                            .line_clamp(1)
                                            .child(owner),
                                    ),
                            ),
                    )
                    .child(div().w(px(1.0)).flex_shrink_0()),
            )
            .children(
                issues
                    .into_iter()
                    .map(|issue| issue_row(issue, state.clone())),
            ),
    )
}

fn issue_row(issue: IssueSummary, state: Entity<AppState>) -> impl IntoElement {
    let repo_ref = format!("{} #{}", issue.repository, issue.number);
    let title = issue_title(&issue);
    let author_login = issue.author_login.clone();
    let author_avatar_url = issue.author_avatar_url.clone();
    let updated = format_relative_time(&issue.updated_at);
    let comments = issue.comments_count;
    let labels = issue.labels.clone();
    let assignees = issue.assignees.clone();
    let issue_for_open = issue.clone();

    div()
        .w_full()
        .min_w_0()
        .px(px(14.0))
        .py(px(12.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .hover(|style| style.bg(bg_surface()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_issue(&state, issue_for_open.clone(), window, cx);
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(18.0))
                .child(
                    div()
                        .min_w_0()
                        .flex_grow()
                        .flex()
                        .items_start()
                        .gap(px(10.0))
                        .child(div().pt(px(3.0)).child(lucide_icon(
                            LucideIcon::Circle,
                            12.0,
                            success(),
                        )))
                        .child(
                            div()
                                .min_w_0()
                                .flex_grow()
                                .flex()
                                .flex_col()
                                .gap(px(7.0))
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .text_size(px(14.0))
                                        .line_height(px(20.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .line_clamp(1)
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(7.0))
                                        .min_w_0()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_muted())
                                        .child(repo_ref)
                                        .child(format!("updated {updated}")),
                                )
                                .child(issue_badge_row(labels, assignees, comments)),
                        ),
                )
                .child(
                    div()
                        .w(px(150.0))
                        .flex_shrink_0()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(8.0))
                        .child(user_avatar(
                            &author_login,
                            author_avatar_url.as_deref(),
                            18.0,
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
                ),
        )
}

fn issue_title(issue: &IssueSummary) -> String {
    let title = issue.title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        format!("Issue #{}", issue.number)
    } else {
        title
    }
}

fn issue_detail_title(issue: &IssueDetail) -> String {
    let title = issue.title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        format!("Issue #{}", issue.number)
    } else {
        title
    }
}

fn summary_for_issue_detail(detail: &IssueDetail) -> IssueSummary {
    IssueSummary {
        repository: detail.repository.clone(),
        number: detail.number,
        title: detail.title.clone(),
        author_login: detail.author_login.clone(),
        author_avatar_url: detail.author_avatar_url.clone(),
        comments_count: detail.comments_count,
        state: detail.state.clone(),
        updated_at: detail.updated_at.clone(),
        url: detail.url.clone(),
        labels: detail.labels.clone(),
        assignees: detail.assignees.clone(),
        repository_default_branch: None,
    }
}

fn issue_state_label(state: &str) -> &'static str {
    match state {
        "CLOSED" => "Closed",
        _ => "Open",
    }
}

fn issue_body_text(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        "No description.".to_string()
    } else {
        body.to_string()
    }
}

fn issue_badge_row(labels: Vec<String>, assignees: Vec<String>, comments: i64) -> impl IntoElement {
    div()
        .flex()
        .gap(px(6.0))
        .flex_wrap()
        .when(comments > 0, |el| {
            el.child(subtle_pill(&format!("{comments} comments")))
        })
        .children(labels.into_iter().take(4).map(|label| subtle_pill(&label)))
        .children(
            assignees
                .into_iter()
                .take(3)
                .map(|assignee| subtle_pill(&format!("assigned {assignee}"))),
        )
}
