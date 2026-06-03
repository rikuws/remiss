use std::path::PathBuf;

use gpui::prelude::*;
use gpui::*;

use crate::icons::{lucide_icon, LucideIcon};
use crate::local_review::{self, LocalReviewStatusKind, RememberedLocalRepository};
use crate::onboarding::WizardStepTarget;
use crate::review_intelligence::{self, ReviewIntelligenceScope};
use crate::review_session::load_review_session;
use crate::state::*;
use crate::theme::*;

use super::super::diff_view::{load_pull_request_file_content_flow, warm_structural_diffs_flow};
use super::super::tooltips::build_text_tooltip;
use super::tabs::compact_close_button;
use super::{close_workspace_tab, onboarding_highlight_shell, sidebar_utility_button};

pub(crate) fn refresh_active_local_review(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let path = {
        let s = state.read(cx);
        let Some(detail) = s
            .active_detail()
            .filter(|detail| local_review::is_local_review_detail(detail))
        else {
            return;
        };
        s.local_review_repositories
            .iter()
            .find(|item| item.repository == detail.repository)
            .map(|item| PathBuf::from(item.path.clone()))
    };

    if let Some(path) = path {
        open_local_review_from_path(state, path, false, window, cx);
    }
}

pub(super) fn trigger_add_local_repository(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(SharedString::from("Add Repository")),
    });
    let model = state.clone();

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let selected_path = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                Ok(Err(error)) => {
                    set_local_review_error(
                        &model,
                        format!("Failed to open folder picker: {error}"),
                        cx,
                    )
                    .await;
                    return;
                }
                Err(_) => {
                    set_local_review_error(
                        &model,
                        "Folder picker was closed before returning a path.".to_string(),
                        cx,
                    )
                    .await;
                    return;
                }
            };

            let Some(path) = selected_path else {
                return;
            };

            inspect_and_open_local_review(model, path, false, cx).await;
        })
        .detach();
}

fn open_local_review_from_path(
    state: &Entity<AppState>,
    path: PathBuf,
    fetch: bool,
    window: &mut Window,
    cx: &mut App,
) {
    mark_local_review_path_inspecting(state, &path, cx);
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            inspect_and_open_local_review(model, path, fetch, cx).await;
        })
        .detach();
}

pub(super) fn refresh_local_review_repositories(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let repositories = state.read(cx).local_review_repositories.clone();
    if repositories.is_empty() {
        return;
    }

    state.update(cx, |state, cx| {
        state.local_review_loading = true;
        state.local_review_error = None;
        for repository in &mut state.local_review_repositories {
            local_review::mark_repository_inspecting(repository);
        }
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result =
                cx.background_executor()
                    .spawn(async move {
                        Ok::<_, String>(
                            repositories
                                .into_iter()
                                .map(|remembered| {
                                    local_review::inspect_working_checkout(
                                        &PathBuf::from(&remembered.path),
                                        false,
                                    )
                                    .map(|inspection| {
                                        local_review::remembered_from_inspection(&inspection)
                                    })
                                    .unwrap_or_else(|error| RememberedLocalRepository {
                                        last_status: LocalReviewStatusKind::Error,
                                        last_message: Some(error),
                                        last_inspected_at_ms: None,
                                        ..remembered
                                    })
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .await;

            model
                .update(cx, |state, cx| {
                    state.local_review_loading = false;
                    match result {
                        Ok(updated) => {
                            state.local_review_repositories = updated;
                            let _ = local_review::save_remembered_repositories(
                                state.cache.as_ref(),
                                &state.local_review_repositories,
                            );
                        }
                        Err(error) => {
                            state.local_review_error = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

async fn inspect_and_open_local_review(
    model: Entity<AppState>,
    path: PathBuf,
    fetch: bool,
    cx: &mut AsyncWindowContext,
) {
    let result = cx
        .background_executor()
        .spawn({
            let path = path.clone();
            async move { local_review::inspect_working_checkout(&path, fetch) }
        })
        .await;

    match result {
        Ok(inspection) => {
            let detail_key = inspection.key.clone();
            let remembered = local_review::remembered_from_inspection(&inspection);
            let snapshot = local_review::detail_snapshot_from_inspection(&inspection);
            let summary = inspection.summary.clone();
            let local_repository_status = inspection.local_repository_status.clone();

            model
                .update(cx, |state, cx| {
                    local_review::upsert_remembered_repository(
                        &mut state.local_review_repositories,
                        remembered.clone(),
                    );
                    let _ = local_review::save_remembered_repositories(
                        state.cache.as_ref(),
                        &state.local_review_repositories,
                    );

                    state.open_tabs.retain(|tab| {
                        summary_key(tab) != detail_key
                            && !(tab.local_key.is_some() && tab.repository == summary.repository)
                    });
                    state.open_tabs.insert(0, summary.clone());
                    state.active_pr_key = Some(detail_key.clone());
                    state.active_surface = PullRequestSurface::Files;
                    state.pr_header_compact = false;
                    state.review_body.clear();
                    state.review_editor_active = false;
                    state.review_message = None;
                    state.review_success = false;
                    state.local_review_loading = false;
                    state.local_review_error = None;

                    let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
                    detail_state.snapshot = Some(snapshot.clone());
                    detail_state.loading = false;
                    detail_state.syncing = false;
                    detail_state.error = None;
                    detail_state.local_repository_status = Some(local_repository_status.clone());
                    detail_state.local_repository_loading = false;
                    detail_state.local_repository_error =
                        if local_repository_status.ready_for_local_features {
                            None
                        } else {
                            Some(local_repository_status.message.clone())
                        };

                    let cached_review_session =
                        load_review_session(state.cache.as_ref(), &detail_key)
                            .ok()
                            .flatten();
                    state.apply_review_session_document(&detail_key, cached_review_session);
                    state.ensure_active_selected_file_is_valid();
                    cx.notify();
                })
                .ok();

            load_pull_request_file_content_flow(model.clone(), None, cx).await;
            warm_structural_diffs_flow(model.clone(), cx).await;
            let should_run_background_review_intelligence = model
                .read_with(cx, |state, _| state.review_ai_background_jobs_enabled())
                .ok()
                .unwrap_or(false);
            if should_run_background_review_intelligence {
                review_intelligence::run_review_intelligence_flow(
                    model.clone(),
                    ReviewIntelligenceScope::StackOnly,
                    false,
                    true,
                    cx,
                )
                .await;
            }
        }
        Err(error) => {
            model
                .update(cx, |state, cx| {
                    state.local_review_loading = false;
                    state.local_review_error = Some(error.clone());
                    for repository in &mut state.local_review_repositories {
                        if PathBuf::from(&repository.path) == path {
                            repository.last_status = LocalReviewStatusKind::Error;
                            repository.last_message = Some(error.clone());
                        }
                    }
                    let _ = local_review::save_remembered_repositories(
                        state.cache.as_ref(),
                        &state.local_review_repositories,
                    );
                    cx.notify();
                })
                .ok();
        }
    }
}

async fn set_local_review_error(
    model: &Entity<AppState>,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            state.local_review_loading = false;
            state.local_review_error = Some(error);
            cx.notify();
        })
        .ok();
}

fn mark_local_review_path_inspecting(state: &Entity<AppState>, path: &PathBuf, cx: &mut App) {
    state.update(cx, |state, cx| {
        state.local_review_loading = true;
        state.local_review_error = None;
        for repository in &mut state.local_review_repositories {
            if PathBuf::from(&repository.path) == *path {
                local_review::mark_repository_inspecting(repository);
            }
        }
        cx.notify();
    });
}

pub(super) fn render_local_review_sidebar_section(
    state: &Entity<AppState>,
    cx: &App,
    icons_only: bool,
) -> impl IntoElement {
    let s = state.read(cx);
    let repositories = s.local_review_repositories.clone();
    let error = s.local_review_error.clone();
    let loading = s.local_review_loading;
    let highlight_add = s.is_onboarding_target(WizardStepTarget::LocalReview);
    let active_local_repository = s
        .active_detail()
        .filter(|detail| local_review::is_local_review_detail(detail))
        .map(|detail| detail.repository.clone());
    let state_for_add = state.clone();
    let base = div()
        .px(if icons_only { px(10.0) } else { px(14.0) })
        .pb(px(10.0))
        .flex()
        .flex_col()
        .gap(px(6.0));

    if icons_only {
        base.items_center()
            .child(onboarding_highlight_shell(
                highlight_add,
                sidebar_utility_button(
                    if loading {
                        LucideIcon::RefreshCw
                    } else {
                        LucideIcon::Plus
                    },
                    "Add local review",
                    false,
                    false,
                    move |_, window, cx| {
                        trigger_add_local_repository(&state_for_add, window, cx);
                    },
                ),
            ))
            .children(repositories.into_iter().map(|repository| {
                let state = state.clone();
                let state_for_close = state.clone();
                let path = PathBuf::from(repository.path.clone());
                let repository_key = repository.repository.clone();
                let active =
                    active_local_repository.as_deref() == Some(repository.repository.as_str());
                local_review_sidebar_row(
                    repository,
                    active,
                    true,
                    move |_, window, cx| {
                        open_local_review_from_path(&state, path.clone(), false, window, cx);
                    },
                    move |_, window, cx| {
                        close_local_review_sidebar_repository(
                            &state_for_close,
                            repository_key.clone(),
                            window,
                            cx,
                        );
                    },
                )
            }))
            .when_some(error, |el, error| {
                let tooltip = SharedString::from(error);
                el.child(
                    div()
                        .id("local-review-sidebar-error-icon")
                        .w_full()
                        .h(px(34.0))
                        .rounded(radius_sm())
                        .bg(danger_muted())
                        .flex()
                        .items_center()
                        .justify_center()
                        .tooltip(move |_, cx| build_text_tooltip(tooltip.clone(), cx))
                        .child(lucide_icon(LucideIcon::AlertTriangle, 15.0, danger())),
                )
            })
    } else {
        base.child(
            div()
                .px(px(6.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_subtle())
                        .child("LOCAL REVIEW"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(onboarding_highlight_shell(
                            highlight_add,
                            sidebar_utility_button(
                                if loading {
                                    LucideIcon::RefreshCw
                                } else {
                                    LucideIcon::Plus
                                },
                                "Add local review",
                                false,
                                false,
                                move |_, window, cx| {
                                    trigger_add_local_repository(&state_for_add, window, cx);
                                },
                            ),
                        )),
                ),
        )
        .when(repositories.is_empty(), |el| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(8.0))
                    .rounded(radius_sm())
                    .border_1()
                    .border_color(transparent())
                    .bg(bg_surface())
                    .text_size(px(11.0))
                    .line_height(px(15.0))
                    .text_color(fg_muted())
                    .child("Add a working checkout to review local changes on disk."),
            )
        })
        .children(repositories.into_iter().map(|repository| {
            let state = state.clone();
            let state_for_close = state.clone();
            let path = PathBuf::from(repository.path.clone());
            let repository_key = repository.repository.clone();
            let active = active_local_repository.as_deref() == Some(repository.repository.as_str());
            local_review_sidebar_row(
                repository,
                active,
                false,
                move |_, window, cx| {
                    open_local_review_from_path(&state, path.clone(), false, window, cx);
                },
                move |_, window, cx| {
                    close_local_review_sidebar_repository(
                        &state_for_close,
                        repository_key.clone(),
                        window,
                        cx,
                    );
                },
            )
        }))
        .when_some(error, |el, error| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(7.0))
                    .rounded(radius_sm())
                    .bg(danger_muted())
                    .text_size(px(11.0))
                    .line_height(px(15.0))
                    .text_color(danger())
                    .child(error),
            )
        })
    }
}

fn local_review_sidebar_row(
    repository: RememberedLocalRepository,
    active: bool,
    icons_only: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let repository_label = repository
        .repository
        .split('/')
        .last()
        .unwrap_or(&repository.repository)
        .to_string();
    let branch = repository
        .last_branch
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let status_label = local_review_status_label(repository.last_status);
    let status_color = local_review_status_color(repository.last_status);
    let close_id = SharedString::from(format!(
        "local-review-sidebar-close-{}",
        repository.repository
    ));
    let row_id = format!("local-review-sidebar-row-{}", repository.repository);

    if icons_only {
        let element_id = ElementId::Name(row_id.clone().into());
        let tooltip = SharedString::from(format!(
            "{} - {} - {}",
            repository_label, branch, status_label
        ));
        return div()
            .id(element_id)
            .w_full()
            .h(px(38.0))
            .rounded(radius_sm())
            .border_1()
            .border_color(transparent())
            .bg(if active { bg_emphasis() } else { bg_surface() })
            .flex()
            .items_center()
            .justify_center()
            .hover(move |style| {
                style
                    .bg(if active { bg_emphasis() } else { bg_selected() })
                    .text_color(fg_emphasis())
            })
            .tooltip(move |_, cx| build_text_tooltip(tooltip.clone(), cx))
            .on_mouse_down(MouseButton::Left, on_click)
            .child(lucide_icon(LucideIcon::GitBranch, 15.0, status_color));
    }

    div()
        .id(ElementId::Name(row_id.into()))
        .h(px(48.0))
        .px(px(9.0))
        .py(px(7.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if active { bg_emphasis() } else { bg_surface() })
        .flex()
        .items_center()
        .gap(px(8.0))
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(LucideIcon::GitBranch, 14.0, status_color))
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { fg_emphasis() } else { fg_default() })
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(repository_label),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w_0()
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(10.0))
                                .font_family(mono_font_family())
                                .text_color(fg_muted())
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(branch),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.0))
                                .font_family(mono_font_family())
                                .text_color(status_color)
                                .child(status_label),
                        ),
                ),
        )
        .child(compact_close_button(
            close_id,
            "Remove local review",
            on_close,
        ))
}

fn local_review_status_label(status: LocalReviewStatusKind) -> &'static str {
    match status {
        LocalReviewStatusKind::Ready => "ready",
        LocalReviewStatusKind::NoDiff => "no diff",
        LocalReviewStatusKind::Blocked => "blocked",
        LocalReviewStatusKind::Error => "error",
        LocalReviewStatusKind::Inspecting => "checking",
        LocalReviewStatusKind::Unknown => "unknown",
    }
}

fn local_review_status_color(status: LocalReviewStatusKind) -> Rgba {
    match status {
        LocalReviewStatusKind::Ready => success(),
        LocalReviewStatusKind::NoDiff => fg_subtle(),
        LocalReviewStatusKind::Blocked => warning(),
        LocalReviewStatusKind::Error => danger(),
        LocalReviewStatusKind::Inspecting => accent(),
        LocalReviewStatusKind::Unknown => fg_subtle(),
    }
}

fn close_local_review_sidebar_repository(
    state: &Entity<AppState>,
    repository: String,
    window: &mut Window,
    cx: &mut App,
) {
    let local_tab_keys = state
        .read(cx)
        .open_tabs
        .iter()
        .filter(|tab| tab.local_key.is_some() && tab.repository == repository)
        .map(summary_key)
        .collect::<Vec<_>>();

    for key in local_tab_keys {
        close_workspace_tab(state, key, window, cx);
    }

    state.update(cx, |s, cx| {
        s.local_review_repositories
            .retain(|item| item.repository != repository);
        let _ = local_review::save_remembered_repositories(
            s.cache.as_ref(),
            &s.local_review_repositories,
        );
        cx.notify();
    });
}
