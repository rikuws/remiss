use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui::prelude::*;
use gpui::*;

use crate::app_assets::APP_LOGO_ASSET;
use crate::app_menu::{
    AddLocalRepository, AddWaypoint, CheckForUpdates, CycleCodeTheme, DecreaseCodeFontSize,
    IncreaseCodeFontSize, JumpToNextReviewComment, OpenReviewFiles, OpenSelectedLineInSource,
    RefreshLocalRepositories, ResetCodeFontSize, ShowPullRequestBriefing, ShowSettings,
    SubmitReview, SwitchToAiTour, SwitchToCode, SwitchToDiff, SwitchToSource, SwitchToStack,
    SwitchToStructuralDiff, SyncWorkspace, ToggleCommandPalette, ToggleWaypointSpotlight,
};
use crate::branding::APP_NAME;
use crate::deep_link::{self, DeepLinkRequest};
use crate::github;
use crate::icons::{lucide_icon, LucideIcon};
use crate::local_review::{self, LocalReviewStatusKind, RememberedLocalRepository};
use crate::onboarding::WizardStepTarget;
use crate::review_session::{load_review_session, ReviewCenterMode};
use crate::state::*;
use crate::theme::*;

use super::diff_view::{
    ensure_active_review_focus_loaded, ensure_structural_diff_warmup_started, enter_files_surface,
    enter_stack_review_mode, switch_review_code_mode, toggle_waypoint_spotlight,
    trigger_add_waypoint_shortcut, trigger_submit_inline_comment,
    trigger_submit_review_from_review_mode, warm_structural_diffs_flow,
};
use super::palette::{render_palette, toggle_palette};
use super::pr_detail::render_pr_workspace;
use super::sections::render_section_workspace;
use super::settings::{
    cycle_diff_color_theme_preference, decrease_code_font_size_preference,
    increase_code_font_size_preference, prepare_settings_view, reset_code_font_size_preference,
    trigger_software_update_check, update_theme_preference,
};
use super::welcome_wizard::{refresh_onboarding_gh_status, render_onboarding_wizard};
use super::workspace_sync::{
    sync_workspace_flow, trigger_sync_workspace, wait_for_workspace_poll_interval,
};

pub struct RootView {
    state: Entity<AppState>,
    workspace_route_key: Option<WorkspaceRouteKey>,
    workspace_route_animation_started_at: Option<Instant>,
}

const APP_SIDEBAR_EXPANDED_WIDTH: f32 = 216.0;
const APP_SIDEBAR_HIDDEN_WIDTH: f32 = 0.0;
const APP_SIDEBAR_TRAFFIC_LIGHT_CLEARANCE: f32 = 58.0;
pub(crate) const APP_CHROME_HEIGHT: f32 = 64.0;
pub(crate) const APP_TRAFFIC_LIGHT_LEFT: f32 = 12.0;
pub(crate) const APP_TRAFFIC_LIGHT_TOP: f32 = 11.0;
const APP_TITLEBAR_SIDEBAR_TOGGLE_LEFT: f32 = 76.0;
const APP_TITLEBAR_CONTROL_SIZE: f32 = 24.0;
const APP_TITLEBAR_CONTROL_TOP: f32 = 8.0;
const APP_TITLEBAR_CONTROL_ICON_SIZE: f32 = 13.0;
const APP_CHROME_HIDDEN_LEFT_INSET: f32 = 206.0;
const APP_SIDEBAR_ANIMATION_MS: u64 = 220;
const NOTIFICATION_DRAWER_ANIMATION_MS: u64 = 160;
const WORKSPACE_ROUTE_ANIMATION_MS: u64 = 160;
const WORKSPACE_ROUTE_INITIAL_CONTENT_OPACITY: f32 = 0.58;
const WORKSPACE_ROUTE_INITIAL_VEIL_ALPHA: f32 = 0.22;

#[derive(Clone, Debug, PartialEq)]
struct WorkspaceRouteKey {
    active_section: SectionId,
    active_pr_key: Option<String>,
    active_surface: PullRequestSurface,
    active_center_mode: ReviewCenterMode,
    active_code_lens_mode: ReviewCenterMode,
}

#[derive(Clone, Copy)]
struct WorkspaceRouteTransition {
    progress: f32,
}

#[derive(Clone, Copy)]
struct AppMenuAvailability {
    has_active_detail: bool,
    has_active_remote_detail: bool,
    has_local_repositories: bool,
    has_next_review_comment: bool,
    has_selected_diff_line: bool,
}

impl RootView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_appearance = window.appearance();
        state.update(cx, |state, _| {
            state.set_window_appearance(initial_appearance);
        });
        cx.observe_window_appearance(window, {
            let state = state.clone();
            move |_, window, cx| {
                let appearance = window.appearance();
                state.update(cx, |state, cx| {
                    state.set_window_appearance(appearance);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.observe_window_bounds(window, {
            let state = state.clone();
            move |_, window, cx| {
                if window.is_fullscreen() || window.is_maximized() {
                    return;
                }

                let cache = state.read(cx).cache.clone();
                let _ =
                    crate::window_settings::save_window_size(cache.as_ref(), window.bounds().size);
            }
        })
        .detach();

        // Bootstrap: load workspace from cache, then sync in background.
        let model = state.clone();
        cx.spawn_in(window, async move |_this, cx| {
            // Load bootstrap status
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };

            let result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move { github::load_workspace_snapshot(&cache) }
                })
                .await;

            model
                .update(cx, |state, cx| {
                    state.workspace_loading = false;
                    state.bootstrap_loading = false;
                    match &result {
                        Ok(ws) => {
                            state.gh_available = ws.auth.is_authenticated;
                            state.workspace = Some(ws.clone());
                        }
                        Err(e) => {
                            state.workspace_error = Some(e.clone());
                        }
                    }
                    cx.notify();
                })
                .ok();

            maybe_bootstrap_debug_pull_request(&model, cache.as_ref(), cx).await;

            // Check gh version
            let gh_result = cx
                .background_executor()
                .spawn(async { crate::gh::run(&["--version"]) })
                .await;

            model
                .update(cx, |state, cx| {
                    if let Ok(output) = gh_result {
                        if output.exit_code == Some(0) {
                            state.gh_available = true;
                            state.gh_version = output.stdout.lines().next().map(str::to_string);
                        }
                    }
                    cx.notify();
                })
                .ok();

            // Now sync workspace in background.
            model
                .update(cx, |state, cx| {
                    state.workspace_syncing = true;
                    cx.notify();
                })
                .ok();

            sync_workspace_flow(model.clone(), cx).await;

            loop {
                wait_for_workspace_poll_interval(cx).await;

                let should_sync = model
                    .read_with(cx, |state, _| {
                        state.is_authenticated() && !state.workspace_syncing
                    })
                    .ok()
                    .unwrap_or(false);
                if !should_sync {
                    continue;
                }

                model
                    .update(cx, |state, cx| {
                        if state.workspace_syncing {
                            return;
                        }

                        state.workspace_syncing = true;
                        cx.notify();
                    })
                    .ok();

                sync_workspace_flow(model.clone(), cx).await;
            }
        })
        .detach();

        refresh_onboarding_gh_status(&state, window, cx);
        refresh_local_review_repositories(&state, window, cx);

        Self {
            state,
            workspace_route_key: None,
            workspace_route_animation_started_at: None,
        }
    }

    fn workspace_route_transition(
        &mut self,
        window: &mut Window,
        cx: &App,
    ) -> WorkspaceRouteTransition {
        let current_key = {
            let state = self.state.read(cx);
            workspace_route_key(&state)
        };

        match &self.workspace_route_key {
            None => {
                self.workspace_route_key = Some(current_key);
            }
            Some(previous_key) if *previous_key != current_key => {
                self.workspace_route_key = Some(current_key);
                self.workspace_route_animation_started_at = Some(Instant::now());
            }
            Some(_) => {}
        }

        let progress = self
            .workspace_route_animation_started_at
            .map(|started_at| {
                let duration = Duration::from_millis(WORKSPACE_ROUTE_ANIMATION_MS).as_secs_f32();
                let raw_progress = (started_at.elapsed().as_secs_f32() / duration).clamp(0.0, 1.0);
                if raw_progress < 1.0 {
                    window.request_animation_frame();
                    ease_in_out(raw_progress).clamp(0.0, 1.0)
                } else {
                    self.workspace_route_animation_started_at = None;
                    1.0
                }
            })
            .unwrap_or(1.0);

        WorkspaceRouteTransition { progress }
    }
}

async fn maybe_bootstrap_debug_pull_request(
    model: &Entity<AppState>,
    _cache: &crate::cache::CacheStore,
    cx: &mut AsyncWindowContext,
) {
    let Some(debug_target) = std::env::var("REMISS_DEBUG_OPEN_PR")
        .or_else(|_| std::env::var("REVIEWBUDDY_DEBUG_OPEN_PR"))
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };

    let Some((repository, number)) = parse_debug_pull_request_target(&debug_target) else {
        return;
    };

    open_remote_pull_request_flow(
        model.clone(),
        repository,
        number,
        PullRequestSurface::Overview,
        cx,
    )
    .await;
}

pub(crate) fn open_deep_link_request(
    state: &Entity<AppState>,
    request: DeepLinkRequest,
    window: &mut Window,
    cx: &mut App,
) {
    match request {
        DeepLinkRequest::GitHubPullRequest { repository, number } => {
            let model = state.clone();
            window
                .spawn(cx, async move |cx: &mut AsyncWindowContext| {
                    open_remote_pull_request_flow(
                        model,
                        repository,
                        number,
                        PullRequestSurface::Files,
                        cx,
                    )
                    .await;
                })
                .detach();
        }
    }
}

async fn open_remote_pull_request_flow(
    model: Entity<AppState>,
    repository: String,
    number: i64,
    initial_surface: PullRequestSurface,
    cx: &mut AsyncWindowContext,
) {
    let detail_key = pr_key(&repository, number);
    let cached_review_session = model
        .read_with(cx, |state, _| {
            load_review_session(state.cache.as_ref(), &detail_key)
                .ok()
                .flatten()
        })
        .ok()
        .flatten();
    let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
    let Some(cache) = cache else {
        return;
    };

    model
        .update(cx, |state, cx| {
            if !state
                .open_tabs
                .iter()
                .any(|tab| pr_key(&tab.repository, tab.number) == detail_key)
            {
                state
                    .open_tabs
                    .insert(0, placeholder_pull_request_summary(&repository, number));
            }
            state.set_active_section(SectionId::Pulls);
            state.active_surface = initial_surface;
            state.active_pr_key = Some(detail_key.clone());
            state.pr_header_compact = false;
            state.review_body.clear();
            state.review_editor_active = false;
            state.review_message = None;
            state.review_success = false;

            let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
            detail_state.loading = detail_state
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.detail.as_ref())
                .is_none();
            detail_state.syncing = true;
            detail_state.error = None;
            state.apply_review_session_document(&detail_key, cached_review_session.clone());
            cx.notify();
        })
        .ok();

    let snapshot = match cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = repository.clone();
            async move { github::sync_pull_request_detail(&cache, &repository, number) }
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            model
                .update(cx, |state, cx| {
                    let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
                    detail_state.loading = false;
                    detail_state.syncing = false;
                    detail_state.error = Some(error);
                    cx.notify();
                })
                .ok();
            return;
        }
    };

    let Some(detail) = snapshot.detail.clone() else {
        let error = if !snapshot.auth.is_authenticated {
            snapshot.auth.message.clone()
        } else {
            format!("Pull request {repository}#{number} was not found.")
        };
        model
            .update(cx, |state, cx| {
                let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
                detail_state.loading = false;
                detail_state.syncing = false;
                detail_state.error = Some(error);
                cx.notify();
            })
            .ok();
        return;
    };

    let summary = summary_from_detail(&detail);

    model
        .update(cx, |state, cx| {
            if let Some(tab) = state
                .open_tabs
                .iter()
                .position(|tab| pr_key(&tab.repository, tab.number) == detail_key)
                .and_then(|index| state.open_tabs.get_mut(index))
            {
                *tab = summary.clone();
            } else {
                state.open_tabs.insert(0, summary);
            }

            state.set_active_section(SectionId::Pulls);
            state.active_surface = initial_surface;
            state.active_pr_key = Some(detail_key.clone());
            state.pr_header_compact = false;
            state.review_body.clear();
            state.review_editor_active = false;
            state.review_message = None;
            state.review_success = false;

            let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
            detail_state.snapshot = Some(snapshot.clone());
            detail_state.loading = false;
            detail_state.syncing = false;
            detail_state.error = None;

            state.apply_review_session_document(&detail_key, cached_review_session.clone());
            state.ensure_active_selected_file_is_valid();
            cx.notify();
        })
        .ok();

    super::diff_view::load_pull_request_file_content_flow(model.clone(), None, cx).await;
    warm_structural_diffs_flow(model.clone(), cx).await;
}

fn parse_debug_pull_request_target(target: &str) -> Option<(String, i64)> {
    let (repository, number) = target.trim().rsplit_once('#')?;
    let number = number.parse::<i64>().ok()?;
    Some((repository.to_string(), number))
}

fn summary_from_detail(detail: &github::PullRequestDetail) -> github::PullRequestSummary {
    github::PullRequestSummary {
        local_key: None,
        repository: detail.repository.clone(),
        number: detail.number,
        title: detail.title.clone(),
        author_login: detail.author_login.clone(),
        author_avatar_url: detail.author_avatar_url.clone(),
        is_draft: detail.is_draft,
        comments_count: detail.comments_count,
        additions: detail.additions,
        deletions: detail.deletions,
        changed_files: detail.changed_files,
        state: detail.state.clone(),
        review_decision: detail.review_decision.clone(),
        updated_at: detail.updated_at.clone(),
        url: detail.url.clone(),
    }
}

fn placeholder_pull_request_summary(repository: &str, number: i64) -> github::PullRequestSummary {
    github::PullRequestSummary {
        local_key: None,
        repository: repository.to_string(),
        number,
        title: format!("Pull request #{number}"),
        author_login: "unknown".to_string(),
        author_avatar_url: None,
        is_draft: false,
        comments_count: 0,
        additions: 0,
        deletions: 0,
        changed_files: 0,
        state: "LOADING".to_string(),
        review_decision: None,
        updated_at: String::new(),
        url: deep_link::github_pull_request_web_url(repository, number),
    }
}

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

fn trigger_add_local_repository(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
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

fn refresh_local_review_repositories(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
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

            super::diff_view::load_pull_request_file_content_flow(model.clone(), None, cx).await;
            warm_structural_diffs_flow(model.clone(), cx).await;
            crate::review_intelligence::run_review_intelligence_flow(
                model.clone(),
                crate::review_intelligence::ReviewIntelligenceScope::TourOnly,
                false,
                false,
                cx,
            )
            .await;
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

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace_route_transition = self.workspace_route_transition(window, cx);
        let (
            palette_visible,
            notification_drawer_open,
            onboarding_wizard_visible,
            app_menu_availability,
        ) = {
            let state = self.state.read(cx);
            (
                state.palette_open || state.palette_closing,
                state.notification_drawer_open,
                state.active_onboarding_wizard.is_some(),
                AppMenuAvailability {
                    has_active_detail: state.active_detail().is_some(),
                    has_active_remote_detail: state.active_detail().is_some()
                        && !state.active_is_local_review(),
                    has_local_repositories: !state.local_review_repositories.is_empty(),
                    has_next_review_comment: state.next_review_comment_location().is_some(),
                    has_selected_diff_line: state.selected_diff_line_target().is_some(),
                },
            )
        };

        let root = div()
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(bg_canvas())
            .text_color(fg_default())
            .text_size(px(14.0))
            .font_family(ui_font_family())
            .child(render_app_sidebar(&self.state, cx))
            .child(render_main_column(
                &self.state,
                workspace_route_transition,
                window,
                cx,
            ))
            .child(render_titlebar_sidebar_toggle(&self.state, cx))
            .when(notification_drawer_open, |el| {
                el.child(render_notification_drawer(&self.state, cx))
            })
            .when(palette_visible, |el| {
                el.child(render_palette(&self.state, cx))
            })
            .when(onboarding_wizard_visible, |el| {
                el.child(render_onboarding_wizard(&self.state, cx))
            });

        attach_app_menu_action_handlers(root, self.state.clone(), app_menu_availability)
    }
}

fn attach_app_menu_action_handlers(
    element: Div,
    state: Entity<AppState>,
    availability: AppMenuAvailability,
) -> Div {
    let state_for_palette = state.clone();
    let element = element.on_action(move |_: &ToggleCommandPalette, _, cx| {
        toggle_palette(&state_for_palette, cx);
        cx.stop_propagation();
    });

    let state_for_settings = state.clone();
    let element = element.on_action(move |_: &ShowSettings, window, cx| {
        show_settings_from_menu(&state_for_settings, window, cx);
        cx.stop_propagation();
    });

    let state_for_updates = state.clone();
    let element = element.on_action(move |_: &CheckForUpdates, _, cx| {
        trigger_software_update_check(&state_for_updates, cx);
        cx.stop_propagation();
    });

    let state_for_sync = state.clone();
    let element = element.on_action(move |_: &SyncWorkspace, window, cx| {
        trigger_sync_workspace(&state_for_sync, window, cx);
        cx.stop_propagation();
    });

    let state_for_add_local = state.clone();
    let element = element.on_action(move |_: &AddLocalRepository, window, cx| {
        trigger_add_local_repository(&state_for_add_local, window, cx);
        cx.stop_propagation();
    });

    let state_for_refresh_local = state.clone();
    let element = element.when(availability.has_local_repositories, move |element| {
        element.on_action(move |_: &RefreshLocalRepositories, window, cx| {
            refresh_local_review_repositories(&state_for_refresh_local, window, cx);
            cx.stop_propagation();
        })
    });

    let state_for_increase_font = state.clone();
    let element = element.on_action(move |_: &IncreaseCodeFontSize, window, cx| {
        increase_code_font_size_preference(&state_for_increase_font, window, cx);
        cx.stop_propagation();
    });

    let state_for_decrease_font = state.clone();
    let element = element.on_action(move |_: &DecreaseCodeFontSize, window, cx| {
        decrease_code_font_size_preference(&state_for_decrease_font, window, cx);
        cx.stop_propagation();
    });

    let state_for_reset_font = state.clone();
    let element = element.on_action(move |_: &ResetCodeFontSize, window, cx| {
        reset_code_font_size_preference(&state_for_reset_font, window, cx);
        cx.stop_propagation();
    });

    let state_for_code_theme = state.clone();
    let element = element.on_action(move |_: &CycleCodeTheme, window, cx| {
        cycle_diff_color_theme_preference(&state_for_code_theme, window, cx);
        cx.stop_propagation();
    });

    let state_for_waypoint_search = state.clone();
    let element = element.on_action(move |_: &ToggleWaypointSpotlight, _, cx| {
        toggle_waypoint_spotlight(&state_for_waypoint_search, cx);
        cx.stop_propagation();
    });

    let state_for_add_waypoint = state.clone();
    let element = element.when(availability.has_selected_diff_line, move |element| {
        element.on_action(move |_: &AddWaypoint, _, cx| {
            trigger_add_waypoint_shortcut(&state_for_add_waypoint, cx);
            cx.stop_propagation();
        })
    });

    let state_for_open_source = state.clone();
    let element = element.when(availability.has_selected_diff_line, move |element| {
        element.on_action(move |_: &OpenSelectedLineInSource, window, cx| {
            crate::temp_source_window::open_temp_source_window_for_selected_diff_line(
                &state_for_open_source,
                window,
                cx,
            );
            cx.stop_propagation();
        })
    });

    let state_for_open_files = state.clone();
    let state_for_switch_code = state.clone();
    let state_for_diff = state.clone();
    let state_for_structural = state.clone();
    let state_for_source = state.clone();
    let state_for_ai_tour = state.clone();
    let state_for_stack = state.clone();
    let element = element.when(availability.has_active_detail, move |element| {
        element
            .on_action(move |_: &OpenReviewFiles, window, cx| {
                enter_files_surface(&state_for_open_files, window, cx);
                cx.stop_propagation();
            })
            .on_action(move |_: &SwitchToCode, window, cx| {
                switch_to_code_from_menu(&state_for_switch_code, window, cx);
                cx.stop_propagation();
            })
            .on_action(move |_: &SwitchToDiff, window, cx| {
                switch_code_lens_from_menu(
                    &state_for_diff,
                    ReviewCenterMode::SemanticDiff,
                    window,
                    cx,
                );
                cx.stop_propagation();
            })
            .on_action(move |_: &SwitchToStructuralDiff, window, cx| {
                switch_code_lens_from_menu(
                    &state_for_structural,
                    ReviewCenterMode::StructuralDiff,
                    window,
                    cx,
                );
                cx.stop_propagation();
            })
            .on_action(move |_: &SwitchToSource, window, cx| {
                switch_code_lens_from_menu(
                    &state_for_source,
                    ReviewCenterMode::SourceBrowser,
                    window,
                    cx,
                );
                cx.stop_propagation();
            })
            .on_action(move |_: &SwitchToAiTour, window, cx| {
                switch_to_ai_tour_from_menu(&state_for_ai_tour, window, cx);
                cx.stop_propagation();
            })
            .on_action(move |_: &SwitchToStack, window, cx| {
                enter_stack_review_mode(&state_for_stack, window, cx);
                cx.stop_propagation();
            })
    });

    let state_for_briefing = state.clone();
    let state_for_submit = state.clone();
    let element = element.when(availability.has_active_remote_detail, move |element| {
        element
            .on_action(move |_: &ShowPullRequestBriefing, _, cx| {
                show_pull_request_briefing_from_menu(&state_for_briefing, cx);
                cx.stop_propagation();
            })
            .on_action(move |_: &SubmitReview, window, cx| {
                submit_review_from_menu(&state_for_submit, window, cx);
                cx.stop_propagation();
            })
    });

    let state_for_next_comment = state.clone();
    element.when(availability.has_next_review_comment, move |element| {
        element.on_action(move |_: &JumpToNextReviewComment, window, cx| {
            jump_to_next_review_comment_from_menu(&state_for_next_comment, window, cx);
            cx.stop_propagation();
        })
    })
}

fn show_settings_from_menu(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    prepare_settings_view(state, window, cx);
    state.update(cx, |state, cx| {
        state.set_active_section(SectionId::Settings);
        state.active_pr_key = None;
        state.palette_open = false;
        state.palette_selected_index = 0;
        cx.notify();
    });
}

fn show_pull_request_briefing_from_menu(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.active_detail().is_none() || state.active_is_local_review() {
            return;
        }
        state.active_surface = PullRequestSurface::Overview;
        state.pr_header_compact = false;
        state.persist_active_review_session();
        cx.notify();
    });
}

fn switch_to_code_from_menu(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.active_detail().is_none() {
            return;
        }
        state.active_surface = PullRequestSurface::Files;
        state.pr_header_compact = false;
        state.enter_code_review_mode();
        state.persist_active_review_session();
        cx.notify();
    });
    ensure_active_review_focus_loaded(state, window, cx);
}

fn switch_code_lens_from_menu(
    state: &Entity<AppState>,
    mode: ReviewCenterMode,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        if state.active_detail().is_none() {
            return;
        }
        state.active_surface = PullRequestSurface::Files;
        state.pr_header_compact = false;
        cx.notify();
    });
    switch_review_code_mode(state, mode, window, cx);
}

fn switch_to_ai_tour_from_menu(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    enter_stack_review_mode(state, window, cx);
}

fn jump_to_next_review_comment_from_menu(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let location = state.read(cx).next_review_comment_location();
    if let Some(location) = location {
        state.update(cx, |state, cx| {
            state.active_surface = PullRequestSurface::Files;
            state.pr_header_compact = false;
            state.set_review_file_collapsed(&location.file_path, false);
            state.navigate_to_review_location(location, true);
            state.persist_active_review_session();
            cx.notify();
        });
        ensure_active_review_focus_loaded(state, window, cx);
    }
}

fn submit_review_from_menu(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let submit_inline_comment = {
        let state = state.read(cx);
        state.active_review_line_action.is_some()
            && state.review_line_action_mode == ReviewLineActionMode::Comment
    };

    if submit_inline_comment {
        trigger_submit_inline_comment(state, window, cx);
    } else {
        trigger_submit_review_from_review_mode(state, window, cx);
    }
}

fn render_app_sidebar(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let hidden = s.app_sidebar_collapsed;
    let active_section = s.active_section;
    let is_authenticated = s.is_authenticated();
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let theme_preference = s.theme_preference;
    let sidebar_width = if hidden {
        APP_SIDEBAR_HIDDEN_WIDTH
    } else {
        APP_SIDEBAR_EXPANDED_WIDTH
    };
    let sync_label = if workspace_syncing {
        "Syncing workspace"
    } else {
        "Sync workspace"
    };
    let sync_color = if workspace_syncing {
        accent()
    } else if workspace_error.is_some() {
        danger()
    } else if is_authenticated {
        success()
    } else {
        fg_muted()
    };

    let state_for_nav = state.clone();
    let state_for_sync = state.clone();
    let state_for_theme = state.clone();
    let animation_key = ("app-sidebar", usize::from(hidden));

    div()
        .w(px(sidebar_width))
        .flex_shrink_0()
        .min_h_0()
        .bg(bg_overlay())
        .border_r(if hidden { px(0.0) } else { px(1.0) })
        .border_color(border_muted())
        .overflow_hidden()
        .child(
            div()
                .w(px(APP_SIDEBAR_EXPANDED_WIDTH))
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .opacity(if hidden { 0.0 } else { 1.0 })
                .child(render_sidebar_brand())
                .child(
                    div()
                        .px(px(14.0))
                        .pt(px(4.0))
                        .pb(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children(
                            SectionId::all()
                                .iter()
                                .filter(|section| **section != SectionId::Issues)
                                .map(|section| {
                                    let section = *section;
                                    let count = s.section_count(section);
                                    let state = state_for_nav.clone();
                                    sidebar_nav_button(
                                        section.label(),
                                        sidebar_icon_for_section(section),
                                        count,
                                        active_section == section,
                                        false,
                                        move |_, window, cx| {
                                            if section == SectionId::Settings {
                                                prepare_settings_view(&state, window, cx);
                                            }
                                            state.update(cx, |s, cx| {
                                                s.set_active_section(section);
                                                s.active_pr_key = None;
                                                s.palette_open = false;
                                                s.palette_selected_index = 0;
                                                cx.notify();
                                            });
                                        },
                                    )
                                }),
                        ),
                )
                .child(div().flex_grow().min_h(px(16.0)))
                .child(render_local_review_sidebar_section(state, cx))
                .child(
                    div()
                        .px(px(14.0))
                        .pb(px(14.0))
                        .pt(px(12.0))
                        .border_t(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .text_size(px(10.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child("THEME"),
                                )
                                .child(div().flex().gap(px(6.0)).flex_row().children(
                                    ThemePreference::all().iter().map(|candidate| {
                                        let candidate = *candidate;
                                        let state = state_for_theme.clone();
                                        sidebar_theme_button(
                                            theme_icon(candidate),
                                            theme_preference == candidate,
                                            false,
                                            move |_, window, cx| {
                                                update_theme_preference(
                                                    &state, candidate, window, cx,
                                                );
                                            },
                                        )
                                    }),
                                )),
                        )
                        .child(sidebar_action_button(
                            LucideIcon::RefreshCw,
                            sync_label,
                            false,
                            sync_color,
                            move |_, window, cx| {
                                trigger_sync_workspace(&state_for_sync, window, cx)
                            },
                        )),
                ),
        )
        .with_animation(
            animation_key,
            Animation::new(Duration::from_millis(APP_SIDEBAR_ANIMATION_MS))
                .with_easing(ease_in_out),
            move |el, delta| {
                let progress = sidebar_hidden_progress(hidden, delta);
                el.w(lerp_px(
                    APP_SIDEBAR_EXPANDED_WIDTH,
                    APP_SIDEBAR_HIDDEN_WIDTH,
                    progress,
                ))
            },
        )
}

fn render_sidebar_brand() -> impl IntoElement {
    div()
        .px(px(14.0))
        .pt(px(APP_SIDEBAR_TRAFFIC_LIGHT_CLEARANCE))
        .pb(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(
                    img(APP_LOGO_ASSET)
                        .size(px(28.0))
                        .object_fit(ObjectFit::Contain),
                )
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(APP_NAME),
                ),
        )
}

fn render_local_review_sidebar_section(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
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

    div()
        .px(px(14.0))
        .pb(px(10.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
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

fn local_review_sidebar_row(
    repository: RememberedLocalRepository,
    active: bool,
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
    div()
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
        .cursor_pointer()
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

fn render_main_column(
    state: &Entity<AppState>,
    workspace_route_transition: WorkspaceRouteTransition,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    div()
        .flex_grow()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .child(render_workspace_chrome(state, cx))
        .child(render_workspace_body(
            state,
            workspace_route_transition,
            window,
            cx,
        ))
}

fn render_titlebar_sidebar_toggle(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let sidebar_hidden = s.app_sidebar_collapsed;
    let state_for_sidebar = state.clone();
    let sidebar_tooltip = if sidebar_hidden {
        "Show sidebar"
    } else {
        "Hide sidebar"
    };
    let sidebar_icon = if sidebar_hidden {
        LucideIcon::PanelLeftOpen
    } else {
        LucideIcon::PanelLeftClose
    };

    div()
        .absolute()
        .left(px(APP_TITLEBAR_SIDEBAR_TOGGLE_LEFT))
        .top(px(APP_TITLEBAR_CONTROL_TOP))
        .child(titlebar_icon_button(
            "titlebar-sidebar-toggle",
            sidebar_icon,
            sidebar_tooltip,
            false,
            move |_, _, cx| {
                state_for_sidebar.update(cx, |state, cx| {
                    state.app_sidebar_collapsed = !sidebar_hidden;
                    cx.notify();
                });
            },
        ))
}

fn render_workspace_chrome(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let active_pr_key = s.active_pr_key.clone();
    let active_surface = s.active_surface;
    let active_center_mode = s
        .active_review_session()
        .map(|session| session.center_mode)
        .unwrap_or(ReviewCenterMode::SemanticDiff);
    let active_code_lens = s.active_code_lens_mode();
    let has_active_pr = active_pr_key.is_some();
    let active_is_local_review = s.active_is_local_review();
    let unread_count = s
        .active_detail()
        .map(|detail| s.unread_review_comment_ids_for_detail(detail).len())
        .unwrap_or(0);
    let drawer_open = s.notification_drawer_open;
    let sidebar_hidden = s.app_sidebar_collapsed;
    let file_tree_visible = s
        .active_review_session()
        .map(|session| session.show_file_tree)
        .unwrap_or(true);
    let show_file_tree_toggle = active_surface == PullRequestSurface::Files
        && s.active_detail().is_some()
        && !file_tree_visible;
    let file_tree_tooltip = "Show file tree";
    let file_tree_icon = LucideIcon::PanelLeftOpen;
    let tabs: Vec<_> = s.open_tabs.clone();
    let highlight_review_surface = s.is_onboarding_target(WizardStepTarget::TutorialReview);
    let highlight_guided_review = s.is_onboarding_target(WizardStepTarget::GuidedReview);
    let state_for_tabs = state.clone();
    let state_for_notifications = state.clone();
    let state_for_briefing = state.clone();
    let state_for_review = state.clone();
    let state_for_code = state.clone();
    let state_for_stack = state.clone();
    let state_for_diff_lens = state.clone();
    let state_for_structural_lens = state.clone();
    let state_for_source_lens = state.clone();
    let state_for_file_tree = state.clone();
    let code_mode_active = matches!(
        active_center_mode,
        ReviewCenterMode::SemanticDiff
            | ReviewCenterMode::StructuralDiff
            | ReviewCenterMode::SourceBrowser
    );

    div()
        .h(px(APP_CHROME_HEIGHT))
        .flex_shrink_0()
        .bg(bg_canvas())
        .border_b(px(1.0))
        .border_color(border_muted())
        .pl(if sidebar_hidden {
            px(APP_CHROME_HIDDEN_LEFT_INSET)
        } else {
            px(14.0)
        })
        .pr(px(14.0))
        .py(px(10.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(render_workspace_tabs(state_for_tabs, active_pr_key, tabs))
        .when(has_active_pr && !active_is_local_review, |el| {
            el.child(onboarding_highlight_shell(
                highlight_review_surface,
                chrome_segmented_control(vec![
                    chrome_segment(
                        "Briefing",
                        active_surface == PullRequestSurface::Overview,
                        false,
                        move |_, window, cx| {
                            state_for_briefing.update(cx, |state, cx| {
                                state.active_surface = PullRequestSurface::Overview;
                                state.pr_header_compact = false;
                                state.persist_active_review_session();
                                cx.notify();
                            });
                            crate::review_intelligence::refresh_active_review_brief(
                                &state_for_briefing,
                                window,
                                cx,
                                true,
                            );
                            crate::review_intelligence::refresh_active_review_partner(
                                &state_for_briefing,
                                window,
                                cx,
                                true,
                            );
                        },
                    ),
                    chrome_segment(
                        "Review",
                        active_surface == PullRequestSurface::Files,
                        false,
                        move |_, window, cx| {
                            enter_files_surface(&state_for_review, window, cx);
                        },
                    ),
                ]),
            ))
        })
        .when(
            has_active_pr && active_surface == PullRequestSurface::Files && code_mode_active,
            |el| {
                el.child(chrome_segmented_control(vec![
                    chrome_segment(
                        "Diff",
                        active_code_lens == ReviewCenterMode::SemanticDiff,
                        false,
                        move |_, window, cx| {
                            switch_review_code_mode(
                                &state_for_diff_lens,
                                ReviewCenterMode::SemanticDiff,
                                window,
                                cx,
                            );
                        },
                    ),
                    chrome_segment(
                        "Structural",
                        active_code_lens == ReviewCenterMode::StructuralDiff,
                        false,
                        move |_, window, cx| {
                            switch_review_code_mode(
                                &state_for_structural_lens,
                                ReviewCenterMode::StructuralDiff,
                                window,
                                cx,
                            );
                        },
                    ),
                    chrome_segment(
                        "Source",
                        active_code_lens == ReviewCenterMode::SourceBrowser,
                        false,
                        move |_, window, cx| {
                            switch_review_code_mode(
                                &state_for_source_lens,
                                ReviewCenterMode::SourceBrowser,
                                window,
                                cx,
                            );
                        },
                    ),
                ]))
            },
        )
        .when(show_file_tree_toggle, |el| {
            el.child(chrome_icon_button(
                "workspace-file-tree-toggle",
                file_tree_icon,
                file_tree_tooltip,
                file_tree_visible,
                move |_, _, cx| {
                    state_for_file_tree.update(cx, |state, cx| {
                        state.set_review_file_tree_visible(!file_tree_visible);
                        state.persist_active_review_session();
                        cx.notify();
                    });
                },
            ))
        })
        .when(has_active_pr, |el| {
            el.child(onboarding_highlight_shell(
                highlight_guided_review,
                chrome_segmented_control(vec![
                    chrome_segment(
                        "Code",
                        code_mode_active,
                        active_surface != PullRequestSurface::Files,
                        move |_, window, cx| {
                            state_for_code.update(cx, |state, cx| {
                                state.active_surface = PullRequestSurface::Files;
                                state.enter_code_review_mode();
                                state.persist_active_review_session();
                                cx.notify();
                            });
                            ensure_active_review_focus_loaded(&state_for_code, window, cx);
                        },
                    ),
                    chrome_segment(
                        "Guided Review",
                        active_center_mode == ReviewCenterMode::GuidedReview,
                        active_surface != PullRequestSurface::Files,
                        move |_, window, cx| {
                            enter_stack_review_mode(&state_for_stack, window, cx);
                        },
                    ),
                ]),
            ))
        })
        .child(
            div()
                .relative()
                .child(titlebar_icon_button(
                    "workspace-notification-drawer",
                    LucideIcon::Bell,
                    "Notifications",
                    drawer_open,
                    move |_, _, cx| {
                        state_for_notifications.update(cx, |state, cx| {
                            state.notification_drawer_open = !drawer_open;
                            cx.notify();
                        });
                    },
                ))
                .when(unread_count > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(-5.0))
                            .right(px(-5.0))
                            .min_w(px(14.0))
                            .h(px(14.0))
                            .px(px(3.0))
                            .rounded(px(999.0))
                            .bg(danger())
                            .text_size(px(9.0))
                            .font_family(mono_font_family())
                            .text_color(bg_canvas())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(unread_count.min(99).to_string()),
                    )
                })
                .into_any_element(),
        )
}

fn titlebar_icon_button(
    id: &'static str,
    icon: LucideIcon,
    tooltip: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id =
        SharedString::from(format!("titlebar-icon-button-{id}-{}", usize::from(active)));

    div()
        .id(id)
        .w(px(APP_TITLEBAR_CONTROL_SIZE))
        .h(px(APP_TITLEBAR_CONTROL_SIZE))
        .rounded(px(6.0))
        .bg(if active { bg_emphasis() } else { transparent() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            APP_TITLEBAR_CONTROL_ICON_SIZE,
            if active { fg_emphasis() } else { fg_subtle() },
        ))
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
            },
        )
}

fn render_workspace_tabs(
    state: Entity<AppState>,
    active_pr_key: Option<String>,
    tabs: Vec<github::PullRequestSummary>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .id("workspace-tabs-scroll")
        .overflow_x_scroll()
        .min_w_0()
        .flex_grow()
        .children(tabs.into_iter().map(|tab| {
            let key = summary_key(&tab);
            let is_local = tab.local_key.is_some();
            let is_active = active_pr_key.as_deref() == Some(&key);
            let state = state.clone();
            let state_for_close = state.clone();
            let key_for_select = key.clone();
            let key_for_close = key.clone();
            pr_tab(
                &tab.repository,
                tab.number,
                &tab.title,
                tab.additions,
                tab.deletions,
                &tab.state,
                tab.is_draft,
                is_local,
                is_active,
                move |_, window, cx| {
                    let cached_review_session = {
                        let cache = state.read(cx).cache.clone();
                        load_review_session(cache.as_ref(), &key_for_select)
                            .ok()
                            .flatten()
                    };
                    state.update(cx, |s, cx| {
                        s.active_pr_key = Some(key_for_select.clone());
                        s.set_active_section(SectionId::Pulls);
                        s.palette_open = false;
                        s.palette_selected_index = 0;
                        s.detail_states.entry(key_for_select.clone()).or_default();
                        s.apply_review_session_document(
                            &key_for_select,
                            cached_review_session.clone(),
                        );
                        s.ensure_active_selected_file_is_valid();
                        cx.notify();
                    });
                    ensure_active_review_focus_loaded(&state, window, cx);
                    ensure_structural_diff_warmup_started(&state, window, cx);
                },
                move |_, window, cx| {
                    close_workspace_tab(&state_for_close, key_for_close.clone(), window, cx);
                },
            )
        }))
}

fn close_workspace_tab(state: &Entity<AppState>, key: String, window: &mut Window, cx: &mut App) {
    let next_active_key = state.update(cx, |s, cx| {
        let Some(index) = s.open_tabs.iter().position(|tab| summary_key(tab) == key) else {
            return None;
        };
        let closed_active = s.active_pr_key.as_deref() == Some(key.as_str());

        s.open_tabs.remove(index);
        s.detail_states.remove(&key);

        if !closed_active {
            cx.notify();
            return None;
        }

        let next_key = s
            .open_tabs
            .get(index)
            .or_else(|| {
                index
                    .checked_sub(1)
                    .and_then(|previous| s.open_tabs.get(previous))
            })
            .map(summary_key);

        s.active_pr_key = next_key.clone();
        s.palette_open = false;
        s.palette_selected_index = 0;
        s.pr_header_compact = false;
        s.review_body.clear();
        s.review_editor_active = false;
        s.review_message = None;
        s.review_success = false;

        if let Some(next_key) = next_key.as_deref() {
            s.set_active_section(SectionId::Pulls);
            if local_review::is_local_review_key(next_key) {
                s.active_surface = PullRequestSurface::Files;
            }
            s.detail_states.entry(next_key.to_string()).or_default();
        } else {
            s.selected_file_path = None;
            s.selected_diff_anchor = None;
            s.active_surface = PullRequestSurface::Overview;
        }

        cx.notify();
        next_key
    });

    if let Some(next_key) = next_active_key {
        let cached_review_session = {
            let cache = state.read(cx).cache.clone();
            load_review_session(cache.as_ref(), &next_key)
                .ok()
                .flatten()
        };
        state.update(cx, |s, cx| {
            if s.active_pr_key.as_deref() == Some(next_key.as_str()) {
                s.apply_review_session_document(&next_key, cached_review_session);
                cx.notify();
            }
        });
        ensure_active_review_focus_loaded(state, window, cx);
        ensure_structural_diff_warmup_started(state, window, cx);
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

fn workspace_route_key(state: &AppState) -> WorkspaceRouteKey {
    let active_center_mode = state
        .active_review_session()
        .map(|session| session.center_mode)
        .unwrap_or(ReviewCenterMode::SemanticDiff);

    WorkspaceRouteKey {
        active_section: state.active_section,
        active_pr_key: state.active_pr_key.clone(),
        active_surface: state.active_surface,
        active_center_mode,
        active_code_lens_mode: state.active_code_lens_mode(),
    }
}

fn render_workspace_body(
    state: &Entity<AppState>,
    workspace_route_transition: WorkspaceRouteTransition,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let s = state.read(cx);
    let has_active_pr = s.active_pr_key.is_some();
    let progress = workspace_route_transition.progress;
    let content_opacity = lerp_f32(WORKSPACE_ROUTE_INITIAL_CONTENT_OPACITY, 1.0, progress);
    let veil_alpha = lerp_f32(WORKSPACE_ROUTE_INITIAL_VEIL_ALPHA, 0.0, progress);

    div()
        .relative()
        .flex_grow()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                .opacity(content_opacity)
                .child(if has_active_pr {
                    render_pr_workspace(state, window, cx).into_any_element()
                } else {
                    render_section_workspace(state, cx).into_any_element()
                }),
        )
        .when(veil_alpha > 0.0, |el| {
            el.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(with_alpha(bg_canvas(), veil_alpha)),
            )
        })
}

fn chrome_icon_button(
    id: &'static str,
    icon: LucideIcon,
    tooltip: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .w(px(34.0))
        .h(px(34.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if active {
            bg_emphasis()
        } else {
            control_button_bg()
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            16.0,
            if active { fg_emphasis() } else { fg_muted() },
        ))
}

fn chrome_segmented_control(children: Vec<AnyElement>) -> impl IntoElement {
    div()
        .h(px(34.0))
        .p(px(3.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(control_track_bg())
        .flex()
        .items_center()
        .gap(px(1.0))
        .children(children)
}

fn onboarding_highlight_shell(active: bool, child: impl IntoElement) -> AnyElement {
    div()
        .rounded(px(8.0))
        .border_1()
        .border_color(transparent())
        .bg(if active {
            with_alpha(focus_border(), 0.12)
        } else {
            transparent()
        })
        .p(px(2.0))
        .child(child)
        .into_any_element()
}

fn chrome_segment(
    label: &'static str,
    active: bool,
    disabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let animation_id =
        SharedString::from(format!("chrome-segment-{label}-{}", usize::from(active)));

    div()
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(transparent())
        .bg(if active { bg_emphasis() } else { transparent() })
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .flex()
        .items_center()
        .justify_center()
        .opacity(if disabled { 0.5 } else { 1.0 })
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if !disabled {
                on_click(event, window, cx);
            }
        })
        .child(label)
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
                    .text_color(mix_rgba(fg_muted(), fg_emphasis(), progress))
            },
        )
        .into_any_element()
}

fn render_notification_drawer(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let active_detail = s.active_detail();
    let unread_ids = active_detail
        .map(|detail| s.unread_review_comment_ids_for_detail(detail))
        .unwrap_or_default();
    let unread_id_set = unread_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
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
        .top(px(64.0))
        .right(px(16.0))
        .w(px(360.0))
        .max_h(px(520.0))
        .rounded(radius())
        .border_1()
        .border_color(transparent())
        .bg(bg_overlay())
        .shadow(popover_shadow())
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
                                    .cursor_pointer()
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

fn sidebar_hidden_progress(hidden: bool, delta: f32) -> f32 {
    if hidden {
        delta
    } else {
        1.0 - delta
    }
}

fn lerp_px(from: f32, to: f32, progress: f32) -> Pixels {
    px(from + (to - from) * progress)
}

fn lerp_f32(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

fn build_static_tooltip(text: &'static str, cx: &mut App) -> AnyView {
    AnyView::from(cx.new(|_| ChromeTooltipView {
        text: SharedString::from(text),
    }))
}

struct ChromeTooltipView {
    text: SharedString,
}

impl Render for ChromeTooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(radius_sm())
            .border_1()
            .border_color(transparent())
            .bg(bg_overlay())
            .shadow(tooltip_shadow())
            .text_size(px(11.0))
            .text_color(fg_default())
            .child(self.text.clone())
    }
}

fn sidebar_icon_for_section(section: SectionId) -> LucideIcon {
    match section {
        SectionId::Overview => LucideIcon::LayoutDashboard,
        SectionId::Pulls => LucideIcon::GitPullRequest,
        SectionId::Reviews => LucideIcon::MessagesSquare,
        SectionId::Settings => LucideIcon::Settings,
        SectionId::Issues => LucideIcon::Inbox,
    }
}

fn theme_icon(preference: ThemePreference) -> LucideIcon {
    match preference {
        ThemePreference::System => LucideIcon::Monitor,
        ThemePreference::Light => LucideIcon::Sun,
        ThemePreference::Dark => LucideIcon::Moon,
    }
}

fn sidebar_nav_button(
    label: &str,
    icon: LucideIcon,
    count: i64,
    active: bool,
    collapsed: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "sidebar-nav-button-{label}-{}",
        usize::from(active)
    ));

    div()
        .h(px(38.0))
        .px(px(10.0))
        .when(collapsed, |el| el.px(px(0.0)))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .when(active, |el| el.bg(bg_emphasis()))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .justify_center()
                .when(!collapsed, |el| el.justify_start())
                .flex_grow()
                .min_w_0()
                .child(lucide_icon(
                    icon,
                    18.0,
                    if active { fg_emphasis() } else { fg_muted() },
                ))
                .when(!collapsed, |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if active { fg_emphasis() } else { fg_default() })
                            .child(label.to_string()),
                    )
                }),
        )
        .when(!collapsed && count > 0, |el| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .font_family(mono_font_family())
                    .text_color(if active { fg_default() } else { fg_subtle() })
                    .child(count.to_string()),
            )
        })
        .when(collapsed, |el| el.justify_center())
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
            },
        )
}

fn sidebar_theme_button(
    icon: LucideIcon,
    active: bool,
    collapsed: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "sidebar-theme-button-{}-{}",
        icon.unicode(),
        usize::from(active)
    ));

    div()
        .h(px(34.0))
        .when(collapsed, |el| el.w_full())
        .when(!collapsed, |el| el.flex_1())
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if active {
            bg_emphasis()
        } else {
            control_button_bg()
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(if active { bg_emphasis() } else { bg_selected() }))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            16.0,
            if active { fg_emphasis() } else { fg_muted() },
        ))
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(control_button_bg(), bg_emphasis(), progress))
            },
        )
}

fn sidebar_utility_button(
    icon: LucideIcon,
    active: bool,
    _bordered: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if active {
            bg_emphasis()
        } else {
            control_button_bg()
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(if active { bg_emphasis() } else { bg_selected() }))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            16.0,
            if active { fg_emphasis() } else { fg_muted() },
        ))
}

fn sidebar_action_button(
    icon: LucideIcon,
    label: &str,
    collapsed: bool,
    icon_color: Rgba,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(36.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(control_button_bg())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .when(!collapsed, |el| el.px(px(10.0)).justify_start())
        .when(collapsed, |el| el.w_full())
        .cursor_pointer()
        .hover(|style| style.bg(control_button_hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(icon, 16.0, icon_color))
        .when(!collapsed, |el| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg_default())
                    .child(label.to_string()),
            )
        })
}

fn pr_tab(
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
        .cursor_pointer()
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

fn compact_close_button(
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
        .cursor_pointer()
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
