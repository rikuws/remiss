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
    render_highlighted_code_content, InteractiveCodeToken,
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
use crate::managed_lsp::{self, ManagedServerInstallState};
use crate::markdown::render_markdown;
use crate::onboarding::WizardStepTarget;
use crate::review_ai::DiffAnchor;
use crate::review_anchors::{line_matches_diff_anchor, review_thread_anchor};
use crate::review_file_header::{render_review_file_header_with_controls, ReviewFileHeaderProps};
use crate::review_file_tree::{
    build_repository_file_tree_rows, build_review_file_tree_rows,
    ordered_review_files_from_tree_rows, review_file_tree_cache_scope, review_file_tree_totals,
};
use crate::review_queue::{build_review_queue, ReviewQueue, ReviewQueueBucket};
use crate::review_session::{
    location_label, DiffLayout, ReviewCenterMode, ReviewGuideLens, ReviewLocation,
    ReviewSourceTarget, GUIDED_REVIEW_PANEL_DEFAULT_WIDTH, GUIDED_REVIEW_PANEL_MAX_WIDTH,
    GUIDED_REVIEW_PANEL_MIN_WIDTH,
};
use crate::selectable_text::{AppTextFieldKind, AppTextInput, SelectableText};
use crate::semantic_diff::{build_semantic_diff_file, SemanticDiffFile, SemanticDiffSection};
use crate::shortcuts;
use crate::source_browser::render_source_browser;
use crate::stacks::{
    discover_review_stack,
    model::{
        normalize_stack_layer_title, ChangeAtomId, Confidence, LayerDiffFilter, LayerMetrics,
        LayerReviewStatus, RepoContext, ReviewStack, ReviewStackLayer, StackDiffMode,
        StackDiscoveryOptions, StackKind, StackSource, StackWarning, VirtualLayerRef,
        STACK_GENERATOR_VERSION,
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
use crate::vim::{diff::VimDiffOutcome, ReadOnlyVimMode, VimIntent, VimKey};
use crate::{github, notifications, review_intelligence};

use super::corner_mask::{render_corner_mask, CornerMask};
use super::file_chooser::render_file_chooser;
use super::file_tree::{
    render_file_tree_directory_row, render_file_tree_file_row, render_file_tree_header,
    render_file_tree_state_message, render_structural_warmup_status, ReviewFileRowOpenHandler,
    ReviewFileRowOpenMode, REVIEW_FILE_TREE_ROW_HEIGHT,
};
use super::motion::{lerp_px, lerp_rgba};
use super::root::refresh_active_local_review;
use super::sections::{
    badge, badge_success, error_text, eyebrow, format_relative_time, ghost_button, nested_panel,
    panel_state_text, review_button, success_text, user_avatar,
};
use super::tooltips::{build_static_tooltip, build_text_tooltip};

mod combined_diff;
mod diff_metrics;
mod file_content;
mod guided_review;
mod review_comments;
mod review_sidebar;
mod side_by_side;
mod single_file_diff;

pub use self::file_content::{
    ensure_selected_file_content_loaded, load_local_source_file_content_flow,
    load_pull_request_file_content_flow, load_source_file_tree_flow, load_structural_diff_flow,
    load_temp_source_file_content_flow, warm_structural_diffs_flow,
};
pub use self::review_comments::{
    trigger_submit_inline_comment, trigger_submit_review_from_review_mode,
};

use self::combined_diff::*;
use self::diff_metrics::*;
use self::guided_review::*;
use self::review_comments::{
    begin_review_line_drag, build_review_line_action_target, finish_review_line_drag,
    open_review_line_action, pending_review_comment_count, render_diff_comment_icon,
    render_diff_open_source_icon, render_diff_waypoint_icon, render_finish_review_modal,
    render_review_line_action_overlay, render_review_thread, render_reviewable_diff_line,
    render_waypoint_pill, review_line_action_target_with_range, review_thread_ui_state,
    update_review_line_drag, ReviewThreadUiState,
};
use self::review_sidebar::{
    default_stack_layer, default_waymark_name, metric_pill, open_review_location_card,
    prepare_review_file_tree_rows, prepare_review_queue, prepare_review_stack,
    prepare_semantic_diff_file, render_review_sidebar_pane, reset_stack_timeline_list_state,
    stack_file_paths_for_filter,
};
use self::side_by_side::*;
use self::single_file_diff::*;

pub(super) const DIFF_FOCUS_CONTEXT_ROWS: usize = 12;
const DIFF_VIM_SCROLL_EDGE_CONTEXT_ROWS: f32 = 4.0;
const DIFF_VIM_SCROLL_EDGE_MAX_VIEWPORT_FRACTION: f32 = 0.24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiffFocusScrollBehavior {
    Context,
    VimMotion,
}

pub(super) fn diff_focus_scroll_top_item_ix(focus_item_ix: usize) -> usize {
    focus_item_ix.saturating_sub(DIFF_FOCUS_CONTEXT_ROWS)
}

pub(super) fn scroll_diff_list_to_vim_focus(list_state: &ListState, item_ix: usize) {
    let viewport_bounds = list_state.viewport_bounds();
    let Some(item_bounds) = list_state.bounds_for_item(item_ix) else {
        list_state.scroll_to_reveal_item(item_ix);
        return;
    };

    if let Some(delta) = diff_vim_scroll_delta_for_bounds(item_bounds, viewport_bounds) {
        list_state.scroll_by(delta);
    }
}

fn diff_vim_scroll_delta_for_bounds(
    item_bounds: Bounds<Pixels>,
    viewport_bounds: Bounds<Pixels>,
) -> Option<Pixels> {
    let viewport_height = f32::from(viewport_bounds.size.height);
    let item_height = f32::from(item_bounds.size.height);
    if viewport_height <= 0.0 || item_height <= 0.0 {
        return None;
    }

    let available_margin = ((viewport_height - item_height).max(0.0) * 0.5)
        .min(viewport_height * DIFF_VIM_SCROLL_EDGE_MAX_VIEWPORT_FRACTION);
    let edge_context = (item_height * DIFF_VIM_SCROLL_EDGE_CONTEXT_ROWS).min(available_margin);
    let top_edge = viewport_bounds.top() + px(edge_context);
    let bottom_edge = viewport_bounds.bottom() - px(edge_context);

    if item_bounds.top() < top_edge {
        Some(item_bounds.top() - top_edge)
    } else if item_bounds.bottom() > bottom_edge {
        Some(item_bounds.bottom() - bottom_edge)
    } else {
        None
    }
}

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
    if state.read(cx).review_ai_background_jobs_enabled() {
        review_intelligence::trigger_review_intelligence(
            state,
            window,
            cx,
            review_intelligence::ReviewIntelligenceScope::All,
            false,
        );
    }
    ensure_structural_diff_warmup_started(state, window, cx);
}

pub fn switch_review_code_mode(
    state: &Entity<AppState>,
    mode: ReviewCenterMode,
    window: &mut Window,
    cx: &mut App,
) {
    if mode == ReviewCenterMode::GuidedReview && !state.read(cx).review_ai_features_enabled() {
        return;
    }

    state.update(cx, |state, cx| {
        state.set_review_center_mode_preserving_focus(mode);
        state.persist_active_review_session();
        cx.notify();
    });

    ensure_active_review_focus_loaded(state, window, cx);
}

pub fn enter_stack_review_mode(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    if !state.read(cx).review_ai_features_enabled() {
        return;
    }

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

pub fn trigger_diff_vim_key(
    state: &Entity<AppState>,
    key: VimKey,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let mut confirmed_target = None;
    let consumed = state.update(cx, |state, cx| {
        if !diff_vim_keyboard_available(state) {
            return false;
        }

        let visual_active = state.diff_vim.mode() == ReadOnlyVimMode::VisualLine
            || state.diff_vim_state.visual_anchor_index().is_some();
        if matches!(key, VimKey::Escape) && !visual_active && !state.diff_vim.has_pending_input() {
            return false;
        }

        if matches!(key, VimKey::Char('c')) && !visual_active {
            state.diff_vim.handle_key(key);
            return false;
        }

        let rows = state.active_diff_vim_targets();
        if rows.is_empty() {
            state.diff_vim.reset();
            return false;
        }

        if let Some(selected) = state.diff_vim_cursor_seed_target() {
            state.diff_vim_state.set_cursor_for_target(&rows, &selected);
        }

        let had_pending_input = state.diff_vim.has_pending_input();
        let intent = state.diff_vim.handle_key(key);
        let has_pending_input = state.diff_vim.has_pending_input();
        let outcome = state.diff_vim_state.apply_intent(&rows, intent);
        let should_consume_noop = intent != VimIntent::Noop
            || had_pending_input
            || has_pending_input
            || visual_active
            || matches!(key, VimKey::Escape);

        match outcome {
            VimDiffOutcome::Noop => {
                if should_consume_noop {
                    state.suppress_diff_vim_pointer_hover();
                    cx.notify();
                }
                should_consume_noop
            }
            VimDiffOutcome::Cancelled => {
                state.active_review_line_drag_origin = None;
                state.active_review_line_drag_current = None;
                state.suppress_diff_vim_pointer_hover();
                cx.notify();
                true
            }
            VimDiffOutcome::Moved { cursor } => {
                focus_diff_vim_target(state, &cursor);
                state.suppress_diff_vim_pointer_hover();
                cx.notify();
                true
            }
            VimDiffOutcome::VisualStarted { origin, cursor } => {
                focus_diff_vim_target(state, &cursor);
                state.active_review_line_drag_origin = Some(origin);
                state.active_review_line_drag_current = Some(cursor);
                state.suppress_diff_vim_pointer_hover();
                cx.notify();
                true
            }
            VimDiffOutcome::VisualChanged { origin, cursor, .. } => {
                focus_diff_vim_target(state, &cursor);
                state.active_review_line_drag_origin = Some(origin);
                state.active_review_line_drag_current = Some(cursor);
                state.suppress_diff_vim_pointer_hover();
                cx.notify();
                true
            }
            VimDiffOutcome::SelectionConfirmed { target } => {
                state.active_review_line_drag_origin = None;
                state.active_review_line_drag_current = None;
                focus_diff_vim_target(state, &target);
                state.suppress_diff_vim_pointer_hover();
                confirmed_target = Some(target);
                cx.notify();
                true
            }
        }
    });

    if let Some(target) = confirmed_target {
        open_review_line_action(state, target, window.mouse_position(), cx);
    }

    consumed
}

fn diff_vim_keyboard_available(state: &AppState) -> bool {
    state.active_surface == PullRequestSurface::Files
        && state.active_detail().is_some()
        && state.effective_review_center_mode() != ReviewCenterMode::SourceBrowser
        && !state.palette_open
        && !state.file_chooser_open
        && !state.waypoint_spotlight_open
        && state.project_shader_picker.is_none()
        && state.active_onboarding_wizard.is_none()
        && !state.review_editor_active
        && !state.review_finish_modal_open
        && state.active_review_line_action.is_none()
        && state.active_review_thread_reply_id.is_none()
        && state.editing_review_comment_id.is_none()
}

fn focus_diff_vim_target(state: &mut AppState, target: &ReviewLineActionTarget) {
    state.active_surface = PullRequestSurface::Files;
    state.selected_file_path = Some(target.anchor.file_path.clone());
    state.selected_diff_anchor = Some(target.anchor.clone());
    state.active_review_line_action = None;
    state.active_review_line_action_position = None;
    state.review_line_action_mode = ReviewLineActionMode::Menu;
    state.inline_comment_error = None;
    state.waypoint_spotlight_open = false;
    state.clear_review_scroll_focus();
    state.persist_active_review_session();
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
        state.file_chooser_open = false;
        state.file_chooser_query.clear();
        state.file_chooser_selected_index = 0;
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
                                        .child(badge(&shortcuts::secondary_key_label("j")))
                                        .child(badge(&shortcuts::secondary_shift_key_label("j"))),
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
                                    .child(panel_state_text(&format!(
                                        "No waypoints yet. Click a diff line, choose Add waypoint, or press {} on a selected line.",
                                        shortcuts::secondary_shift_key_label("j")
                                    ))),
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
    let location_label = waypoint_spotlight_location_label(&waymark.location);
    let detail_label = waypoint_spotlight_detail_label(waymark, &location_label);
    let mode_label = waymark.location.mode.label();
    let state = state.clone();

    div()
        .w_full()
        .flex_shrink_0()
        .px(px(20.0))
        .py(px(12.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .bg(if selected {
            bg_selected()
        } else {
            bg_overlay()
        })
        .hover(move |style| style.bg(if selected { bg_selected() } else { bg_subtle() }))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            close_waypoint_spotlight(&state, cx);
            open_review_location_card(&state, &location, window, cx);
        })
        .child(
            div()
                .w_full()
                .flex()
                .items_start()
                .gap(px(12.0))
                .flex_grow()
                .min_w_0()
                .child(
                    div()
                        .mt(px(1.0))
                        .w(px(24.0))
                        .h(px(24.0))
                        .flex_shrink_0()
                        .rounded(radius_sm())
                        .border_1()
                        .border_color(border_muted())
                        .bg(if selected { bg_overlay() } else { bg_surface() })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(lucide_icon(
                            LucideIcon::Waypoints,
                            13.0,
                            if selected { accent() } else { fg_muted() },
                        )),
                )
                .child(
                    div()
                        .flex_grow()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(4.0))
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .font_family(mono_font_family())
                                .text_color(fg_emphasis())
                                .child(location_label),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(11.0))
                                .line_height(px(15.0))
                                .text_color(fg_muted())
                                .child(
                                    detail_label
                                        .map(|detail| format!("{detail} · {mode_label}"))
                                        .unwrap_or_else(|| mode_label.to_string()),
                                ),
                        ),
                ),
        )
}

fn waypoint_spotlight_location_label(location: &ReviewLocation) -> String {
    let line = match location.mode {
        ReviewCenterMode::SourceBrowser => location.source_line,
        ReviewCenterMode::SemanticDiff
        | ReviewCenterMode::StructuralDiff
        | ReviewCenterMode::GuidedReview => location
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.line)
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0),
    };

    let rebuilt = location_label(&location.file_path, line);
    if rebuilt.trim().is_empty() {
        location.label.clone()
    } else {
        rebuilt
    }
}

fn waypoint_spotlight_detail_label(
    waymark: &crate::review_session::ReviewWaymark,
    primary_label: &str,
) -> Option<String> {
    let name = waymark.name.trim();
    (!name.is_empty() && name != primary_label && name != waymark.location.label)
        .then(|| name.to_string())
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
    let file_chooser_open = s.file_chooser_open;
    let line_action_target = s.active_review_line_action.clone();
    let line_action_position = s.active_review_line_action_position;
    let line_action_mode = s.review_line_action_mode.clone();
    let review_finish_modal_open = s.review_finish_modal_open;
    let is_local_review = crate::local_review::is_local_review_detail(detail);
    let review_stack = prepare_review_stack(&s, detail);
    let review_queue = prepare_review_queue(&s, detail);
    let mut review_session = s.active_review_session().cloned().unwrap_or_default();
    review_session.center_mode = s.effective_review_center_mode();
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
    let surface_radius = radius();

    div()
        .relative()
        .flex()
        .flex_grow()
        .min_h_0()
        .bg(bg_canvas())
        .p(px(REVIEW_SURFACE_GAP))
        .gap(if show_file_tree {
            px(REVIEW_SURFACE_GAP)
        } else {
            px(0.0)
        })
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
                .relative()
                .rounded(surface_radius)
                .bg(bg_surface())
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
                .child(render_corner_mask(
                    surface_radius,
                    bg_canvas(),
                    CornerMask::ALL,
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
        .when(file_chooser_open, |el| {
            el.child(render_file_chooser(state, cx))
        })
        .when(review_finish_modal_open && !is_local_review, |el| {
            el.child(render_finish_review_modal(state, detail, cx))
        })
        .when_some(
            line_action_target
                .as_ref()
                .zip(line_action_position)
                .map(|(target, position)| (target.clone(), position)),
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
const REVIEW_SURFACE_GAP: f32 = 10.0;

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
const DIFF_FILE_HEADER_BOTTOM_MARGIN: f32 = 2.0;
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
    let mut review_session = app_state
        .active_review_session()
        .cloned()
        .unwrap_or_default();
    review_session.center_mode = app_state.effective_review_center_mode();
    let center_mode = review_session.center_mode;
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
    let stack_filter = (center_mode == ReviewCenterMode::GuidedReview)
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
    let guided_review_preparation_overlay = (center_mode == ReviewCenterMode::GuidedReview)
        .then(|| render_guided_review_preparation_overlay(state, app_state, window))
        .flatten();
    let active_lsp_path = if center_mode == ReviewCenterMode::SourceBrowser {
        source_target.as_ref().map(|target| target.path.as_str())
    } else {
        selected_file.map(|file| file.path.as_str())
    };
    let lsp_status_popup = active_lsp_path.and_then(|path| {
        app_state
            .active_lsp_status_notice_for_path(path)
            .map(|notice| (path.to_string(), notice))
    });

    div()
        .relative()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .bg(bg_canvas())
        .gap(px(REVIEW_SURFACE_GAP))
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
        .child({
            let panel_radius = radius();
            div()
                .relative()
                .flex_grow()
                .min_h_0()
                .bg(bg_surface())
                .rounded(panel_radius)
                .overflow_hidden()
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
                            window,
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
                            window,
                            cx,
                        )
                        .into_any_element()
                    },
                )
                .child(render_corner_mask(
                    panel_radius,
                    bg_canvas(),
                    CornerMask::ALL,
                ))
        })
        .when_some(guided_review_preparation_overlay, |el, overlay| {
            el.child(overlay)
        })
        .when_some(lsp_status_popup, |el, (path, notice)| {
            el.child(render_lsp_status_popup(state, path, notice, app_state))
        })
}

fn render_lsp_status_popup(
    state: &Entity<AppState>,
    path: String,
    notice: LspStatusNotice,
    app_state: &AppState,
) -> AnyElement {
    let dismissal_key = notice.dismissal_key.clone();
    let icon = if notice.busy {
        LucideIcon::RefreshCw
    } else {
        LucideIcon::AlertTriangle
    };
    let icon_color = if notice.busy { info() } else { warning() };
    let detail = compact_lsp_notice_detail(&notice.detail);

    div()
        .absolute()
        .right(px(18.0))
        .bottom(px(18.0))
        .w(px(336.0))
        .max_w(px(336.0))
        .rounded(radius())
        .border_1()
        .border_color(diff_annotation_border())
        .bg(bg_overlay())
        .shadow(popover_shadow())
        .occlude()
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(9.0))
                .child(div().mt(px(1.0)).child(lucide_icon(icon, 14.0, icon_color)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(notice.title),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(fg_muted())
                                .child(detail),
                        ),
                )
                .child(render_lsp_status_close_button(
                    state,
                    dismissal_key,
                    "Dismiss LSP notice",
                )),
        )
        .when_some(notice.install_kind, |el, kind| {
            el.child(render_lsp_install_action(state, path, kind, app_state))
        })
        .into_any_element()
}

fn render_lsp_status_close_button(
    state: &Entity<AppState>,
    dismissal_key: String,
    tooltip: &'static str,
) -> impl IntoElement {
    let state = state.clone();
    div()
        .id("lsp-status-dismiss")
        .w(px(22.0))
        .h(px(22.0))
        .flex_shrink_0()
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(fg_subtle())
        .hover(|style| style.bg(bg_selected()).text_color(fg_emphasis()))
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            state.update(cx, |state, cx| {
                state.dismiss_lsp_status_notice_key(dismissal_key.clone());
                cx.notify();
            });
        })
        .child(lucide_icon(LucideIcon::X, 12.0, fg_subtle()))
}

fn render_lsp_install_action(
    state: &Entity<AppState>,
    path: String,
    kind: managed_lsp::ManagedServerKind,
    app_state: &AppState,
) -> AnyElement {
    let installing = app_state.managed_lsp_settings.installing.contains(&kind);
    let install_state = app_state
        .managed_lsp_settings
        .statuses
        .get(&kind)
        .map(|status| status.state)
        .unwrap_or(ManagedServerInstallState::NotInstalled);
    let label = lsp_install_action_label(install_state, installing);
    let base = div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(radius_sm())
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if installing { fg_subtle() } else { info() })
        .child(lucide_icon(
            if installing {
                LucideIcon::RefreshCw
            } else {
                LucideIcon::Plug
            },
            12.0,
            if installing { fg_subtle() } else { info() },
        ))
        .child(label);

    if installing {
        return base.bg(bg_subtle()).into_any_element();
    }

    let state_for_install = state.clone();
    base.hover(|style| style.bg(info_muted()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let path = path.clone();
            state_for_install.update(cx, |state, cx| {
                let Some(detail_key) = state.active_pr_key.clone() else {
                    return;
                };
                let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                    return;
                };
                detail_state.lsp_statuses.remove(&path);
                detail_state.lsp_loading_paths.remove(&path);
                cx.notify();
            });
            super::settings::trigger_managed_lsp_install(&state_for_install, kind, window, cx);
        })
        .into_any_element()
}

fn lsp_install_action_label(
    install_state: ManagedServerInstallState,
    installing: bool,
) -> &'static str {
    if installing {
        return "Installing...";
    }

    match install_state {
        ManagedServerInstallState::NotInstalled => "Install",
        ManagedServerInstallState::Installed => "Reinstall",
        ManagedServerInstallState::Broken => "Repair",
    }
}

fn compact_lsp_notice_detail(detail: &str) -> String {
    const MAX_LEN: usize = 180;
    let normalized = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= MAX_LEN {
        return normalized;
    }

    let mut compact = normalized
        .chars()
        .take(MAX_LEN.saturating_sub(3))
        .collect::<String>();
    compact.push_str("...");
    compact
}

fn render_review_header_change_summary(
    total_additions: i64,
    total_deletions: i64,
    focus_summary: String,
) -> impl IntoElement {
    let has_focus_summary = !focus_summary.is_empty();

    div()
        .text_size(px(11.0))
        .font_family(mono_font_family())
        .min_w_0()
        .whitespace_nowrap()
        .overflow_x_hidden()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(
            div()
                .flex_shrink_0()
                .text_color(if total_additions > 0 {
                    success()
                } else {
                    fg_subtle()
                })
                .child(format!("+{total_additions}")),
        )
        .child(div().flex_shrink_0().text_color(fg_subtle()).child("/"))
        .child(
            div()
                .flex_shrink_0()
                .text_color(if total_deletions > 0 {
                    danger()
                } else {
                    fg_subtle()
                })
                .child(format!("-{total_deletions}")),
        )
        .when(has_focus_summary, |el| {
            el.child(div().flex_shrink_0().text_color(fg_subtle()).child("/"))
                .child(
                    div()
                        .min_w_0()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .text_color(fg_muted())
                        .child(focus_summary),
                )
        })
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
    );
    let is_local_review = crate::local_review::is_local_review_detail(detail);
    let state_for_refresh = state.clone();
    let state_for_submit = state.clone();
    let pending_count = pending_review_comment_count(detail);
    let stale_local_feedback_count = if is_local_review {
        detail
            .review_threads
            .iter()
            .filter(|thread| thread.is_outdated)
            .count()
    } else {
        0
    };
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
        .bg(bg_surface())
        .rounded(radius())
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
                .child(render_review_header_change_summary(
                    total_additions,
                    total_deletions,
                    focus_summary,
                )),
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
            el.when(pending_count > 0, |el| {
                el.child(badge(&format!("{pending_count} feedback")))
            })
            .when(stale_local_feedback_count > 0, |el| {
                el.child(badge(&format!("{stale_local_feedback_count} stale")))
            })
            .child(review_button("Refresh", move |_, window, cx| {
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
            el.hover(move |style| style.bg(if active { bg_emphasis() } else { bg_selected() }))
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
            review_thread_anchor(t)
                .map(|anchor| line_matches_diff_anchor(line, Some(&anchor)))
                .unwrap_or(false)
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
        build_normal_side_by_side_diff_file, current_combined_diff_file_index_for_scroll_top,
        diff_focus_scroll_top_item_ix, diff_vim_scroll_delta_for_bounds,
        estimated_combined_diff_body_height_for_counts, focus_item_index_around,
        max_side_by_side_column_widths, reading_focus_item_index,
        should_animate_combined_diff_jump, should_hydrate_combined_diff_file,
        waypoint_spotlight_detail_label, waypoint_spotlight_location_label, CombinedDiffViewItem,
        DiffFileCollapseScrollAdjustment, DiffVerticalScrollbarMetrics, DiffViewItem,
        SideBySideColumnWidths, StructuralDiffTerminalStatus, DIFF_FILE_HEADER_TOP_MARGIN,
        DIFF_FOCUS_CONTEXT_ROWS,
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
    fn stack_timeline_initializes_with_base_branch_at_top() {
        let list_state = ListState::new(0, ListAlignment::Top, px(36.0));

        sync_stack_timeline_item_count(&list_state, 5);

        let scroll_top = list_state.logical_scroll_top();
        assert_eq!(scroll_top.item_ix, 0);
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
    fn diff_focus_scroll_uses_middle_context_offset() {
        assert_eq!(
            diff_focus_scroll_top_item_ix(DIFF_FOCUS_CONTEXT_ROWS + 9),
            9
        );
        assert_eq!(diff_focus_scroll_top_item_ix(4), 0);
    }

    #[test]
    fn vim_focus_scroll_waits_until_selection_reaches_edge_context() {
        let viewport = test_bounds(0.0, 300.0);

        assert_eq!(
            diff_vim_scroll_delta_for_bounds(test_bounds(116.0, 144.0), viewport),
            None
        );
        assert_eq!(
            diff_vim_scroll_delta_for_bounds(test_bounds(240.0, 268.0), viewport),
            Some(px(40.0))
        );
        assert_eq!(
            diff_vim_scroll_delta_for_bounds(test_bounds(20.0, 48.0), viewport),
            Some(px(-52.0))
        );
    }

    #[test]
    fn waypoint_spotlight_rebuilds_primary_location_label() {
        let anchor = crate::review_ai::DiffAnchor {
            file_path: "src/views/diff_view.rs".to_string(),
            hunk_header: None,
            line: Some(842),
            side: Some("RIGHT".to_string()),
            thread_id: None,
        };
        let mut location = crate::review_session::ReviewLocation::from_diff(
            "src/views/diff_view.rs",
            Some(anchor),
        );
        location.label = "...".to_string();

        assert_eq!(
            waypoint_spotlight_location_label(&location),
            "src/views/diff_view.rs:842"
        );
    }

    #[test]
    fn waypoint_spotlight_suppresses_duplicate_detail_label() {
        let location =
            crate::review_session::ReviewLocation::from_source("src/main.rs", Some(12), None);
        let waymark = crate::review_session::ReviewWaymark {
            id: "wm-test".to_string(),
            name: "src/main.rs:12".to_string(),
            location,
            created_at_ms: 0,
        };

        assert_eq!(
            waypoint_spotlight_detail_label(&waymark, "src/main.rs:12"),
            None
        );
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
    fn floating_combined_header_waits_for_next_header_row() {
        let items = vec![
            CombinedDiffViewItem::Header(0),
            CombinedDiffViewItem::Row {
                file_index: 0,
                item: DiffViewItem::Row(0),
            },
            CombinedDiffViewItem::Footer,
            CombinedDiffViewItem::Header(1),
            CombinedDiffViewItem::Row {
                file_index: 1,
                item: DiffViewItem::Row(0),
            },
        ];

        assert_eq!(
            current_combined_diff_file_index_for_scroll_top(
                &items,
                ListOffset {
                    item_ix: 1,
                    offset_in_item: px(0.0),
                },
            ),
            Some(0)
        );
        assert_eq!(
            current_combined_diff_file_index_for_scroll_top(
                &items,
                ListOffset {
                    item_ix: 2,
                    offset_in_item: px(0.0),
                },
            ),
            Some(0)
        );
        assert_eq!(
            current_combined_diff_file_index_for_scroll_top(
                &items,
                ListOffset {
                    item_ix: 3,
                    offset_in_item: px(DIFF_FILE_HEADER_TOP_MARGIN - 1.0),
                },
            ),
            Some(0)
        );
        assert_eq!(
            current_combined_diff_file_index_for_scroll_top(
                &items,
                ListOffset {
                    item_ix: 3,
                    offset_in_item: px(DIFF_FILE_HEADER_TOP_MARGIN),
                },
            ),
            Some(1)
        );
        assert_eq!(
            current_combined_diff_file_index_for_scroll_top(
                &items,
                ListOffset {
                    item_ix: 4,
                    offset_in_item: px(0.0),
                },
            ),
            Some(1)
        );
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
    fn combined_diff_progressive_hydration_prioritizes_initial_and_requested_files() {
        let mut hydrated_paths = std::collections::HashSet::new();
        hydrated_paths.insert("src/far.rs".to_string());

        assert!(should_hydrate_combined_diff_file(
            "src/near.rs",
            2,
            4,
            &hydrated_paths,
            false,
        ));
        assert!(should_hydrate_combined_diff_file(
            "src/far.rs",
            80,
            4,
            &hydrated_paths,
            false,
        ));
        assert!(!should_hydrate_combined_diff_file(
            "src/collapsed.rs",
            1,
            4,
            &hydrated_paths,
            true,
        ));
        assert!(!should_hydrate_combined_diff_file(
            "src/later.rs",
            80,
            4,
            &hydrated_paths,
            false,
        ));
    }

    #[test]
    fn combined_diff_deferred_body_height_uses_line_estimate() {
        assert_eq!(
            estimated_combined_diff_body_height_for_counts(20, 2),
            px(708.0)
        );
        assert_eq!(
            estimated_combined_diff_body_height_for_counts(0, 0),
            px(58.0)
        );
        assert_eq!(
            estimated_combined_diff_body_height_for_counts(10_000, 200),
            px(326800.0)
        );
    }

    #[test]
    fn diff_scrollbar_metrics_map_offsets_without_measuring_rows() {
        let metrics = DiffVerticalScrollbarMetrics::from_item_heights([10.0, 20.0, 30.0]);

        assert_eq!(metrics.max_offset(15.0), 45.0);
        assert_eq!(
            metrics.scroll_offset_for(ListOffset {
                item_ix: 1,
                offset_in_item: px(5.0),
            }),
            15.0
        );

        let scroll_top = metrics.scroll_top_for_offset(35.0);
        assert_eq!(scroll_top.item_ix, 2);
        assert_eq!(scroll_top.offset_in_item, px(5.0));
    }

    #[test]
    fn combined_diff_jump_animation_only_applies_to_established_far_targets() {
        assert!(!should_animate_combined_diff_jump(false, 0, 100));
        assert!(!should_animate_combined_diff_jump(true, 40, 50));
        assert!(should_animate_combined_diff_jump(true, 4, 80));
        assert!(should_animate_combined_diff_jump(true, 80, 4));
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
