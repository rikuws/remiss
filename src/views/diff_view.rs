use std::{
    cell::RefCell,
    collections::{hash_map::DefaultHasher, BTreeSet, VecDeque},
    hash::{Hash, Hasher},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::prelude::*;
use gpui::*;

use crate::code_display::{
    build_interactive_code_tokens, build_lsp_hover_tooltip_view, code_text_runs, mono_code_font,
    render_highlighted_code_block, render_highlighted_code_content, InteractiveCodeToken,
};
use crate::code_tour::{
    line_matches_diff_anchor, thread_matches_diff_anchor, CodeTourProvider, CodeTourProviderStatus,
    DiffAnchor, GeneratedCodeTour, TourSection, TourSectionCategory, TourSectionPriority, TourStep,
};
use crate::diff::{
    build_diff_render_rows, build_diff_render_rows_for_parsed_file, find_parsed_diff_file,
    find_parsed_diff_file_with_index, DiffLineKind, DiffRenderRow, ParsedDiffFile, ParsedDiffHunk,
    ParsedDiffLine,
};
use crate::difftastic::build_adapted_diff_highlights;
use crate::emoji::{emoji_shortcode_suggestions, EmojiSuggestion};
use crate::github::{
    PullRequestDetail, PullRequestFile, PullRequestReviewComment, PullRequestReviewThread,
    RepositoryFileContent, ReviewAction, REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT,
};
use crate::icons::{lucide_icon, LucideIcon};
use crate::inline_diff::{build_hunk_inline_emphasis, normalize_inline_emphasis_ranges};
use crate::local_documents;
use crate::local_repo;
use crate::lsp;
use crate::markdown::render_markdown;
use crate::onboarding::WizardStepTarget;
use crate::review_file_header::{
    render_review_file_header, render_review_file_header_with_controls, ReviewFileHeaderProps,
};
use crate::review_file_tree::{
    build_repository_file_tree_rows, build_review_file_tree_rows,
    ordered_review_files_from_tree_rows, review_file_tree_cache_scope, review_file_tree_totals,
};
use crate::review_queue::{build_review_queue, ReviewQueue, ReviewQueueBucket};
use crate::review_session::{
    DiffLayout, ReviewCenterMode, ReviewGuideLens, ReviewLocation, ReviewSourceTarget,
    GUIDED_REVIEW_PANEL_DEFAULT_WIDTH, GUIDED_REVIEW_PANEL_MAX_WIDTH,
    GUIDED_REVIEW_PANEL_MIN_WIDTH,
};
use crate::selectable_text::{AppTextFieldKind, AppTextInput, SelectableText};
use crate::semantic_diff::{build_semantic_diff_file, SemanticDiffFile, SemanticDiffSection};
use crate::source_browser::render_source_browser;
use crate::stacks::{
    discover_review_stack,
    model::{
        ChangeAtomId, Confidence, LayerDiffFilter, LayerMetrics, LayerReviewStatus, RepoContext,
        ReviewStack, ReviewStackLayer, StackDiffMode, StackDiscoveryOptions, StackKind,
        StackSource, StackWarning, VirtualLayerRef, STACK_GENERATOR_VERSION,
    },
};
use crate::state::*;
use crate::structural_diff::{
    build_and_cache_structural_diff, build_structural_diff_request, checkout_head_oid,
    structural_diff_warmup_request_key, structural_result_from_cached, StructuralDiffBuildResult,
    StructuralDiffRequest, StructuralDiffTerminalStatus,
};
use crate::structural_diff_cache::load_cached_structural_diff;
use crate::syntax::{self, SyntaxSpan};
use crate::temp_source_window::{
    open_temp_source_window_for_diff_target, temp_source_target_for_diff_line,
    temp_source_target_for_diff_side,
};
use crate::theme::*;
use crate::{github, notifications, review_intelligence};

use super::ai_tour::trigger_generate_tour;
use super::file_tree::{
    render_file_tree_directory_row, render_file_tree_file_row, render_file_tree_header,
    render_file_tree_state_message, render_structural_warmup_status, ReviewFileRowOpenHandler,
    ReviewFileRowOpenMode, REVIEW_FILE_TREE_ROW_HEIGHT,
};
use super::root::refresh_active_local_review;
use super::sections::{
    badge, badge_success, error_text, eyebrow, format_relative_time, ghost_button, nested_panel,
    panel_state_text, review_button, success_text, user_avatar,
};
use super::tooltips::{build_static_tooltip, build_text_tooltip};

mod ai_tour_panel;
mod combined_diff;
mod diff_metrics;
mod file_content;
mod guided_review;
mod review_comments;
mod review_sidebar;
mod side_by_side;
mod single_file_diff;
mod tour_diff_preview;

pub use self::file_content::{
    ensure_selected_file_content_loaded, load_local_source_file_content_flow,
    load_pull_request_file_content_flow, load_source_file_tree_flow, load_structural_diff_flow,
    load_temp_source_file_content_flow, warm_structural_diffs_flow,
};
pub use self::review_comments::{
    trigger_submit_inline_comment, trigger_submit_review_from_review_mode,
};

use self::ai_tour_panel::*;
use self::combined_diff::*;
use self::diff_metrics::*;
use self::guided_review::*;
use self::review_comments::{
    begin_review_line_drag, build_review_line_action_target, finish_review_line_drag,
    open_review_line_action, pending_review_comment_count, render_diff_open_source_icon,
    render_diff_waypoint_icon, render_finish_review_modal, render_review_line_action_overlay,
    render_review_thread, render_reviewable_diff_line, render_waypoint_pill,
    review_line_action_target_with_range, review_thread_ui_state, update_review_line_drag,
    ReviewThreadUiState,
};
use self::review_sidebar::{
    default_stack_layer, default_waymark_name, metric_pill, open_review_location_card,
    prepare_review_file_tree_rows, prepare_review_queue, prepare_review_stack,
    prepare_semantic_diff_file, render_review_sidebar_pane, reset_stack_timeline_list_state,
    stack_file_paths_for_filter,
};
use self::side_by_side::*;
use self::single_file_diff::*;
use self::tour_diff_preview::render_tour_diff_file_compact;
pub fn enter_files_surface(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.active_surface = PullRequestSurface::Files;
        s.pr_header_compact = false;
        s.set_review_file_tree_visible(true);

        s.ensure_active_selected_file_is_valid();

        s.persist_active_review_session();
        cx.notify();
    });

    ensure_active_review_focus_loaded(state, window, cx);
    ensure_active_stack_refs_loaded(state, window, cx);
    review_intelligence::trigger_review_intelligence(
        state,
        window,
        cx,
        review_intelligence::ReviewIntelligenceScope::All,
        false,
    );
    ensure_structural_diff_warmup_started(state, window, cx);
}

pub fn switch_review_code_mode(
    state: &Entity<AppState>,
    mode: ReviewCenterMode,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        state.set_review_center_mode_preserving_focus(mode);
        state.persist_active_review_session();
        cx.notify();
    });

    ensure_active_review_focus_loaded(state, window, cx);
}

pub fn enter_stack_review_mode(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let stack_defaults = {
        let app_state = state.read(cx);
        app_state.active_detail().map(|detail| {
            let stack = prepare_review_stack(&app_state, detail);
            let layer = default_stack_layer(stack.as_ref(), detail);
            let layer_id = layer.map(|layer| layer.id.clone());
            let layer_file = layer.and_then(|layer| {
                let belongs_to_current_pr = layer
                    .pr
                    .as_ref()
                    .map(|pr| pr.repository == detail.repository && pr.number == detail.number)
                    .unwrap_or(true);

                belongs_to_current_pr
                    .then(|| stack.first_file_for_layer(layer))
                    .flatten()
            });

            (layer_id, layer_file)
        })
    };

    state.update(cx, |state, cx| {
        state.active_surface = PullRequestSurface::Files;
        state.pr_header_compact = false;
        state.set_review_file_tree_visible(true);
        state.set_review_center_mode(ReviewCenterMode::GuidedReview);
        reset_stack_timeline_list_state(state);

        if let Some((layer_id, layer_file)) = stack_defaults.clone() {
            if let Some(session) = state.active_review_session_mut() {
                let has_existing_stack_choice = session.selected_stack_layer_id.is_some()
                    || session.stack_diff_mode != StackDiffMode::WholePr;

                if !has_existing_stack_choice {
                    session.selected_stack_layer_id = layer_id;
                    session.stack_diff_mode = StackDiffMode::CurrentLayerOnly;
                }
            }

            if state.selected_file_path.is_none() {
                state.selected_file_path = layer_file;
            }
        }

        state.ensure_active_selected_file_is_valid();
        state.persist_active_review_session();
        cx.notify();
    });

    ensure_active_review_focus_loaded(state, window, cx);
    ensure_active_stack_refs_loaded(state, window, cx);
    review_intelligence::trigger_review_intelligence(
        state,
        window,
        cx,
        review_intelligence::ReviewIntelligenceScope::StackOnly,
        false,
    );
}

pub fn open_review_diff_location(
    state: &Entity<AppState>,
    file_path: String,
    anchor: Option<DiffAnchor>,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        state.active_surface = PullRequestSurface::Files;
        state.navigate_to_review_location(
            ReviewLocation::from_diff(file_path.clone(), anchor),
            true,
        );
        state.persist_active_review_session();
        cx.notify();
    });

    ensure_active_review_focus_loaded(state, window, cx);
}

pub fn open_review_source_location(
    state: &Entity<AppState>,
    path: String,
    line: Option<usize>,
    reason: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        if state
            .active_detail()
            .map(crate::local_review::is_local_review_detail)
            .unwrap_or(false)
        {
            return;
        }
        state.active_surface = PullRequestSurface::Files;
        state.navigate_to_review_location(
            ReviewLocation::from_source(path.clone(), line, reason.clone()),
            true,
        );
        state.persist_active_review_session();
        cx.notify();
    });

    ensure_active_review_focus_loaded(state, window, cx);
}

pub fn ensure_active_review_focus_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let (center_mode, source_path) = {
        let app_state = state.read(cx);
        let Some(session) = app_state.active_review_session() else {
            return;
        };
        if session.center_mode == ReviewCenterMode::SourceBrowser {
            let source_path = session
                .source_target
                .as_ref()
                .map(|target| target.path.clone())
                .or_else(|| app_state.selected_file_path.clone())
                .or_else(|| {
                    app_state
                        .active_detail()
                        .and_then(|detail| detail.files.first().map(|file| file.path.clone()))
                });

            (session.center_mode, source_path)
        } else {
            (session.center_mode, None)
        }
    };

    if center_mode == ReviewCenterMode::SourceBrowser {
        ensure_source_file_tree_loaded(state, window, cx);
    }

    if let Some(source_path) = source_path {
        let model = state.clone();
        window
            .spawn(cx, async move |cx: &mut AsyncWindowContext| {
                load_local_source_file_content_flow(model, source_path, cx).await;
            })
            .detach();
    } else if center_mode == ReviewCenterMode::StructuralDiff {
        ensure_selected_structural_diff_loaded(state, window, cx);
        ensure_selected_file_content_loaded(state, window, cx);
    } else {
        ensure_selected_file_content_loaded(state, window, cx);
    }
}

pub fn ensure_selected_structural_diff_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            load_structural_diff_flow(model, None, cx).await;
        })
        .detach();
}

pub fn ensure_structural_diff_warmup_started(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            warm_structural_diffs_flow(model, cx).await;
        })
        .detach();
}

pub fn ensure_source_file_tree_loaded(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            load_source_file_tree_flow(model, cx).await;
        })
        .detach();
}

pub fn ensure_active_stack_refs_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let request = {
        let app_state = state.read(cx);
        let Some(detail) = app_state.active_detail() else {
            return;
        };
        if crate::local_review::is_local_review_detail(detail) {
            return;
        }
        let Some(detail_key) = app_state.active_pr_key.clone() else {
            return;
        };
        let detail_state = app_state.detail_states.get(&detail_key);
        if detail_state
            .map(|state| {
                state.stack_open_pull_requests.is_some()
                    || state.stack_open_pull_requests_loading
                    || state.stack_open_pull_requests_error.is_some()
            })
            .unwrap_or(false)
        {
            return;
        }
        (detail_key, detail.repository.clone())
    };

    let (detail_key, repository) = request;
    state.update(cx, |state, cx| {
        if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
            detail_state.stack_open_pull_requests_loading = true;
            detail_state.stack_open_pull_requests_error = None;
        }
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async move { github::fetch_open_pull_request_stack_refs(&repository) })
                .await;

            model
                .update(cx, |state, cx| {
                    if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                        detail_state.stack_open_pull_requests_loading = false;
                        match result {
                            Ok(open_prs) => {
                                detail_state.stack_open_pull_requests = Some(open_prs);
                                detail_state.stack_open_pull_requests_error = None;
                                state.review_stack_cache.borrow_mut().clear();
                            }
                            Err(error) => {
                                detail_state.stack_open_pull_requests_error = Some(error);
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn close_review_line_action(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.inline_comment_loading {
            return;
        }
        state.active_review_line_action = None;
        state.active_review_line_action_position = None;
        state.review_line_action_mode = ReviewLineActionMode::Menu;
        state.active_review_line_drag_origin = None;
        state.active_review_line_drag_current = None;
        state.inline_comment_draft.clear();
        state.inline_comment_preview = false;
        state.inline_comment_error = None;
        state.editing_review_comment_id = None;
        state.active_review_thread_reply_id = None;
        cx.notify();
    });
}

pub fn close_review_finish_modal(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.review_loading {
            return;
        }
        state.review_finish_modal_open = false;
        state.review_editor_preview = false;
        state.review_message = None;
        state.review_success = false;
        cx.notify();
    });
}

pub fn open_waypoint_spotlight(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.active_surface != PullRequestSurface::Files || state.active_pr_key.is_none() {
            return;
        }
        state.waypoint_spotlight_open = true;
        state.waypoint_spotlight_query.clear();
        state.waypoint_spotlight_selected_index = 0;
        state.active_review_line_action = None;
        state.active_review_line_action_position = None;
        state.review_line_action_mode = ReviewLineActionMode::Menu;
        state.active_review_line_drag_origin = None;
        state.active_review_line_drag_current = None;
        state.inline_comment_error = None;
        cx.notify();
    });
}

pub fn toggle_waypoint_spotlight(state: &Entity<AppState>, cx: &mut App) {
    let is_open = state.read(cx).waypoint_spotlight_open;
    if is_open {
        close_waypoint_spotlight(state, cx);
    } else {
        open_waypoint_spotlight(state, cx);
    }
}

pub fn close_waypoint_spotlight(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |state, cx| {
        state.waypoint_spotlight_open = false;
        state.waypoint_spotlight_query.clear();
        state.waypoint_spotlight_selected_index = 0;
        cx.notify();
    });
}

pub fn move_waypoint_spotlight_selection(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    state.update(cx, |state, cx| {
        if !state.waypoint_spotlight_open {
            return;
        }

        let item_count = filtered_waypoint_spotlight_items(state).len();
        if item_count == 0 {
            state.waypoint_spotlight_selected_index = 0;
            cx.notify();
            return;
        }

        let max_index = item_count.saturating_sub(1) as isize;
        let next =
            (state.waypoint_spotlight_selected_index as isize + delta).clamp(0, max_index) as usize;
        if next != state.waypoint_spotlight_selected_index {
            state.waypoint_spotlight_selected_index = next;
            cx.notify();
        }
    });
}

pub fn execute_waypoint_spotlight_selection(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let item = {
        let app_state = state.read(cx);
        let items = filtered_waypoint_spotlight_items(&app_state);
        let selected_index = app_state
            .waypoint_spotlight_selected_index
            .min(items.len().saturating_sub(1));
        items.get(selected_index).cloned()
    };

    let Some(waymark) = item else {
        return;
    };

    close_waypoint_spotlight(state, cx);
    open_review_location_card(state, &waymark.location, window, cx);
}

pub fn trigger_add_waypoint_shortcut(state: &Entity<AppState>, cx: &mut App) {
    let waypoint_name = {
        let app_state = state.read(cx);
        if app_state.active_surface != PullRequestSurface::Files
            || app_state.selected_diff_line_target().is_none()
        {
            return;
        }

        default_waymark_name(
            app_state.selected_file_path.as_deref(),
            None,
            app_state.selected_diff_anchor.as_ref(),
        )
    };

    state.update(cx, |state, cx| {
        if state.selected_diff_line_target().is_none() {
            return;
        }
        state.add_waymark_for_current_review_location(waypoint_name.clone());
        state.persist_active_review_session();
        cx.notify();
    });
}

fn filtered_waypoint_spotlight_items(
    state: &AppState,
) -> Vec<crate::review_session::ReviewWaymark> {
    let mut items = state
        .active_review_session()
        .map(|session| session.waymarks.clone())
        .unwrap_or_default();
    items.reverse();

    let query = state.waypoint_spotlight_query.trim().to_lowercase();
    if query.is_empty() {
        return items;
    }

    items
        .into_iter()
        .filter(|waymark| {
            let haystack = format!(
                "{} {} {}",
                waymark.name, waymark.location.label, waymark.location.file_path
            )
            .to_lowercase();
            haystack.contains(&query)
        })
        .collect()
}

fn render_waypoint_spotlight(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let app_state = state.read(cx);
    let query = app_state.waypoint_spotlight_query.clone();
    let filtered = filtered_waypoint_spotlight_items(&app_state);
    let selected_index = app_state
        .waypoint_spotlight_selected_index
        .min(filtered.len().saturating_sub(1));
    let state_for_backdrop = state.clone();

    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .justify_center()
        .pt(px(88.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    close_waypoint_spotlight(&state_for_backdrop, cx);
                }),
        )
        .child(
            div()
                .relative()
                .w(px(680.0))
                .max_h(px(620.0))
                .rounded(radius_lg())
                .border_1()
                .border_color(transparent())
                .bg(bg_overlay())
                .shadow(dialog_shadow())
                .occlude()
                .overflow_hidden()
                .child(
                    div()
                        .px(px(24.0))
                        .py(px(20.0))
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .child(render_waypoint_pill("Waypoint Spotlight", true))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child("Jump between saved review stops"),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .child(badge("cmd-j"))
                                        .child(badge("cmd-shift-j")),
                                ),
                        )
                        .child(
                            div()
                                .px(px(16.0))
                                .py(px(14.0))
                                .rounded(radius())
                                .border_1()
                                .border_color(transparent())
                                .bg(bg_surface())
                                .text_size(px(15.0))
                                .text_color(if query.is_empty() {
                                    fg_subtle()
                                } else {
                                    fg_emphasis()
                                })
                                .child(
                                    AppTextInput::new(
                                        "waypoint-spotlight-query",
                                        state.clone(),
                                        AppTextFieldKind::WaypointSpotlightQuery,
                                        "Search waypoints by name, file, or line",
                                    )
                                    .autofocus(true),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(format!("{} waypoints", filtered.len())),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child("↑↓ move")
                                        .child("•")
                                        .child("enter open"),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .id("waypoint-spotlight-scroll")
                        .overflow_y_scroll()
                        .max_h(px(430.0))
                        .when(filtered.is_empty(), |el| {
                            el.child(
                                div()
                                    .px(px(20.0))
                                    .pb(px(18.0))
                                    .child(panel_state_text(
                                        "No waypoints yet. Click a diff line, choose Add waypoint, or press cmd-shift-j on a selected line.",
                                    )),
                            )
                        })
                        .children(filtered.into_iter().enumerate().map(|(ix, waymark)| {
                            render_waypoint_spotlight_row(
                                state,
                                &waymark,
                                ix == selected_index,
                            )
                        })),
                )
                .with_animation(
                    "waypoint-spotlight",
                    Animation::new(Duration::from_millis(160)).with_easing(ease_in_out),
                    move |el, delta| {
                        el.mt(lerp_px(10.0, 0.0, delta))
                            .bg(lerp_rgba(bg_canvas(), bg_overlay(), delta))
                    },
                ),
        )
}

fn render_waypoint_spotlight_row(
    state: &Entity<AppState>,
    waymark: &crate::review_session::ReviewWaymark,
    selected: bool,
) -> impl IntoElement {
    let location = waymark.location.clone();
    let state = state.clone();

    div()
        .px(px(20.0))
        .py(px(13.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .bg(if selected {
            bg_emphasis()
        } else {
            bg_overlay()
        })
        .cursor_pointer()
        .hover(move |style| {
            style.bg(if selected {
                bg_emphasis()
            } else {
                bg_selected()
            })
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            close_waypoint_spotlight(&state, cx);
            open_review_location_card(&state, &location, window, cx);
        })
        .child(render_waypoint_pill(&waymark.name, selected))
        .child(
            div()
                .mt(px(8.0))
                .text_size(px(12.0))
                .text_color(fg_emphasis())
                .child(waymark.location.label.clone()),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(11.0))
                .text_color(fg_muted())
                .child(waymark.location.mode.label()),
        )
}

pub fn render_files_view(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();

    let Some(detail) = detail else {
        return div()
            .child(panel_state_text("No detail data available."))
            .into_any_element();
    };

    let files = &detail.files;
    let selected_anchor = s.selected_diff_anchor.clone();
    let waypoint_spotlight_open = s.waypoint_spotlight_open;
    let line_action_target = s.active_review_line_action.clone();
    let line_action_position = s.active_review_line_action_position;
    let line_action_mode = s.review_line_action_mode.clone();
    let review_finish_modal_open = s.review_finish_modal_open;
    let is_local_review = crate::local_review::is_local_review_detail(detail);
    let review_stack = prepare_review_stack(&s, detail);
    let review_queue = prepare_review_queue(&s, detail);
    let review_session = s.active_review_session().cloned().unwrap_or_default();
    let show_file_tree = review_session.show_file_tree;
    let file_tree_hidden = !show_file_tree;
    let file_tree_animation_key = ("review-file-tree", usize::from(file_tree_hidden));

    let default_path = review_queue
        .default_item()
        .map(|item| item.file_path.clone())
        .or_else(|| detail.parsed_diff.first().map(|file| file.path.clone()));
    let selected_path = s
        .selected_file_path
        .as_ref()
        .filter(|path| files.iter().any(|file| file.path == **path))
        .cloned()
        .or(default_path);
    let selected_path = selected_path.as_deref();
    let sidebar_selected_path = if review_session.center_mode == ReviewCenterMode::SourceBrowser {
        review_session
            .source_target
            .as_ref()
            .map(|target| target.path.as_str())
            .or(selected_path)
    } else {
        selected_path
    };
    let selected_file = selected_path.and_then(|path| files.iter().find(|file| file.path == path));
    let semantic_file = selected_file.map(|file| prepare_semantic_diff_file(&s, detail, file));

    div()
        .relative()
        .flex()
        .flex_grow()
        .min_h_0()
        .bg(diff_editor_bg())
        .child(
            div()
                .w(if show_file_tree {
                    file_tree_width()
                } else {
                    px(0.0)
                })
                .h_full()
                .flex_shrink_0()
                .min_h_0()
                .flex()
                .flex_row()
                .overflow_hidden()
                .child(render_review_sidebar_pane(
                    state,
                    detail,
                    review_queue.as_ref(),
                    sidebar_selected_path,
                    semantic_file.as_deref(),
                    &review_session,
                    review_stack.clone(),
                    cx,
                ))
                .with_animation(
                    file_tree_animation_key,
                    Animation::new(Duration::from_millis(REVIEW_FILE_TREE_ANIMATION_MS))
                        .with_easing(ease_in_out),
                    move |el, delta| {
                        let progress = review_file_tree_hidden_progress(file_tree_hidden, delta);
                        let expanded_width = file_tree_width();
                        let hidden_width = px(0.0);
                        el.w(expanded_width + (hidden_width - expanded_width) * progress)
                    },
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow()
                .min_w_0()
                .min_h_0()
                .child(render_diff_panel(
                    state,
                    &s,
                    detail,
                    selected_path,
                    selected_anchor.as_ref(),
                    review_stack.clone(),
                    window,
                    cx,
                )),
        )
        .when(waypoint_spotlight_open, |el| {
            el.child(render_waypoint_spotlight(state, cx))
        })
        .when(review_finish_modal_open && !is_local_review, |el| {
            el.child(render_finish_review_modal(state, detail, cx))
        })
        .when_some(
            (!is_local_review)
                .then(|| {
                    line_action_target
                        .as_ref()
                        .zip(line_action_position)
                        .map(|(target, position)| (target.clone(), position))
                })
                .flatten(),
            |el, (target, position)| {
                el.child(render_review_line_action_overlay(
                    state,
                    &target,
                    position,
                    line_action_mode.clone(),
                    cx,
                ))
            },
        )
        .into_any_element()
}

const REVIEW_FILE_TREE_ANIMATION_MS: u64 = 220;

fn review_file_tree_hidden_progress(hidden: bool, delta: f32) -> f32 {
    if hidden {
        delta
    } else {
        1.0 - delta
    }
}

fn review_cache_key(active_pr_key: Option<&str>, scope: &str) -> String {
    format!("{}:{scope}", active_pr_key.unwrap_or("detached"))
}

fn reset_list_state_preserving_scroll(list_state: &ListState, item_count: usize) {
    if list_state.item_count() == item_count {
        return;
    }

    let scroll_top = list_state.logical_scroll_top();
    list_state.reset(item_count);
    list_state.scroll_to(scroll_top);
}

const DIFF_CONTENT_LEFT_GUTTER: f32 = 24.0;
const DIFF_CONTENT_RIGHT_GUTTER: f32 = DIFF_CONTENT_LEFT_GUTTER;
const DIFF_SECTION_LEFT_MARGIN: f32 = 0.0;
const DIFF_SECTION_RIGHT_MARGIN: f32 = 0.0;
const DIFF_SECTION_BODY_INSET: f32 = 12.0;
const DIFF_SECTION_BODY_LEFT_MARGIN: f32 = DIFF_SECTION_LEFT_MARGIN + DIFF_SECTION_BODY_INSET;
const DIFF_SECTION_BODY_RIGHT_MARGIN: f32 = DIFF_SECTION_RIGHT_MARGIN + DIFF_SECTION_BODY_INSET;
const DIFF_SECTION_HEADER_OVERHANG: f32 = DIFF_SECTION_BODY_INSET;
const DIFF_SECTION_HEADER_LEFT_MARGIN: f32 =
    DIFF_SECTION_BODY_LEFT_MARGIN - DIFF_SECTION_HEADER_OVERHANG;
const DIFF_SECTION_HEADER_RIGHT_MARGIN: f32 =
    DIFF_SECTION_BODY_RIGHT_MARGIN - DIFF_SECTION_HEADER_OVERHANG;
const DIFF_FILE_HEADER_TOP_MARGIN_FIRST: f32 = 14.0;
const DIFF_FILE_HEADER_TOP_MARGIN: f32 = 36.0;
const DIFF_FILE_HEADER_BOTTOM_MARGIN: f32 = 10.0;
const DIFF_FLOATING_FILE_HEADER_TOP_PADDING: f32 = 10.0;
const DIFF_FLOATING_FILE_HEADER_BOTTOM_PADDING: f32 = 10.0;
const DIFF_SCROLL_TOP_FADE_HEIGHT: f32 = 30.0;
const DIFF_SCROLLBAR_WIDTH: f32 = 8.0;

fn render_diff_panel(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
    review_stack: Arc<ReviewStack>,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let files = &detail.files;
    let selected_file = selected_path
        .and_then(|p| files.iter().find(|f| f.path == p))
        .or(files.first());

    let (total_additions, total_deletions) = files.iter().fold((0i64, 0i64), |acc, file| {
        (acc.0 + file.additions, acc.1 + file.deletions)
    });
    let local_repo_status = app_state
        .active_detail_state()
        .and_then(|detail_state| detail_state.local_repository_status.as_ref());
    let local_repo_loading = app_state
        .active_detail_state()
        .map(|detail_state| detail_state.local_repository_loading)
        .unwrap_or(false);
    let local_repo_error = app_state
        .active_detail_state()
        .and_then(|detail_state| detail_state.local_repository_error.as_deref());
    let review_session = app_state
        .active_review_session()
        .cloned()
        .unwrap_or_default();
    let center_mode = match review_session.center_mode {
        ReviewCenterMode::AiTour | ReviewCenterMode::Stack => ReviewCenterMode::GuidedReview,
        mode => mode,
    };
    let normal_diff_layout = review_session.normal_diff_layout;
    let structural_diff_layout = review_session.structural_diff_layout;
    let guided_review_lens = review_session.guided_review_lens;
    let active_diff_layout = match center_mode {
        ReviewCenterMode::StructuralDiff => structural_diff_layout,
        ReviewCenterMode::GuidedReview if guided_review_lens == ReviewGuideLens::Structural => {
            structural_diff_layout
        }
        _ => normal_diff_layout,
    };
    let stack_filter = matches!(
        center_mode,
        ReviewCenterMode::GuidedReview | ReviewCenterMode::Stack
    )
    .then(|| {
        build_layer_diff_filter(
            review_stack.as_ref(),
            review_session.stack_diff_mode,
            review_session.selected_stack_layer_id.as_deref(),
            &review_session.reviewed_stack_atom_ids,
        )
    })
    .flatten();
    let has_textual_diff = detail
        .parsed_diff
        .iter()
        .any(|parsed| !parsed.is_binary && !parsed.hunks.is_empty());
    let source_target = review_session.source_target.clone().or_else(|| {
        selected_file.map(|file| ReviewSourceTarget {
            path: file.path.clone(),
            line: selected_anchor
                .and_then(|anchor| anchor.line)
                .and_then(|line| usize::try_from(line).ok())
                .filter(|line| *line > 0),
            reason: Some("Current review focus".to_string()),
        })
    });
    let source_parsed = source_target
        .as_ref()
        .and_then(|target| find_parsed_diff_file(&detail.parsed_diff, &target.path));
    let structural_warmup_status = (center_mode == ReviewCenterMode::StructuralDiff
        || (center_mode == ReviewCenterMode::GuidedReview
            && guided_review_lens == ReviewGuideLens::Structural))
        .then(|| {
            app_state
                .active_detail_state()
                .and_then(|detail_state| detail_state.structural_diff_warmup.status_text())
        })
        .flatten();

    div()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .bg(diff_editor_bg())
        .child(render_diff_toolbar(
            state,
            detail,
            files.len(),
            total_additions,
            total_deletions,
            local_repo_status,
            local_repo_loading,
            local_repo_error,
            structural_warmup_status,
            center_mode,
            active_diff_layout,
            (center_mode == ReviewCenterMode::GuidedReview).then_some(guided_review_lens),
            !has_textual_diff,
            app_state.is_onboarding_target(WizardStepTarget::ReviewFeedback),
        ))
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .bg(diff_editor_bg())
                .flex()
                .flex_col()
                .child(
                    if crate::local_review::is_local_review_detail(detail) && files.is_empty() {
                        render_local_review_empty_state(
                            state,
                            detail,
                            local_repo_status,
                            local_repo_loading,
                            local_repo_error,
                        )
                        .into_any_element()
                    } else if center_mode == ReviewCenterMode::SourceBrowser {
                        source_target
                            .as_ref()
                            .map(|target| render_source_browser(state, target, source_parsed, cx))
                            .unwrap_or_else(|| {
                                panel_state_text(
                                    "Select a file or definition to open the source browser.",
                                )
                                .into_any_element()
                            })
                    } else if center_mode == ReviewCenterMode::GuidedReview {
                        render_guided_review_view(
                            state,
                            app_state,
                            detail,
                            selected_path,
                            selected_anchor,
                            review_stack.clone(),
                            stack_filter.clone(),
                            guided_review_lens,
                            normal_diff_layout,
                            structural_diff_layout,
                            window,
                            cx,
                        )
                    } else if center_mode == ReviewCenterMode::StructuralDiff {
                        render_combined_diff_files(
                            state,
                            app_state,
                            detail,
                            selected_path,
                            selected_anchor,
                            review_stack.clone(),
                            None,
                            center_mode,
                            structural_diff_layout,
                            cx,
                        )
                        .into_any_element()
                    } else {
                        render_combined_diff_files(
                            state,
                            app_state,
                            detail,
                            selected_path,
                            selected_anchor,
                            review_stack.clone(),
                            stack_filter.clone(),
                            center_mode,
                            normal_diff_layout,
                            cx,
                        )
                        .into_any_element()
                    },
                ),
        )
}

fn render_diff_toolbar(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    total_files: usize,
    total_additions: i64,
    total_deletions: i64,
    local_repo_status: Option<&local_repo::LocalRepositoryStatus>,
    local_repo_loading: bool,
    _local_repo_error: Option<&str>,
    structural_warmup_status: Option<String>,
    center_mode: ReviewCenterMode,
    active_diff_layout: DiffLayout,
    guided_review_lens: Option<ReviewGuideLens>,
    layout_toggle_disabled: bool,
    highlight_review_feedback: bool,
) -> impl IntoElement {
    let mut focus_meta = Vec::new();
    focus_meta.push(format!("+{total_additions} / -{total_deletions}"));
    if local_repo_loading {
        focus_meta.push("preparing checkout".to_string());
    } else if let Some(status) = local_repo_status.filter(|status| !status.ready_for_local_features)
    {
        focus_meta.push(if !status.is_valid_repository {
            "checkout needs repair".to_string()
        } else if !status.matches_expected_head {
            "checkout needs sync".to_string()
        } else if !status.is_worktree_clean {
            "checkout is dirty".to_string()
        } else {
            "checkout pending".to_string()
        });
    }
    if let Some(status) = structural_warmup_status {
        focus_meta.push(status);
    }
    let focus_summary = focus_meta.join(" / ");
    let show_layout_toggle = matches!(
        center_mode,
        ReviewCenterMode::SemanticDiff
            | ReviewCenterMode::StructuralDiff
            | ReviewCenterMode::GuidedReview
            | ReviewCenterMode::Stack
    );
    let is_local_review = crate::local_review::is_local_review_detail(detail);
    let state_for_refresh = state.clone();
    let state_for_submit = state.clone();
    let pending_count = pending_review_comment_count(detail);
    let submit_label = if pending_count > 0 {
        format!("Submit review ({pending_count})")
    } else {
        "Submit review".to_string()
    };

    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(20.0))
        .py(px(12.0))
        .bg(diff_editor_surface())
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .flex_grow()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_family(mono_font_family())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(format!("{total_files} files changed")),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_family(mono_font_family())
                        .text_color(fg_muted())
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(focus_summary),
                ),
        )
        .when_some(guided_review_lens, |el, lens| {
            el.child(render_guided_review_lens_toggle(state, lens))
        })
        .when(show_layout_toggle, |el| {
            let layout_center_mode = if guided_review_lens == Some(ReviewGuideLens::Structural) {
                ReviewCenterMode::StructuralDiff
            } else {
                center_mode
            };
            el.child(render_diff_layout_toggle(
                state,
                layout_center_mode,
                active_diff_layout,
                layout_toggle_disabled,
            ))
        })
        .when(!is_local_review, |el| {
            el.child(diff_toolbar_primary_button(
                &submit_label,
                highlight_review_feedback,
                move |_, _, cx| {
                    state_for_submit.update(cx, |state, cx| {
                        state.review_finish_modal_open = true;
                        state.review_editor_active = true;
                        state.review_message = None;
                        state.review_success = false;
                        cx.notify();
                    });
                },
            ))
        })
        .when(is_local_review, |el| {
            el.child(review_button("Refresh", move |_, window, cx| {
                refresh_active_local_review(&state_for_refresh, window, cx);
            }))
        })
}

fn diff_toolbar_primary_button(
    label: &str,
    highlighted: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .rounded(px(8.0))
        .border_1()
        .border_color(transparent())
        .bg(if highlighted {
            with_alpha(focus_border(), 0.12)
        } else {
            transparent()
        })
        .p(px(2.0))
        .child(
            div()
                .px(px(12.0))
                .py(px(6.0))
                .rounded(radius_sm())
                .bg(primary_action_bg())
                .text_color(fg_on_primary_action())
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .hover(|style| style.bg(primary_action_hover()))
                .on_mouse_down(MouseButton::Left, on_click)
                .child(label.to_string()),
        )
}

fn toolbar_icon_button(
    id: &'static str,
    tooltip: &'static str,
    active: bool,
    disabled: bool,
    icon: AnyElement,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id =
        SharedString::from(format!("toolbar-icon-button-{id}-{}", usize::from(active)));

    div()
        .id(id)
        .w(px(22.0))
        .h(px(22.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if active { bg_emphasis() } else { transparent() })
        .opacity(if disabled { 0.42 } else { 1.0 })
        .flex()
        .items_center()
        .justify_center()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .when(!disabled, move |el| {
            el.cursor_pointer()
                .hover(move |style| style.bg(if active { bg_emphasis() } else { bg_selected() }))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(icon)
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
            },
        )
}

fn render_stack_tree_toggle_icon(active: bool) -> AnyElement {
    let color = if active { accent() } else { fg_muted() };

    lucide_icon(LucideIcon::ListTree, 14.0, color).into_any_element()
}

fn workspace_mode_button(
    label: &str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "workspace-mode-button-{label}-{}",
        usize::from(active)
    ));

    div()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if active { bg_emphasis() } else { transparent() })
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if active { bg_emphasis() } else { bg_selected() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
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

fn find_threads_for_line<'a>(
    file_path: &str,
    line: &ParsedDiffLine,
    threads: &'a [&PullRequestReviewThread],
) -> Vec<&'a PullRequestReviewThread> {
    threads
        .iter()
        .copied()
        .filter(|t| {
            if t.path != file_path {
                return false;
            }
            match line.kind {
                DiffLineKind::Addition | DiffLineKind::Context => {
                    let line_no = line.right_line_number;
                    if t.diff_side == "RIGHT" {
                        t.line == line_no || t.original_line == line_no
                    } else {
                        false
                    }
                }
                DiffLineKind::Deletion => {
                    let line_no = line.left_line_number;
                    if t.diff_side == "LEFT" {
                        t.line == line_no || t.original_line == line_no
                    } else {
                        false
                    }
                }
                DiffLineKind::Meta => false,
            }
        })
        .collect()
}

fn label_for_change_type(change_type: &str) -> &str {
    match change_type {
        "ADDED" => "added",
        "DELETED" => "deleted",
        "RENAMED" => "renamed",
        "COPIED" => "copied",
        _ => "modified",
    }
}

#[cfg(test)]
mod tests {
    use crate::diff::parse_unified_diff;
    use crate::state::StructuralDiffFileState;
    use gpui::{point, px, size, Bounds, ListAlignment, ListOffset, ListState, Pixels};

    use super::file_content::{
        should_apply_structural_diff_update, should_reuse_structural_diff_state,
        structural_diff_state_terminal_status,
    };
    use super::review_sidebar::sync_stack_timeline_item_count;
    use super::{
        build_normal_side_by_side_diff_file, focus_item_index_around,
        max_side_by_side_column_widths, reading_focus_item_index, DiffFileCollapseScrollAdjustment,
        SideBySideColumnWidths, StructuralDiffTerminalStatus,
    };

    fn test_bounds(top: f32, bottom: f32) -> Bounds<Pixels> {
        Bounds::new(
            point(px(0.0), px(top)),
            size(px(100.0), px((bottom - top).max(0.0))),
        )
    }

    #[test]
    fn selected_structural_load_reuses_warmup_loading_state() {
        let state = StructuralDiffFileState {
            request_key: Some("structural-diff-v1:repo:1:head:MODIFIED::src/lib.rs".to_string()),
            loading: true,
            ..StructuralDiffFileState::default()
        };

        assert!(should_reuse_structural_diff_state(
            &state,
            "structural-diff-v1:repo:1:head:MODIFIED::src/lib.rs"
        ));
        assert_eq!(
            structural_diff_state_terminal_status(
                &state,
                "structural-diff-v1:repo:1:head:MODIFIED::src/lib.rs"
            ),
            None
        );
    }

    #[test]
    fn selected_structural_load_reuses_cached_terminal_error_state() {
        let state = StructuralDiffFileState {
            request_key: Some("structural-diff-v1:repo:1:head:MODIFIED::image.png".to_string()),
            error: Some("Structural diff is not available for binary file image.png.".to_string()),
            terminal_error: true,
            ..StructuralDiffFileState::default()
        };

        assert!(should_reuse_structural_diff_state(
            &state,
            "structural-diff-v1:repo:1:head:MODIFIED::image.png"
        ));
        assert_eq!(
            structural_diff_state_terminal_status(
                &state,
                "structural-diff-v1:repo:1:head:MODIFIED::image.png"
            ),
            Some(StructuralDiffTerminalStatus::Error)
        );
    }

    #[test]
    fn stale_structural_diff_results_do_not_apply_after_pr_switch() {
        assert!(should_apply_structural_diff_update(
            Some("acme/widgets#42"),
            "acme/widgets#42",
            Some("request-a"),
            "request-a",
        ));
        assert!(!should_apply_structural_diff_update(
            Some("acme/widgets#43"),
            "acme/widgets#42",
            Some("request-a"),
            "request-a",
        ));
        assert!(!should_apply_structural_diff_update(
            Some("acme/widgets#42"),
            "acme/widgets#42",
            Some("request-old"),
            "request-a",
        ));
    }

    #[test]
    fn stack_timeline_initializes_at_base_branch_row() {
        let list_state = ListState::new(0, ListAlignment::Top, px(36.0));

        sync_stack_timeline_item_count(&list_state, 5);

        let scroll_top = list_state.logical_scroll_top();
        assert_eq!(scroll_top.item_ix, 5);
        assert_eq!(scroll_top.offset_in_item, px(0.0));
    }

    #[test]
    fn stack_timeline_preserves_manual_scroll_after_count_change() {
        let list_state = ListState::new(5, ListAlignment::Top, px(36.0));
        list_state.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(7.0),
        });

        sync_stack_timeline_item_count(&list_state, 7);

        let scroll_top = list_state.logical_scroll_top();
        assert_eq!(scroll_top.item_ix, 2);
        assert_eq!(scroll_top.offset_in_item, px(7.0));
    }

    #[test]
    fn reading_focus_uses_upper_third_changed_row() {
        let item_bounds = [
            test_bounds(0.0, 32.0),
            test_bounds(32.0, 60.0),
            test_bounds(60.0, 112.0),
            test_bounds(112.0, 140.0),
            test_bounds(140.0, 168.0),
        ];

        let selected = reading_focus_item_index(
            item_bounds.len(),
            0..item_bounds.len(),
            test_bounds(0.0, 300.0),
            |ix| item_bounds.get(ix).cloned(),
            |ix| matches!(ix, 1 | 3 | 4),
        );

        assert_eq!(selected, Some(3));
    }

    #[test]
    fn reading_focus_skips_headers_and_gaps() {
        let item_bounds = [
            test_bounds(0.0, 120.0),
            test_bounds(120.0, 148.0),
            test_bounds(148.0, 176.0),
        ];

        let selected = reading_focus_item_index(
            item_bounds.len(),
            0..item_bounds.len(),
            test_bounds(0.0, 300.0),
            |ix| item_bounds.get(ix).cloned(),
            |ix| ix == 1,
        );

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn reading_focus_falls_back_when_bounds_are_unavailable() {
        let selected =
            reading_focus_item_index(4, 0..4, test_bounds(0.0, 300.0), |_| None, |_| true);

        assert_eq!(selected, None);
        assert_eq!(focus_item_index_around(4, 2, |ix| ix == 3), Some(3));
        assert_eq!(focus_item_index_around(4, 2, |ix| ix == 1), Some(1));
    }

    #[test]
    fn collapsing_combined_file_body_pins_scroll_inside_body_to_header() {
        let list_state = ListState::new(12, ListAlignment::Top, px(400.0));
        list_state.scroll_to(ListOffset {
            item_ix: 4,
            offset_in_item: px(7.0),
        });

        DiffFileCollapseScrollAdjustment {
            list_state: list_state.clone(),
            header_item_ix: 2,
            expanded_extra_item_count: 5,
        }
        .apply_for_toggle(false);

        assert_eq!(list_state.item_count(), 7);
        let scroll_top = list_state.logical_scroll_top();
        assert_eq!(scroll_top.item_ix, 2);
        assert_eq!(scroll_top.offset_in_item, px(0.0));
    }

    #[test]
    fn collapsing_combined_file_body_preserves_scroll_after_body() {
        let list_state = ListState::new(12, ListAlignment::Top, px(400.0));
        list_state.scroll_to(ListOffset {
            item_ix: 9,
            offset_in_item: px(7.0),
        });

        DiffFileCollapseScrollAdjustment {
            list_state: list_state.clone(),
            header_item_ix: 2,
            expanded_extra_item_count: 5,
        }
        .apply_for_toggle(false);

        assert_eq!(list_state.item_count(), 7);
        let scroll_top = list_state.logical_scroll_top();
        assert_eq!(scroll_top.item_ix, 4);
        assert_eq!(scroll_top.offset_in_item, px(7.0));
    }

    #[test]
    fn combined_side_by_side_widths_use_widest_visible_content() {
        let widths = max_side_by_side_column_widths(
            [
                SideBySideColumnWidths {
                    left: 320.0,
                    right: 480.0,
                },
                SideBySideColumnWidths {
                    left: 640.0,
                    right: 360.0,
                },
            ]
            .into_iter(),
        )
        .expect("combined widths");

        assert_eq!(widths.left, 640.0);
        assert_eq!(widths.right, 480.0);
    }

    #[test]
    fn normal_side_by_side_pairs_changed_lines() {
        let parsed = parse_unified_diff(
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,3 +1,3 @@\n\
              fn main() {\n\
             -    let value = 1;\n\
             +    let value = 2;\n\
              }\n",
        );
        let side_by_side = build_normal_side_by_side_diff_file(&parsed[0]);
        let rows = &side_by_side.hunks[0].rows;

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].left_line_index, Some(0));
        assert_eq!(rows[0].right_line_index, Some(0));
        assert_eq!(rows[1].left_line_index, Some(1));
        assert_eq!(rows[1].right_line_index, Some(2));
        assert!(side_by_side.line_map[0][1].unwrap().primary);
        assert!(!side_by_side.line_map[0][2].unwrap().primary);
    }
}
