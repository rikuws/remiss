use super::*;

pub(super) fn render_overview_surface(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();
    let detail_state = s.active_detail_state();

    let Some(detail) = detail else {
        return div().into_any_element();
    };

    let review_action = s.review_action;
    let review_body = s.review_body.clone();
    let review_loading = s.review_loading;
    let review_message = s.review_message.clone();
    let review_success = s.review_success;
    let loaded_from_cache = detail_state
        .and_then(|d| d.snapshot.as_ref())
        .map(|sn| sn.loaded_from_cache)
        .unwrap_or(false);
    let syncing = detail_state.map(|d| d.syncing).unwrap_or(false);
    let review_brief_state = detail_state
        .map(|d| d.review_brief_state.clone())
        .unwrap_or_default();
    let local_repository_loading = detail_state
        .map(|d| d.local_repository_loading)
        .unwrap_or(false);
    let viewer_login = viewer_login(&s);
    let is_local_review = crate::local_review::is_local_review_detail(detail);
    let is_own_pull_request = viewer_login
        .as_deref()
        .map(|viewer_login| detail.author_login == viewer_login)
        .unwrap_or(false);
    let review_status = summarize_review_status(&detail.reviewers, &detail.latest_reviews);
    let own_pr_feedback = viewer_login
        .as_deref()
        .filter(|_| is_own_pull_request)
        .map(|viewer_login| {
            summarize_own_pr_feedback(
                &detail.review_threads,
                viewer_login,
                &s.unread_review_comment_ids,
            )
        })
        .unwrap_or_default();
    let thread_digest =
        summarize_thread_activity(&detail.review_threads, &s.unread_review_comment_ids);
    let recent_activity = summarize_recent_activity(detail, &s.unread_review_comment_ids);
    let automation_activity_key = automation_activity_key(detail);
    let automation_activity_expanded = s
        .expanded_automation_activity_keys
        .contains(&automation_activity_key);
    let participants = summarize_participants(detail, &review_status);
    let provider = s.selected_tour_provider();
    let provider_status = s.selected_tour_provider_status().cloned();
    let provider_loading = s.code_tour_provider_loading;
    let provider_error = s.code_tour_provider_error.clone();
    let brief_automatic_enabled = s
        .code_tour_settings
        .settings
        .automatically_generates_for(&detail.repository);
    let brief_settings_loaded = s.code_tour_settings.loaded;

    let state_for_review = state.clone();
    let state_for_brief = state.clone();
    let state_for_threads = state.clone();
    let state_for_activity = state.clone();
    let state_for_files = state.clone();

    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .flex_wrap()
        .gap(px(20.0))
        .child(
            div()
                .flex_1()
                .min_w(px(460.0))
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(render_overview_summary_strip(
                    detail,
                    is_own_pull_request,
                    &state_for_files,
                ))
                .child(render_pull_request_summary_panel(
                    detail,
                    loaded_from_cache,
                    syncing,
                ))
                .child(render_review_brief_panel(
                    review_brief_state,
                    provider,
                    provider_status,
                    provider_loading,
                    provider_error,
                    local_repository_loading,
                    brief_automatic_enabled,
                    brief_settings_loaded,
                    &state_for_brief,
                ))
                .child(render_review_snapshot_panel(
                    detail,
                    &review_status,
                    &own_pr_feedback,
                    &thread_digest,
                    is_own_pull_request,
                    &state_for_threads,
                ))
                .child(render_recent_activity_panel(
                    &recent_activity,
                    &automation_activity_key,
                    automation_activity_expanded,
                    &state_for_activity,
                ))
                .when(!is_own_pull_request && !is_local_review, |el| {
                    el.child(render_submit_review_panel(
                        review_action,
                        review_body,
                        s.review_editor_active,
                        review_loading,
                        review_message,
                        review_success,
                        &state_for_review,
                    ))
                }),
        )
        .child(
            div()
                .w(detail_side_width())
                .min_w(px(240.0))
                .max_w(detail_side_width())
                .flex_shrink_0()
                .child(render_brief_details_view(
                    detail,
                    &review_status,
                    &participants,
                )),
        )
        .into_any_element()
}

pub(super) fn pr_detail_section() -> Div {
    div()
        .w_full()
        .min_w_0()
        .rounded(radius())
        .bg(bg_overlay())
        .px(px(18.0))
        .py(px(16.0))
}

fn render_overview_summary_strip(
    detail: &github::PullRequestDetail,
    is_own_pull_request: bool,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let state = state.clone();
    let action_label = if is_own_pull_request {
        "Open review workspace"
    } else {
        "Start review"
    };

    pr_detail_section().py(px(12.0)).child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .flex_wrap()
            .child(
                div()
                    .flex()
                    .gap(px(16.0))
                    .items_center()
                    .flex_wrap()
                    .child(render_overview_metric(
                        detail.commits_count.to_string(),
                        "commits",
                        fg_emphasis(),
                    ))
                    .child(render_overview_metric(
                        detail.changed_files.to_string(),
                        "files",
                        fg_emphasis(),
                    ))
                    .child(render_overview_metric(
                        detail.comments_count.to_string(),
                        "comments",
                        fg_muted(),
                    ))
                    .child(render_change_meter(detail.additions, detail.deletions)),
            )
            .child(review_button(action_label, move |_, window, cx| {
                enter_files_surface(&state, window, cx)
            })),
    )
}

fn render_review_brief_panel(
    brief_state: ReviewBriefState,
    provider: CodeTourProvider,
    provider_status: Option<CodeTourProviderStatus>,
    provider_loading: bool,
    provider_error: Option<String>,
    local_repository_loading: bool,
    automatic_enabled: bool,
    settings_loaded: bool,
    state: &Entity<AppState>,
) -> AnyElement {
    let busy = provider_loading
        || local_repository_loading
        || brief_state.loading
        || brief_state.generating;
    let provider_needs_setup = provider_status
        .as_ref()
        .map(|status| !status.available || !status.authenticated)
        .unwrap_or(false)
        || (!provider_loading && provider_error.is_some());
    let state_for_generate = state.clone();
    let state_for_settings = state.clone();
    let has_brief = brief_state.document.is_some();

    let header = div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .flex_wrap()
        .mb(px(14.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(eyebrow("Review Brief"))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Pre-diff briefing"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(6.0))
                .flex_wrap()
                .when(provider_loading, |el| el.child(badge("Checking provider")))
                .when(local_repository_loading, |el| {
                    el.child(badge("Preparing checkout"))
                })
                .when(!busy && !provider_needs_setup && has_brief, |el| {
                    let trigger_generate =
                        move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                            review_intelligence::trigger_review_intelligence(
                                &state_for_generate,
                                window,
                                cx,
                                ReviewIntelligenceScope::BriefOnly,
                                true,
                            );
                        };

                    el.child(review_brief_icon_button(trigger_generate))
                }),
        );

    let body = if let Some(brief) = brief_state.document.as_ref() {
        render_review_brief_document(brief).into_any_element()
    } else if provider_needs_setup {
        render_review_brief_setup_needed(
            provider_status.as_ref(),
            provider_error.as_deref(),
            state,
            state_for_settings,
        )
        .into_any_element()
    } else if let Some(error) = brief_state.error.as_deref() {
        render_review_brief_error(error, state).into_any_element()
    } else if busy {
        render_review_brief_progress(
            provider,
            provider_status.as_ref(),
            provider_error.as_deref(),
            local_repository_loading,
            brief_state.progress_text.as_deref(),
        )
        .into_any_element()
    } else if !settings_loaded {
        render_review_brief_progress(
            provider,
            provider_status.as_ref(),
            provider_error.as_deref(),
            local_repository_loading,
            Some("Preparing briefing for this pull request."),
        )
        .into_any_element()
    } else {
        render_review_brief_idle(state, state_for_settings, automatic_enabled).into_any_element()
    };

    pr_detail_section()
        .child(header)
        .child(body)
        .into_any_element()
}

fn review_brief_icon_button(
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("regenerate-review-brief")
        .w(px(24.0))
        .h(px(24.0))
        .rounded(radius_sm())
        .bg(transparent())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(|_, cx| build_review_brief_tooltip("Regenerate pre-diff briefing", cx))
        .hover(|style| style.bg(bg_selected()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(LucideIcon::RefreshCw, 13.0, fg_subtle()))
}

fn build_review_brief_tooltip(text: &'static str, cx: &mut App) -> AnyView {
    AnyView::from(cx.new(|_| ReviewBriefTooltip {
        text: SharedString::from(text),
    }))
}

struct ReviewBriefTooltip {
    text: SharedString,
}

impl Render for ReviewBriefTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(radius_sm())
            .border_1()
            .border_color(transparent())
            .bg(bg_overlay())
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg_emphasis())
            .child(self.text.clone())
    }
}

fn render_review_brief_document(brief: &ReviewBrief) -> impl IntoElement {
    div().w_full().min_w_0().flex().child(
        div()
            .w_full()
            .min_w_0()
            .max_w(px(760.0))
            .whitespace_normal()
            .text_size(px(13.0))
            .line_height(px(20.0))
            .font_weight(FontWeight::NORMAL)
            .text_color(fg_default())
            .child(brief.brief_paragraph.clone()),
    )
}

fn render_review_brief_progress(
    provider: CodeTourProvider,
    provider_status: Option<&CodeTourProviderStatus>,
    provider_error: Option<&str>,
    local_repository_loading: bool,
    progress_text: Option<&str>,
) -> impl IntoElement {
    let title = progress_text
        .map(str::to_string)
        .unwrap_or_else(|| "Preparing briefing.".to_string());

    div()
        .p(px(14.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .border_1()
        .border_color(transparent())
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .flex_wrap()
        .child(
            div()
                .flex_grow()
                .min_w(px(REVIEW_BRIEF_STATUS_TEXT_MIN_WIDTH))
                .max_w(px(REVIEW_BRIEF_STATUS_TEXT_MAX_WIDTH))
                .flex()
                .items_start()
                .gap(px(9.0))
                .child(lucide_icon(LucideIcon::Sparkles, 15.0, accent()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_normal()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .child(title),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_normal()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child("Start review remains available while this finishes."),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(6.0))
                .flex_wrap()
                .child(badge(provider.label()))
                .when_some(provider_status, |el, status| {
                    el.child(badge(if status.available && status.authenticated {
                        "Ready"
                    } else {
                        "Setup needed"
                    }))
                })
                .when(local_repository_loading, |el| el.child(badge("Checkout")))
                .when_some(provider_error, |el, error| el.child(error_text(error))),
        )
}

fn render_review_brief_error(error: &str, state: &Entity<AppState>) -> impl IntoElement {
    let state_for_retry = state.clone();
    div()
        .p(px(14.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .border_1()
        .border_color(transparent())
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .flex_wrap()
        .child(
            div()
                .flex_grow()
                .min_w(px(REVIEW_BRIEF_STATUS_TEXT_MIN_WIDTH))
                .max_w(px(REVIEW_BRIEF_STATUS_TEXT_MAX_WIDTH))
                .flex()
                .items_start()
                .gap(px(9.0))
                .child(lucide_icon(LucideIcon::CircleHelp, 15.0, danger()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(danger())
                        .child(error.to_string()),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .child(ghost_button("Retry", move |_, window, cx| {
                    review_intelligence::trigger_review_intelligence(
                        &state_for_retry,
                        window,
                        cx,
                        ReviewIntelligenceScope::BriefOnly,
                        true,
                    );
                })),
        )
}

fn render_review_brief_setup_needed(
    provider_status: Option<&CodeTourProviderStatus>,
    provider_error: Option<&str>,
    state: &Entity<AppState>,
    state_for_settings: Entity<AppState>,
) -> impl IntoElement {
    let state_for_retry = state.clone();
    let message = provider_status
        .map(|status| status.message.clone())
        .or_else(|| provider_error.map(str::to_string))
        .unwrap_or_else(|| {
            "The selected AI provider needs setup before briefing generation.".to_string()
        });

    div()
        .p(px(14.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .border_1()
        .border_color(transparent())
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .flex_wrap()
        .child(
            div()
                .flex_grow()
                .min_w(px(REVIEW_BRIEF_STATUS_TEXT_MIN_WIDTH))
                .max_w(px(REVIEW_BRIEF_STATUS_TEXT_MAX_WIDTH))
                .flex()
                .items_start()
                .gap(px(9.0))
                .child(lucide_icon(LucideIcon::Settings, 15.0, fg_muted()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(fg_muted())
                        .child(message),
                ),
        )
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .gap(px(6.0))
                .flex_wrap()
                .child(ghost_button("Retry", move |_, window, cx| {
                    review_intelligence::refresh_active_review_brief(
                        &state_for_retry,
                        window,
                        cx,
                        true,
                    );
                    review_intelligence::refresh_active_review_partner(
                        &state_for_retry,
                        window,
                        cx,
                        true,
                    );
                }))
                .child(ghost_button("Settings", move |_, _, cx| {
                    state_for_settings.update(cx, |state, cx| {
                        state.set_active_section(SectionId::Settings);
                        cx.notify();
                    });
                })),
        )
}

fn render_review_brief_idle(
    state: &Entity<AppState>,
    state_for_settings: Entity<AppState>,
    automatic_enabled: bool,
) -> impl IntoElement {
    let state_for_generate = state.clone();
    let copy = if automatic_enabled {
        "No cached review brief is available for this pull request head yet."
    } else {
        "Automatic briefings use the Background code tours repository setting."
    };

    div()
        .p(px(14.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .border_1()
        .border_color(transparent())
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .flex_wrap()
        .child(
            div()
                .flex_grow()
                .min_w(px(REVIEW_BRIEF_STATUS_TEXT_MIN_WIDTH))
                .max_w(px(REVIEW_BRIEF_STATUS_TEXT_MAX_WIDTH))
                .whitespace_normal()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(fg_muted())
                .child(copy),
        )
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .gap(px(6.0))
                .flex_wrap()
                .child(ghost_button("Generate", move |_, window, cx| {
                    review_intelligence::trigger_review_intelligence(
                        &state_for_generate,
                        window,
                        cx,
                        ReviewIntelligenceScope::BriefOnly,
                        true,
                    );
                }))
                .child(ghost_button("Settings", move |_, _, cx| {
                    state_for_settings.update(cx, |state, cx| {
                        state.set_active_section(SectionId::Settings);
                        cx.notify();
                    });
                })),
        )
}

fn render_overview_metric(value: String, label: &str, color: Rgba) -> impl IntoElement {
    div()
        .flex()
        .items_baseline()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(mono_font_family())
                .text_color(color)
                .child(value),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg_subtle())
                .child(label.to_string()),
        )
}

fn render_change_meter(additions: i64, deletions: i64) -> impl IntoElement {
    let additions = additions.max(0);
    let deletions = deletions.max(0);
    let total = additions + deletions;
    let segments = 10usize;
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
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .gap(px(5.0))
                .items_center()
                .font_family(mono_font_family())
                .text_size(px(12.0))
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
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_subtle())
                .child("diff".to_string()),
        )
}

fn readable_text(text: String) -> impl IntoElement {
    div()
        .max_w(px(760.0))
        .text_size(px(14.0))
        .line_height(px(22.0))
        .text_color(fg_default())
        .child(text)
}

fn section_label(label: &str) -> impl IntoElement {
    div()
        .mb(px(8.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg_muted())
        .child(label.to_string())
}

fn render_review_snapshot_panel(
    detail: &github::PullRequestDetail,
    review_status: &ReviewStatusSummary,
    own_pr_feedback: &[OwnPrFeedbackItem],
    thread_digest: &[ThreadDigestItem],
    is_own_pull_request: bool,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let review_decision = detail.review_decision.clone();
    let highlight_count = if is_own_pull_request {
        format!("{} highlights", own_pr_feedback.len())
    } else {
        format!("{} threads", thread_digest.len())
    };
    let unresolved_feedback = own_pr_feedback
        .iter()
        .filter(|item| !item.is_resolved)
        .count();
    let unresolved_threads = thread_digest
        .iter()
        .filter(|item| !item.is_resolved)
        .count();
    let summary_text = if is_own_pull_request {
        build_own_pr_summary_text(review_status, own_pr_feedback)
    } else {
        build_review_snapshot_text(review_status, thread_digest, detail.comments_count as usize)
    };

    pr_detail_section()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
                .mb(px(14.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(if is_own_pull_request {
                            "Feedback Summary"
                        } else {
                            "Review Snapshot"
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(badge(&highlight_count))
                        .when_some(review_decision, |el, decision| {
                            el.child(review_decision_badge(&decision))
                        }),
                ),
        )
        .child(readable_text(summary_text))
        .child(
            div()
                .mt(px(12.0))
                .flex()
                .gap(px(8.0))
                .flex_wrap()
                .child(if is_own_pull_request {
                    render_snapshot_stat(
                        unresolved_feedback.to_string(),
                        "Needs reply",
                        "Reviewer threads still waiting on you.",
                        accent(),
                    )
                    .into_any_element()
                } else {
                    render_snapshot_stat(
                        unresolved_threads.to_string(),
                        "Open threads",
                        "Thread discussions still in progress.",
                        accent(),
                    )
                    .into_any_element()
                })
                .child(render_snapshot_stat(
                    review_status.waiting.len().to_string(),
                    "Waiting",
                    "Requested reviewers without a latest verdict.",
                    fg_muted(),
                ))
                .child(render_snapshot_stat(
                    review_status.approved.len().to_string(),
                    "Approved",
                    "Reviewers whose latest review is approval.",
                    success(),
                ))
                .child(render_snapshot_stat(
                    review_status.changes_requested.len().to_string(),
                    "Changes",
                    "Reviewers currently requesting updates.",
                    danger(),
                )),
        )
        .child(div().mt(px(18.0)).child(render_thread_focus_panel(
            own_pr_feedback,
            thread_digest,
            is_own_pull_request,
            state,
        )))
}

fn render_snapshot_stat(value: String, label: &str, _hint: &str, color: Rgba) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(5.0))
        .rounded(radius_sm())
        .bg(bg_subtle())
        .flex()
        .items_center()
        .gap(px(7.0))
        .child(
            div()
                .font_family(mono_font_family())
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(value),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg_muted())
                .child(label.to_string()),
        )
}

fn render_thread_focus_panel(
    own_pr_feedback: &[OwnPrFeedbackItem],
    thread_digest: &[ThreadDigestItem],
    is_own_pull_request: bool,
    state: &Entity<AppState>,
) -> AnyElement {
    if is_own_pull_request {
        let has_more = own_pr_feedback.len() > 4;

        div()
            .w_full()
            .min_w_0()
            .pt(px(4.0))
            .child(section_label("Needs your attention"))
            .when(own_pr_feedback.is_empty(), |el| {
                el.child(panel_state_text("No reviewer comments yet."))
            })
            .child(
                div().flex().flex_col().children(
                    own_pr_feedback
                        .iter()
                        .take(4)
                        .map(|item| render_own_feedback_card(item, state)),
                ),
            )
            .when(has_more, |el| {
                el.child(
                    div()
                        .mt(px(10.0))
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(format!(
                            "{} more feedback thread{} in Files view.",
                            own_pr_feedback.len() - 4,
                            if own_pr_feedback.len() - 4 == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )),
                )
            })
            .into_any_element()
    } else {
        let has_more = thread_digest.len() > 4;

        div()
            .w_full()
            .min_w_0()
            .pt(px(4.0))
            .child(section_label("Comment threads"))
            .when(thread_digest.is_empty(), |el| {
                el.child(panel_state_text("No review threads yet."))
            })
            .child(
                div().flex().flex_col().children(
                    thread_digest
                        .iter()
                        .take(4)
                        .map(|item| render_thread_digest_card(item, state)),
                ),
            )
            .when(has_more, |el| {
                el.child(
                    div()
                        .mt(px(10.0))
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(format!(
                            "{} more thread{} in Files view.",
                            thread_digest.len() - 4,
                            if thread_digest.len() - 4 == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )),
                )
            })
            .into_any_element()
    }
}

fn render_own_feedback_card(
    item: &OwnPrFeedbackItem,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let state = state.clone();
    let selected_file_path = item.file_path.clone();
    let selected_anchor = item.anchor.clone();
    let unread_comment_ids = item.unread_comment_ids.clone();
    let updated_at = format_relative_time(&item.updated_at);

    div()
        .relative()
        .min_w_0()
        .pl(px(32.0))
        .py(px(12.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            state.update(cx, |state, cx| {
                state.mark_review_comments_read(unread_comment_ids.clone());
                state.selected_file_path = Some(selected_file_path.clone());
                state.selected_diff_anchor = Some(selected_anchor.clone());
                cx.notify();
            });
            enter_files_surface(&state, window, cx);
        })
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(13.0))
                .child(user_avatar(
                    &item.author_login,
                    item.author_avatar_url.as_deref(),
                    22.0,
                    false,
                )),
        )
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.0))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(13.0))
                                .text_color(fg_emphasis())
                                .child(item.author_login.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child(updated_at.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .justify_end()
                        .flex_shrink_0()
                        .child(subtle_badge(&item.subject_type.to_lowercase()))
                        .when(item.is_resolved, |el| {
                            el.child(tone_badge(
                                "resolved",
                                success(),
                                success_muted(),
                                diff_add_border(),
                            ))
                        })
                        .when(item.is_outdated, |el| el.child(subtle_badge("outdated")))
                        .when(item.unread_count > 0, |el| {
                            el.child(tone_badge(
                                &format!("{} new", item.unread_count),
                                accent(),
                                accent_muted(),
                                accent(),
                            ))
                        })
                        .child(subtle_badge(&format!("{} feedback", item.feedback_count))),
                ),
        )
        .child(
            div()
                .mt(px(4.0))
                .child(overflow_safe_code_label(&item.location_label, fg_muted())),
        )
        .child(div().mt(px(8.0)).max_w(px(760.0)).child(render_markdown(
            &format!(
                "own-pr-feedback-preview-{}-{}",
                item.file_path, item.updated_at
            ),
            &item.preview,
        )))
}

fn render_thread_digest_card(
    item: &ThreadDigestItem,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let state = state.clone();
    let selected_file_path = item.file_path.clone();
    let selected_anchor = item.anchor.clone();
    let unread_comment_ids = item.unread_comment_ids.clone();
    let updated_at = format_relative_time(&item.updated_at);
    let resolved_by = item.resolved_by_login.clone();

    div()
        .relative()
        .min_w_0()
        .pl(px(32.0))
        .py(px(12.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            state.update(cx, |state, cx| {
                state.mark_review_comments_read(unread_comment_ids.clone());
                state.selected_file_path = Some(selected_file_path.clone());
                state.selected_diff_anchor = Some(selected_anchor.clone());
                cx.notify();
            });
            enter_files_surface(&state, window, cx);
        })
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(13.0))
                .child(user_avatar(
                    &item.latest_author,
                    item.latest_author_avatar_url.as_deref(),
                    22.0,
                    false,
                )),
        )
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.0))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(13.0))
                                .text_color(fg_emphasis())
                                .child(item.latest_author.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child(updated_at.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .justify_end()
                        .flex_shrink_0()
                        .child(subtle_badge(&item.subject_type.to_lowercase()))
                        .when(item.is_resolved, |el| {
                            el.child(tone_badge(
                                resolved_by
                                    .as_deref()
                                    .map(|login| format!("resolved by {login}"))
                                    .unwrap_or_else(|| "resolved".to_string())
                                    .as_str(),
                                success(),
                                success_muted(),
                                diff_add_border(),
                            ))
                        })
                        .when(!item.is_resolved, |el| {
                            el.child(tone_badge("open", accent(), accent_muted(), accent()))
                        })
                        .when(item.is_outdated, |el| el.child(subtle_badge("outdated")))
                        .when(item.unread_count > 0, |el| {
                            el.child(tone_badge(
                                &format!("{} new", item.unread_count),
                                accent(),
                                accent_muted(),
                                accent(),
                            ))
                        })
                        .child(subtle_badge(&format!("{} comments", item.comment_count))),
                ),
        )
        .child(
            div()
                .mt(px(4.0))
                .child(overflow_safe_code_label(&item.location_label, fg_muted())),
        )
        .child(div().mt(px(8.0)).max_w(px(760.0)).child(render_markdown(
            &format!(
                "thread-digest-preview-{}-{}",
                item.file_path, item.updated_at
            ),
            &item.preview,
        )))
}

fn render_pull_request_summary_panel(
    detail: &github::PullRequestDetail,
    loaded_from_cache: bool,
    syncing: bool,
) -> impl IntoElement {
    pr_detail_section()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(12.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Summary"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(badge(if loaded_from_cache { "cache" } else { "live" }))
                        .when(syncing, |el| el.child(badge("refreshing"))),
                ),
        )
        .child(div().max_w(px(760.0)).child(if detail.body.is_empty() {
            div()
                .text_size(px(14.0))
                .line_height(px(22.0))
                .text_color(fg_muted())
                .child("No PR description provided.")
                .into_any_element()
        } else {
            render_markdown("pr-summary-body", &detail.body).into_any_element()
        }))
}

fn render_recent_activity_panel(
    activity: &[ActivityItem],
    automation_key: &str,
    automation_expanded: bool,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let mut human_activity = Vec::new();
    let mut automation_activity = Vec::new();
    for item in activity {
        if is_automation_actor(&item.author_login) {
            automation_activity.push(item);
        } else {
            human_activity.push(item);
        }
    }
    let displayed_count = human_activity.len() + usize::from(!automation_activity.is_empty());
    let visible_human_count = human_activity.len().min(10);
    let has_automation_activity = !automation_activity.is_empty();

    pr_detail_section()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(12.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Activity"),
                )
                .child(badge(&displayed_count.to_string())),
        )
        .when(activity.is_empty(), |el| {
            el.child(panel_state_text("No recent review or comment activity."))
        })
        .child(
            div()
                .flex()
                .flex_col()
                .children(
                    human_activity
                        .into_iter()
                        .take(10)
                        .enumerate()
                        .map(|(index, item)| {
                            render_activity_card(
                                item,
                                state,
                                index > 0,
                                index + 1 < visible_human_count || has_automation_activity,
                            )
                        }),
                )
                .when(has_automation_activity, |el| {
                    el.child(render_automation_activity_group(
                        automation_activity,
                        automation_key,
                        automation_expanded,
                        state,
                        visible_human_count > 0,
                    ))
                }),
        )
}

fn render_activity_card(
    item: &ActivityItem,
    state: &Entity<AppState>,
    connector_above: bool,
    connector_below: bool,
) -> AnyElement {
    if !item.thread_comments.is_empty() {
        return render_activity_thread_card(item, state, connector_above, connector_below)
            .into_any_element();
    }

    let clickable = item.file_path.is_some() && item.anchor.is_some();
    let state = state.clone();
    let file_path = item.file_path.clone();
    let anchor = item.anchor.clone();
    let timestamp = format_relative_time(&item.timestamp);

    div()
        .min_w_0()
        .py(px(4.0))
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(render_activity_timeline_avatar(
            &item.author_login,
            item.author_avatar_url.as_deref(),
            connector_above,
            connector_below,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .rounded(radius_sm())
                .border_1()
                .border_color(transparent())
                .px(px(12.0))
                .py(px(10.0))
                .flex()
                .flex_col()
                .when(clickable, |el| {
                    el.cursor_pointer()
                        .hover(|style| style.bg(hover_bg()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            state.update(cx, |state, cx| {
                                state.selected_file_path = file_path.clone();
                                state.selected_diff_anchor = anchor.clone();
                                cx.notify();
                            });
                            enter_files_surface(&state, window, cx);
                        })
                })
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(10.0))
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .flex_grow()
                                .min_w_0()
                                .when(item.kind != ActivityItemKind::Thread, |el| {
                                    el.child(activity_kind_badge(&item.kind))
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(item.title.clone()),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child(timestamp),
                        ),
                )
                .child(
                    div()
                        .mt(px(8.0))
                        .flex()
                        .items_start()
                        .gap(px(6.0))
                        .flex_wrap()
                        .min_w_0()
                        .when_some(item.location_label.clone(), |el, location| {
                            el.child(
                                div()
                                    .min_w_0()
                                    .max_w(px(720.0))
                                    .child(activity_location_text(&location)),
                            )
                        })
                        .when_some(item.status_label.clone(), |el, status| {
                            el.child(activity_status_badge(item, &status))
                        }),
                )
                .when(
                    item.thread_comments.is_empty() && !item.preview.is_empty(),
                    |el| {
                        el.child(div().mt(px(8.0)).max_w(px(760.0)).child(render_markdown(
                            &format!("activity-preview-{}-{}", item.author_login, item.timestamp),
                            &item.preview,
                        )))
                    },
                ),
        )
        .into_any_element()
}

fn render_activity_thread_card(
    item: &ActivityItem,
    state: &Entity<AppState>,
    connector_above: bool,
    connector_below: bool,
) -> impl IntoElement {
    let clickable = item.file_path.is_some() && item.anchor.is_some();
    let state = state.clone();
    let file_path = item.file_path.clone();
    let anchor = item.anchor.clone();
    let timestamp = format_relative_time(&item.timestamp);
    let comment_count = item.thread_comments.len();

    div()
        .flex()
        .flex_col()
        .child(
            div()
                .min_w_0()
                .py(px(4.0))
                .flex()
                .items_start()
                .gap(px(10.0))
                .child(render_activity_timeline_avatar(
                    &item.author_login,
                    item.author_avatar_url.as_deref(),
                    connector_above,
                    true,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .rounded(radius_sm())
                        .border_1()
                        .border_color(transparent())
                        .px(px(12.0))
                        .py(px(10.0))
                        .flex()
                        .flex_col()
                        .when(clickable, |el| {
                            el.cursor_pointer()
                                .hover(|style| style.bg(hover_bg()))
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    state.update(cx, |state, cx| {
                                        state.selected_file_path = file_path.clone();
                                        state.selected_diff_anchor = anchor.clone();
                                        cx.notify();
                                    });
                                    enter_files_surface(&state, window, cx);
                                })
                        })
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .justify_between()
                                .gap(px(10.0))
                                .min_w_0()
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg_emphasis())
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(item.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(12.0))
                                        .text_color(fg_muted())
                                        .child(timestamp),
                                ),
                        )
                        .child(
                            div()
                                .mt(px(8.0))
                                .flex()
                                .items_start()
                                .gap(px(6.0))
                                .flex_wrap()
                                .min_w_0()
                                .when_some(item.location_label.clone(), |el, location| {
                                    el.child(
                                        div()
                                            .min_w_0()
                                            .max_w(px(720.0))
                                            .child(activity_location_text(&location)),
                                    )
                                })
                                .when_some(item.status_label.clone(), |el, status| {
                                    el.child(activity_status_badge(item, &status))
                                }),
                        ),
                ),
        )
        .children(
            item.thread_comments
                .iter()
                .enumerate()
                .map(|(index, comment)| {
                    render_activity_thread_comment_row(
                        comment,
                        true,
                        index + 1 < comment_count || connector_below,
                    )
                }),
        )
}

fn render_activity_thread_comment_row(
    comment: &ActivityThreadComment,
    connector_above: bool,
    connector_below: bool,
) -> impl IntoElement {
    div()
        .min_w_0()
        .py(px(4.0))
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(render_activity_thread_comment_avatar(
            &comment.author_login,
            comment.author_avatar_url.as_deref(),
            connector_above,
            connector_below,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(12.0))
                .py(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(comment.author_login.clone()),
                        )
                        .child(
                            div()
                                .text_color(fg_subtle())
                                .child(format_relative_time(&comment.timestamp)),
                        ),
                )
                .child(div().mt(px(4.0)).max_w(px(760.0)).child(render_markdown(
                    &format!("activity-thread-comment-{}", comment.id),
                    &comment.body,
                ))),
        )
}

fn render_activity_thread_comment_avatar(
    login: &str,
    avatar_url: Option<&str>,
    connector_above: bool,
    connector_below: bool,
) -> impl IntoElement {
    div()
        .relative()
        .w(px(32.0))
        .min_h(px(42.0))
        .flex_shrink_0()
        .flex()
        .justify_center()
        .pt(px(2.0))
        .child(user_avatar(login, avatar_url, 18.0, false))
        .when(connector_above, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(15.5))
                    .w(px(1.0))
                    .h(px(4.0))
                    .bg(border_muted()),
            )
        })
        .when(connector_below, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(24.0))
                    .bottom(px(-6.0))
                    .left(px(15.5))
                    .w(px(1.0))
                    .bg(border_muted()),
            )
        })
}

fn render_activity_timeline_avatar(
    login: &str,
    avatar_url: Option<&str>,
    connector_above: bool,
    connector_below: bool,
) -> impl IntoElement {
    div()
        .relative()
        .w(px(32.0))
        .min_h(px(60.0))
        .flex_shrink_0()
        .flex()
        .justify_center()
        .pt(px(8.0))
        .child(user_avatar(login, avatar_url, 22.0, false))
        .when(connector_above, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(15.5))
                    .w(px(1.0))
                    .h(px(8.0))
                    .bg(border_muted()),
            )
        })
        .when(connector_below, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(34.0))
                    .bottom(px(0.0))
                    .left(px(15.5))
                    .w(px(1.0))
                    .bg(border_muted()),
            )
        })
}

fn render_activity_timeline_icon(
    icon: LucideIcon,
    connector_above: bool,
    connector_below: bool,
) -> impl IntoElement {
    div()
        .relative()
        .w(px(32.0))
        .min_h(px(58.0))
        .flex_shrink_0()
        .flex()
        .justify_center()
        .pt(px(9.0))
        .child(
            div()
                .size(px(20.0))
                .rounded(px(999.0))
                .bg(bg_emphasis())
                .border_1()
                .border_color(transparent())
                .flex()
                .items_center()
                .justify_center()
                .child(lucide_icon(icon, 12.0, fg_muted())),
        )
        .when(connector_above, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(15.5))
                    .w(px(1.0))
                    .h(px(9.0))
                    .bg(border_muted()),
            )
        })
        .when(connector_below, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(35.0))
                    .bottom(px(0.0))
                    .left(px(15.5))
                    .w(px(1.0))
                    .bg(border_muted()),
            )
        })
}

fn automation_activity_key(detail: &github::PullRequestDetail) -> String {
    format!(
        "{}#{}:automation-activity",
        detail.repository, detail.number
    )
}

pub(super) fn is_automation_actor(login: &str) -> bool {
    let login = login.trim().to_ascii_lowercase();
    if login.is_empty() {
        return false;
    }

    login.contains("[bot]")
        || login.ends_with("-bot")
        || login.ends_with("bot")
        || matches!(
            login.as_str(),
            "github-actions" | "dependabot" | "renovate" | "vercel" | "netlify" | "supabase"
        )
}

pub(super) fn automation_activity_needs_attention(item: &ActivityItem) -> bool {
    let mut haystack = String::new();
    haystack.push_str(&item.title);
    haystack.push(' ');
    haystack.push_str(&item.preview);
    if let Some(status) = item.status_label.as_deref() {
        haystack.push(' ');
        haystack.push_str(status);
    }
    let haystack = haystack.to_ascii_lowercase();

    [
        "fail",
        "error",
        "blocked",
        "denied",
        "unauthorized",
        "not authorized",
        "conflict",
        "cancelled",
        "canceled",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn render_automation_activity_group(
    items: Vec<&ActivityItem>,
    automation_key: &str,
    expanded: bool,
    state: &Entity<AppState>,
    connector_above: bool,
) -> impl IntoElement {
    let key = automation_key.to_string();
    let toggle_state = state.clone();
    let has_attention = items
        .iter()
        .any(|item| automation_activity_needs_attention(item));
    let latest = items
        .first()
        .map(|item| format_relative_time(&item.timestamp))
        .unwrap_or_default();
    let count = items.len();

    div()
        .flex()
        .flex_col()
        .child(
            div()
                .min_w_0()
                .py(px(4.0))
                .flex()
                .items_start()
                .gap(px(10.0))
                .child(render_activity_timeline_icon(
                    LucideIcon::Zap,
                    connector_above,
                    expanded,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .rounded(radius_sm())
                        .border_1()
                        .border_color(transparent())
                        .px(px(12.0))
                        .py(px(10.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(hover_bg()))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            toggle_state.update(cx, |state, cx| {
                                if !state.expanded_automation_activity_keys.insert(key.clone()) {
                                    state.expanded_automation_activity_keys.remove(&key);
                                }
                                cx.notify();
                            });
                        })
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child("Automation updates"),
                                )
                                .child(subtle_badge(&format!("{count} updates")))
                                .when(has_attention, |el| {
                                    el.child(tone_badge(
                                        "needs attention",
                                        danger(),
                                        danger_muted(),
                                        diff_remove_border(),
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child(if expanded { "Hide".to_string() } else { latest }),
                        ),
                ),
        )
        .when(expanded, |el| {
            el.children(
                items.into_iter().enumerate().map(|(index, item)| {
                    render_activity_card(item, state, true, index + 1 < count)
                }),
            )
        })
}
