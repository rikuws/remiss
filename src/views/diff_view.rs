use std::{
    collections::{hash_map::DefaultHasher, BTreeSet},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use gpui::prelude::*;
use gpui::*;

use crate::code_display::{
    build_interactive_code_tokens, build_lsp_hover_tooltip_view, code_text_runs,
    render_highlighted_code_block, render_highlighted_code_content, InteractiveCodeToken,
};
use crate::code_tour::{
    line_matches_diff_anchor, thread_matches_diff_anchor, CodeTourProvider, CodeTourProviderStatus,
    DiffAnchor, GeneratedCodeTour, TourSection, TourSectionCategory, TourSectionPriority, TourStep,
};
use crate::diff::{
    build_diff_render_rows, find_parsed_diff_file, find_parsed_diff_file_with_index, DiffLineKind,
    DiffRenderRow, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine,
};
use crate::github::{
    PullRequestDetail, PullRequestFile, PullRequestReviewComment, PullRequestReviewThread,
    RepositoryFileContent, REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT,
};
use crate::icons::{lucide_icon, LucideIcon};
use crate::local_documents;
use crate::local_repo;
use crate::lsp;
use crate::markdown::render_markdown;
use crate::review_queue::{build_review_queue, ReviewQueue, ReviewQueueBucket};
use crate::review_session::{ReviewCenterMode, ReviewLocation, ReviewSourceTarget};
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
use crate::syntax::{self, SyntaxSpan};
use crate::theme::*;
use crate::{github, notifications, review_intelligence};

use super::ai_tour::{refresh_active_tour, trigger_generate_tour};
use super::sections::{
    badge, badge_success, error_text, eyebrow, ghost_button, material_surface, nested_panel,
    panel_state_text, review_button, success_text, user_avatar,
};

pub fn enter_files_surface(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.active_surface = PullRequestSurface::Files;
        s.pr_header_compact = false;
        s.set_review_file_tree_visible(true);

        if s.selected_file_path.is_none() {
            s.selected_file_path = s.active_detail().and_then(|detail| {
                crate::review_queue::default_review_file(detail)
                    .or_else(|| detail.parsed_diff.first().map(|file| file.path.clone()))
            });
        }

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
        state.set_review_center_mode(ReviewCenterMode::Stack);

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
    let source_target = state.read(cx).active_review_session().and_then(|session| {
        (session.center_mode == ReviewCenterMode::SourceBrowser)
            .then(|| session.source_target.clone())
            .flatten()
    });

    if let Some(source_target) = source_target {
        let model = state.clone();
        window
            .spawn(cx, async move |cx: &mut AsyncWindowContext| {
                load_local_source_file_content_flow(model, source_target.path, cx).await;
            })
            .detach();
    } else {
        ensure_selected_file_content_loaded(state, window, cx);
    }
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
        state.inline_comment_draft.clear();
        state.inline_comment_error = None;
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

pub fn trigger_submit_inline_comment(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let Some((detail_id, repository, number, target, body, loading)) = ({
        let app_state = state.read(cx);
        app_state.active_detail().and_then(|detail| {
            app_state.active_review_line_action.clone().map(|target| {
                (
                    detail.id.clone(),
                    detail.repository.clone(),
                    detail.number,
                    target,
                    app_state.inline_comment_draft.clone(),
                    app_state.inline_comment_loading,
                )
            })
        })
    }) else {
        return;
    };

    if loading {
        return;
    }

    if body.trim().is_empty() {
        state.update(cx, |state, cx| {
            state.inline_comment_error =
                Some("Enter a line comment before submitting it.".to_string());
            cx.notify();
        });
        return;
    }

    let Some(line) = target.anchor.line else {
        return;
    };
    let Some(side) = target.anchor.side.clone() else {
        return;
    };

    state.update(cx, |state, cx| {
        state.inline_comment_loading = true;
        state.inline_comment_error = None;
        cx.notify();
    });

    let model = state.clone();
    let target_for_refresh = target.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let submit_result = cx
                .background_executor()
                .spawn(async move {
                    github::add_pull_request_review_thread(
                        &detail_id,
                        &target.anchor.file_path,
                        &body,
                        Some(line),
                        Some(side.as_str()),
                        Some("LINE"),
                    )
                })
                .await;

            let (success, message) = match submit_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            if !success {
                model
                    .update(cx, |state, cx| {
                        state.inline_comment_loading = false;
                        state.inline_comment_error = Some(message);
                        cx.notify();
                    })
                    .ok();
                return;
            }

            let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let repository_for_sync = repository.clone();

            let sync_result = cx
                .background_executor()
                .spawn(async move {
                    notifications::sync_pull_request_detail_with_read_state(
                        &cache,
                        &repository_for_sync,
                        number,
                    )
                })
                .await;

            model
                .update(cx, |state, cx| {
                    state.inline_comment_loading = false;
                    state.inline_comment_draft.clear();
                    state.inline_comment_error = None;

                    if state
                        .active_review_line_action
                        .as_ref()
                        .map(|active| active.stable_key() == target_for_refresh.stable_key())
                        .unwrap_or(false)
                    {
                        state.active_review_line_action = None;
                        state.active_review_line_action_position = None;
                        state.review_line_action_mode = ReviewLineActionMode::Menu;
                    }

                    let detail_key = pr_key(&repository, number);
                    let detail_state = state.detail_states.entry(detail_key).or_default();
                    match sync_result {
                        Ok((snapshot, unread_ids)) => {
                            detail_state.snapshot = Some(snapshot);
                            detail_state.error = None;
                            state.unread_review_comment_ids = unread_ids;
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

fn open_review_line_action(
    state: &Entity<AppState>,
    target: ReviewLineActionTarget,
    position: Point<Pixels>,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        state.active_surface = PullRequestSurface::Files;
        state.navigate_to_review_location(target.review_location(), true);
        state.active_review_line_action = Some(target);
        state.active_review_line_action_position = Some(position);
        state.review_line_action_mode = ReviewLineActionMode::Menu;
        state.inline_comment_draft.clear();
        state.inline_comment_error = None;
        state.waypoint_spotlight_open = false;
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
        .flex()
        .justify_center()
        .pt(px(88.0))
        .child(
            div()
                .absolute()
                .inset_0()
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
                .border_color(border_default())
                .bg(bg_overlay())
                .shadow_sm()
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
                                .border_color(focus_border())
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
        .border_color(if selected {
            focus_border()
        } else {
            border_muted()
        })
        .bg(if selected {
            bg_selected()
        } else {
            bg_overlay()
        })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
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

pub fn render_files_view(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
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
    let review_stack = prepare_review_stack(&s, detail);
    let review_queue = prepare_review_queue(&s, detail);
    let review_session = s.active_review_session().cloned().unwrap_or_default();
    let show_file_tree = review_session.show_file_tree;

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
    let selected_file = selected_path.and_then(|path| files.iter().find(|file| file.path == path));
    let semantic_file = selected_file.map(|file| prepare_semantic_diff_file(&s, detail, file));

    div()
        .relative()
        .flex()
        .flex_grow()
        .min_h_0()
        .bg(diff_editor_bg())
        .when(show_file_tree, |el| {
            el.child(render_review_sidebar_pane(
                state,
                detail,
                review_queue.as_ref(),
                selected_path,
                semantic_file.as_deref(),
                &review_session,
                review_stack.clone(),
                cx,
            ))
        })
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
                    semantic_file.as_deref(),
                    review_stack.clone(),
                    cx,
                )),
        )
        .child(render_review_panel_toggle(
            state,
            ReviewPanelToggleSide::FileTree,
            show_file_tree,
        ))
        .when(waypoint_spotlight_open, |el| {
            el.child(render_waypoint_spotlight(state, cx))
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

const REVIEW_PANEL_TOGGLE_SIZE: f32 = 22.0;
const REVIEW_PANEL_TOGGLE_HIDDEN_INSET: f32 = 10.0;
const REVIEW_PANEL_TOGGLE_OVERLAP: f32 = REVIEW_PANEL_TOGGLE_SIZE / 2.0;
const REVIEW_PANEL_TOGGLE_TOP: f32 = 10.0;

#[derive(Clone, Copy)]
enum ReviewPanelToggleSide {
    FileTree,
}

fn render_review_panel_toggle(
    state: &Entity<AppState>,
    side: ReviewPanelToggleSide,
    visible: bool,
) -> AnyElement {
    let (icon, tooltip) = match (side, visible) {
        (ReviewPanelToggleSide::FileTree, true) => {
            (LucideIcon::PanelLeftClose, "Hide review sidebar")
        }
        (ReviewPanelToggleSide::FileTree, false) => {
            (LucideIcon::PanelLeftOpen, "Show review sidebar")
        }
    };
    let button_id = match side {
        ReviewPanelToggleSide::FileTree => "review-file-tree-toggle",
    };
    let button_animation_id =
        SharedString::from(format!("{button_id}-button-{}", usize::from(visible)));
    let position_animation_id =
        SharedString::from(format!("{button_id}-position-{}", usize::from(visible)));
    let state = state.clone();
    let button = div()
        .id(button_id)
        .w(px(REVIEW_PANEL_TOGGLE_SIZE))
        .h(px(REVIEW_PANEL_TOGGLE_SIZE))
        .rounded(radius_sm())
        .border_1()
        .border_color(if visible {
            diff_annotation_border()
        } else {
            border_muted()
        })
        .bg(if visible {
            diff_editor_chrome()
        } else {
            diff_editor_surface()
        })
        .shadow_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(diff_line_hover_bg()))
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            state.update(cx, |state, cx| {
                match side {
                    ReviewPanelToggleSide::FileTree => {
                        state.set_review_file_tree_visible(!visible);
                    }
                }
                state.persist_active_review_session();
                cx.notify();
            });
        })
        .child(lucide_icon(
            icon,
            12.0,
            if visible { fg_emphasis() } else { fg_muted() },
        ))
        .with_animation(
            button_animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(visible, delta);
                el.bg(mix_rgba(
                    diff_editor_surface(),
                    diff_editor_chrome(),
                    progress,
                ))
                .border_color(mix_rgba(
                    border_muted(),
                    diff_annotation_border(),
                    progress,
                ))
            },
        );

    div()
        .absolute()
        .top(px(REVIEW_PANEL_TOGGLE_TOP))
        .left(if visible {
            file_tree_width() - px(REVIEW_PANEL_TOGGLE_OVERLAP)
        } else {
            px(REVIEW_PANEL_TOGGLE_HIDDEN_INSET)
        })
        .child(button)
        .with_animation(
            position_animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_transition_progress(visible, delta);
                let hidden_left = px(REVIEW_PANEL_TOGGLE_HIDDEN_INSET);
                let visible_left = file_tree_width() - px(REVIEW_PANEL_TOGGLE_OVERLAP);
                el.left(hidden_left + (visible_left - hidden_left) * progress)
            },
        )
        .into_any_element()
}

fn review_cache_key(active_pr_key: Option<&str>, scope: &str) -> String {
    format!("{}:{scope}", active_pr_key.unwrap_or("detached"))
}

fn prepare_review_stack(app_state: &AppState, detail: &PullRequestDetail) -> Arc<ReviewStack> {
    if let Some(stack) = prepare_discovered_review_stack(
        app_state,
        detail,
        "review-stack:real",
        StackDiscoveryOptions {
            enable_ai_virtual: false,
            enable_virtual_commits: false,
            enable_virtual_semantic: false,
            ..StackDiscoveryOptions::default()
        },
    ) {
        return stack;
    }

    let ai_stack_state = app_state
        .active_detail_state()
        .map(|detail_state| detail_state.ai_stack_state.clone())
        .unwrap_or_default();

    if let Some(stack) = ai_stack_state.stack.clone() {
        return stack;
    }

    if let Some(stack) = prepare_discovered_review_stack(
        app_state,
        detail,
        "review-stack:virtual",
        StackDiscoveryOptions {
            enable_github_native: false,
            enable_branch_topology: false,
            enable_local_metadata: false,
            enable_ai_virtual: false,
            enable_virtual_commits: true,
            enable_virtual_semantic: true,
            ..StackDiscoveryOptions::default()
        },
    ) {
        return stack;
    }

    if ai_stack_state.generating {
        return Arc::new(ai_stack_placeholder(
            detail,
            "Generating AI stack",
            "The selected provider is planning review layers from the prepared checkout.",
            None,
        ));
    }

    if ai_stack_state.loading {
        return Arc::new(ai_stack_placeholder(
            detail,
            "Preparing AI stack",
            ai_stack_state
                .message
                .as_deref()
                .unwrap_or("Preparing the local checkout before AI stack generation."),
            None,
        ));
    }

    if let Some(error) = ai_stack_state.error {
        return Arc::new(ai_stack_placeholder(
            detail,
            "AI stack unavailable",
            "The full pull request diff is still available while the AI stack is unavailable.",
            Some(error),
        ));
    }

    Arc::new(ai_stack_placeholder(
        detail,
        "AI stack queued",
        "Entering Review will prepare the checkout and generate the AI stack.",
        None,
    ))
}

fn prepare_discovered_review_stack(
    app_state: &AppState,
    detail: &PullRequestDetail,
    scope: &str,
    options: StackDiscoveryOptions,
) -> Option<Arc<ReviewStack>> {
    let cache_key = review_cache_key(app_state.active_pr_key.as_deref(), scope);
    let revision = detail.updated_at.clone();
    let open_pr_revision = review_stack_context_revision(app_state.active_detail_state());

    if let Some(cached) = app_state
        .review_stack_cache
        .borrow()
        .get(&cache_key)
        .filter(|cached| cached.revision == revision && cached.open_pr_revision == open_pr_revision)
        .cloned()
    {
        return Some(cached.stack);
    }

    let repo_context = review_stack_repo_context(app_state);
    let stack = discover_review_stack(detail, &repo_context, options).ok()?;
    let stack = Arc::new(stack);
    app_state.review_stack_cache.borrow_mut().insert(
        cache_key,
        CachedReviewStack {
            revision,
            open_pr_revision,
            stack: stack.clone(),
        },
    );

    Some(stack)
}

fn review_stack_repo_context(app_state: &AppState) -> RepoContext {
    let detail_state = app_state.active_detail_state();
    RepoContext {
        open_pull_requests: detail_state
            .and_then(|detail_state| detail_state.stack_open_pull_requests.clone())
            .unwrap_or_default(),
        local_repo_path: detail_state
            .and_then(|detail_state| detail_state.local_repository_status.as_ref())
            .and_then(|status| status.path.as_ref())
            .map(PathBuf::from),
        trunk_branch: None,
    }
}

fn review_stack_context_revision(detail_state: Option<&DetailState>) -> usize {
    let mut hasher = DefaultHasher::new();

    if let Some(detail_state) = detail_state {
        detail_state
            .stack_open_pull_requests_loading
            .hash(&mut hasher);
        detail_state
            .stack_open_pull_requests_error
            .hash(&mut hasher);
        if let Some(open_pull_requests) = detail_state.stack_open_pull_requests.as_ref() {
            open_pull_requests.len().hash(&mut hasher);
            for pull_request in open_pull_requests {
                pull_request.repository.hash(&mut hasher);
                pull_request.number.hash(&mut hasher);
                pull_request.base_ref_name.hash(&mut hasher);
                pull_request.head_ref_name.hash(&mut hasher);
                pull_request.base_ref_oid.hash(&mut hasher);
                pull_request.head_ref_oid.hash(&mut hasher);
                pull_request.state.hash(&mut hasher);
            }
        }
        if let Some(path) = detail_state
            .local_repository_status
            .as_ref()
            .and_then(|status| status.path.as_ref())
        {
            path.hash(&mut hasher);
        }
    }

    hasher.finish() as usize
}

fn default_stack_layer<'a>(
    stack: &'a ReviewStack,
    detail: &PullRequestDetail,
) -> Option<&'a ReviewStackLayer> {
    stack
        .layers
        .iter()
        .find(|layer| {
            layer
                .pr
                .as_ref()
                .map(|pr| pr.repository == detail.repository && pr.number == detail.number)
                .unwrap_or(false)
        })
        .or_else(|| stack.layers.first())
}

fn ai_stack_placeholder(
    detail: &PullRequestDetail,
    title: &str,
    summary: &str,
    warning: Option<String>,
) -> ReviewStack {
    let stack_id = format!(
        "ai-stack-placeholder:{}#{}:{}",
        detail.repository,
        detail.number,
        detail.head_ref_oid.as_deref().unwrap_or("unknown")
    );
    let warnings = warning
        .map(|message| vec![StackWarning::new("ai-virtual-stack-unavailable", message)])
        .unwrap_or_default();

    ReviewStack {
        id: stack_id.clone(),
        repository: detail.repository.clone(),
        selected_pr_number: detail.number,
        source: StackSource::VirtualAi,
        kind: StackKind::Virtual,
        confidence: Confidence::Low,
        trunk_branch: Some(detail.base_ref_name.clone()),
        base_oid: detail.base_ref_oid.clone(),
        head_oid: detail.head_ref_oid.clone(),
        layers: vec![ReviewStackLayer {
            id: format!("{stack_id}-pending"),
            index: 0,
            title: title.to_string(),
            summary: summary.to_string(),
            rationale: summary.to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualAi,
                role: crate::stacks::model::ChangeRole::Unknown,
                source_label: "AI stack".to_string(),
            }),
            base_oid: detail.base_ref_oid.clone(),
            head_oid: detail.head_ref_oid.clone(),
            atom_ids: Vec::new(),
            depends_on_layer_ids: Vec::new(),
            metrics: LayerMetrics::default(),
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Low,
            warnings: warnings.clone(),
        }],
        atoms: Vec::new(),
        warnings,
        provider: None,
        generated_at_ms: crate::stacks::model::stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }
}

fn prepare_review_queue(app_state: &AppState, detail: &PullRequestDetail) -> Arc<ReviewQueue> {
    let cache_key = review_cache_key(app_state.active_pr_key.as_deref(), "review-queue");
    let revision = detail.updated_at.clone();

    if let Some(cached) = app_state
        .review_queue_cache
        .borrow()
        .get(&cache_key)
        .filter(|cached| cached.revision == revision)
        .cloned()
    {
        return cached.queue;
    }

    let queue = Arc::new(build_review_queue(detail));
    app_state.review_queue_cache.borrow_mut().insert(
        cache_key,
        CachedReviewQueue {
            revision,
            queue: queue.clone(),
        },
    );
    queue
}

fn prepare_semantic_diff_file(
    app_state: &AppState,
    detail: &PullRequestDetail,
    file: &PullRequestFile,
) -> Arc<SemanticDiffFile> {
    let cache_key = format!(
        "{}:{}",
        review_cache_key(app_state.active_pr_key.as_deref(), "semantic-diff"),
        file.path
    );
    let revision = detail.updated_at.clone();

    if let Some(cached) = app_state
        .semantic_diff_cache
        .borrow()
        .get(&cache_key)
        .filter(|cached| cached.revision == revision)
        .cloned()
    {
        return cached.semantic;
    }

    let parsed = find_parsed_diff_file(&detail.parsed_diff, &file.path);
    let semantic = Arc::new(build_semantic_diff_file(
        file,
        parsed,
        &detail.review_threads,
    ));
    app_state.semantic_diff_cache.borrow_mut().insert(
        cache_key,
        CachedSemanticDiffFile {
            revision,
            semantic: semantic.clone(),
        },
    );
    semantic
}

fn render_review_sidebar_pane(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    _review_queue: &ReviewQueue,
    selected_path: Option<&str>,
    _semantic_file: Option<&SemanticDiffFile>,
    review_session: &crate::review_session::ReviewSessionState,
    review_stack: Arc<ReviewStack>,
    cx: &App,
) -> AnyElement {
    match review_session.center_mode {
        ReviewCenterMode::SemanticDiff => {
            render_changed_files_pane(state, detail, selected_path, cx).into_any_element()
        }
        ReviewCenterMode::SourceBrowser => {
            render_file_tree(state, detail, selected_path, cx).into_any_element()
        }
        ReviewCenterMode::AiTour => render_ai_tour_navigation_pane(state, cx).into_any_element(),
        ReviewCenterMode::Stack => {
            render_stack_navigation_pane(state, detail, review_stack, cx).into_any_element()
        }
    }
}

fn render_changed_files_pane(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    cx: &App,
) -> impl IntoElement {
    let list_state = {
        let app_state = state.read(cx);
        prepare_review_nav_list_state(&app_state)
    };
    let files = Arc::new(detail.files.clone());
    if list_state.item_count() != files.len() {
        list_state.reset(files.len());
    }

    let (additions, deletions) = detail
        .files
        .iter()
        .fold((0i64, 0i64), |(additions, deletions), file| {
            (additions + file.additions, deletions + file.deletions)
        });
    let selected_path = selected_path.map(str::to_string);

    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_r(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .border_b(px(1.0))
                .border_color(diff_annotation_border())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Changed Files"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family(mono_font_family())
                        .flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(div().text_color(fg_muted()).child(files.len().to_string()))
                        .child(div().text_color(fg_subtle()).child("/"))
                        .child(div().text_color(success()).child(format!("+{additions}")))
                        .child(div().text_color(fg_subtle()).child("/"))
                        .child(div().text_color(danger()).child(format!("-{deletions}"))),
                ),
        )
        .child(
            div()
                .id("changed-files-scroll")
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                .px(px(6.0))
                .py(px(6.0))
                .child(
                    list(list_state, {
                        let state = state.clone();
                        let files = files.clone();
                        let selected_path = selected_path.clone();
                        move |ix, _window, _cx| {
                            let file = files[ix].clone();
                            render_file_tree_file_row(
                                state.clone(),
                                file.path.clone(),
                                file.path,
                                file.additions,
                                file.deletions,
                                0,
                                selected_path.as_deref(),
                                ReviewFileRowOpenMode::Diff,
                            )
                            .into_any_element()
                        }
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                ),
        )
}

fn render_ai_tour_navigation_pane(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let (
        generated_tour,
        provider_loading,
        tour_loading,
        tour_generating,
        has_status_messages,
        center_list_state,
    ) = {
        let app_state = state.read(cx);
        let detail_state = app_state.active_detail_state();
        let tour_state = app_state.active_tour_state();

        (
            tour_state.and_then(|state| state.document.clone()),
            app_state.code_tour_provider_loading,
            tour_state.map(|state| state.loading).unwrap_or(false),
            tour_state.map(|state| state.generating).unwrap_or(false),
            app_state.code_tour_provider_error.is_some()
                || detail_state
                    .and_then(|state| state.local_repository_error.as_ref())
                    .is_some()
                || tour_state.and_then(|state| state.error.as_ref()).is_some()
                || tour_state
                    .and_then(|state| state.message.as_ref())
                    .is_some(),
            app_state.ai_tour_section_list_state.clone(),
        )
    };
    let nav_list_state = {
        let app_state = state.read(cx);
        prepare_review_nav_list_state(&app_state)
    };

    match generated_tour {
        Some(tour) => {
            let tour = Arc::new(tour);
            if nav_list_state.item_count() != tour.sections.len() {
                nav_list_state.reset(tour.sections.len());
            }

            let has_progress = provider_loading || tour_loading || tour_generating;
            let section_count = tour.sections.len();

            div()
                .w(file_tree_width())
                .flex_shrink_0()
                .min_h_0()
                .bg(diff_editor_chrome())
                .border_r(px(1.0))
                .border_color(diff_annotation_border())
                .flex()
                .flex_col()
                .child(render_sidebar_header(
                    "AI Tour",
                    "Semantic groups",
                    tour.sections.len().to_string(),
                ))
                .child(
                    div()
                        .id("ai-tour-nav-scroll")
                        .flex_grow()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .px(px(8.0))
                        .py(px(8.0))
                        .child(
                            list(nav_list_state, {
                                let tour = tour.clone();
                                let center_list_state = center_list_state.clone();
                                move |ix, _window, _cx| {
                                    let section = &tour.sections[ix];
                                    let target_index = ai_tour_section_content_index(
                                        section_count,
                                        has_progress,
                                        has_status_messages,
                                        ix,
                                    );
                                    render_ai_tour_nav_row(
                                        tour.as_ref(),
                                        section,
                                        ix,
                                        target_index,
                                        center_list_state.clone(),
                                    )
                                    .into_any_element()
                                }
                            })
                            .with_sizing_behavior(ListSizingBehavior::Auto)
                            .flex_grow()
                            .min_h_0(),
                        ),
                )
        }
        None => div()
            .w(file_tree_width())
            .flex_shrink_0()
            .min_h_0()
            .bg(diff_editor_chrome())
            .border_r(px(1.0))
            .border_color(diff_annotation_border())
            .flex()
            .flex_col()
            .child(render_sidebar_header(
                "AI Tour",
                "Semantic groups",
                "0".to_string(),
            ))
            .child(
                div()
                    .px(px(14.0))
                    .py(px(12.0))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(fg_muted())
                    .child("Generate an AI tour to navigate semantic groups here."),
            ),
    }
}

fn ai_tour_section_content_index(
    section_count: usize,
    has_progress: bool,
    has_status_messages: bool,
    section_ix: usize,
) -> usize {
    let mut item_ix = 0usize;
    if section_count > 0 {
        item_ix += 1;
    }
    if has_progress {
        item_ix += 1;
    }
    if has_status_messages {
        item_ix += 1;
    }
    item_ix + section_ix
}

fn render_ai_tour_nav_row(
    tour: &GeneratedCodeTour,
    section: &TourSection,
    section_ix: usize,
    target_index: usize,
    list_state: ListState,
) -> impl IntoElement {
    let metrics = ai_tour_section_metrics(tour, section);

    div()
        .mb(px(6.0))
        .px(px(8.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_surface())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).border_color(border_default()))
        .on_mouse_down(MouseButton::Left, move |_, _, _| {
            list_state.scroll_to(ListOffset {
                item_ix: target_index,
                offset_in_item: px(0.0),
            });
        })
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .min_w_0()
                .child(render_ai_tour_category_icon(section.category, 24.0, 13.0))
                .child(
                    div()
                        .min_w_0()
                        .flex_grow()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(format!("{:02}", section_ix + 1)),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .text_ellipsis()
                                        .child(section.title.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(fg_muted())
                                .line_clamp(2)
                                .child(section.summary.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .flex_wrap()
                                .child(ai_tour_metric_chip(&format!(
                                    "{} file{}",
                                    metrics.file_count,
                                    if metrics.file_count == 1 { "" } else { "s" }
                                )))
                                .child(ai_tour_metric_chip(&format!(
                                    "+{} / -{}",
                                    metrics.additions, metrics.deletions
                                )))
                                .child(render_ai_tour_priority_chip(section.priority)),
                        ),
                ),
        )
}

fn render_stack_navigation_pane(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    review_stack: Arc<ReviewStack>,
    cx: &App,
) -> impl IntoElement {
    let session = state
        .read(cx)
        .active_review_session()
        .cloned()
        .unwrap_or_default();
    let selected_layer_id = review_stack
        .selected_layer(session.selected_stack_layer_id.as_deref())
        .map(|layer| layer.id.clone());
    let reviewed_layer_ids = session.reviewed_stack_layer_ids.clone();
    let list_state = {
        let app_state = state.read(cx);
        prepare_review_nav_list_state(&app_state)
    };
    if list_state.item_count() != review_stack.layers.len() {
        list_state.reset(review_stack.layers.len());
    }

    let stack_kind = match review_stack.kind {
        StackKind::Real => "Real stack",
        StackKind::Virtual => "Virtual stack",
    };
    let source_label = review_stack.source.label();
    let is_ai_generated = review_stack.source == StackSource::VirtualAi;
    let stack_warning = review_stack
        .warnings
        .first()
        .map(|warning| warning.message.clone());

    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_r(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(render_sidebar_header(
            "Stack",
            stack_kind,
            review_stack.layers.len().to_string(),
        ))
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .border_b(px(1.0))
                .border_color(diff_annotation_border())
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(badge(source_label))
                        .child(badge(session.stack_diff_mode.label()))
                        .when(is_ai_generated, |el| el.child(badge("AI generated"))),
                )
                .when_some(stack_warning, |el, warning_message| {
                    el.child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(if is_ai_generated {
                                fg_muted()
                            } else {
                                warning()
                            })
                            .line_clamp(2)
                            .child(warning_message),
                    )
                }),
        )
        .child(
            div()
                .id("stack-nav-scroll")
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                .px(px(8.0))
                .py(px(8.0))
                .child(
                    list(list_state, {
                        let state = state.clone();
                        let detail = Arc::new(detail.clone());
                        let review_stack = review_stack.clone();
                        let selected_layer_id = selected_layer_id.clone();
                        let reviewed_layer_ids = reviewed_layer_ids.clone();
                        move |ix, _window, _cx| {
                            let layer = &review_stack.layers[ix];
                            render_stack_layer_row(
                                &state,
                                detail.as_ref(),
                                review_stack.as_ref(),
                                layer,
                                selected_layer_id.as_deref() == Some(layer.id.as_str()),
                                reviewed_layer_ids.contains(&layer.id),
                            )
                            .into_any_element()
                        }
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                ),
        )
}

fn render_sidebar_header(title: &str, subtitle: &str, count: String) -> impl IntoElement {
    div()
        .px(px(12.0))
        .py(px(10.0))
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(fg_muted())
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(subtitle.to_string()),
                ),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_family(mono_font_family())
                .text_color(fg_muted())
                .child(count),
        )
}

fn render_legacy_review_navigation_pane(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    review_queue: &ReviewQueue,
    selected_path: Option<&str>,
    semantic_file: Option<&SemanticDiffFile>,
    review_session: &crate::review_session::ReviewSessionState,
    cx: &App,
) -> impl IntoElement {
    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_r(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(render_review_navigation_content(
            state,
            detail,
            review_queue,
            selected_path,
            semantic_file,
            review_session,
            cx,
        ))
}

fn render_review_navigation_content(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    review_queue: &ReviewQueue,
    selected_path: Option<&str>,
    semantic_file: Option<&SemanticDiffFile>,
    review_session: &crate::review_session::ReviewSessionState,
    cx: &App,
) -> impl IntoElement {
    let list_state = {
        let app_state = state.read(cx);
        prepare_review_nav_list_state(&app_state)
    };
    let selected_path = selected_path.map(str::to_string);
    let outline_path = selected_path.clone().unwrap_or_default();
    let nav_items = Arc::new(build_review_nav_items(
        detail,
        review_queue,
        semantic_file,
        review_session,
    ));
    if list_state.item_count() != nav_items.len() {
        list_state.reset(nav_items.len());
    }

    div()
        .flex_grow()
        .min_h_0()
        .flex()
        .flex_col()
        .id("review-nav-scroll")
        .child(
            list(list_state, {
                let state = state.clone();
                let nav_items = nav_items.clone();
                let selected_path = selected_path.clone();
                let outline_path = outline_path.clone();
                move |ix, _window, cx| {
                    render_review_nav_list_item(
                        &state,
                        &nav_items[ix],
                        selected_path.as_deref(),
                        outline_path.as_str(),
                        cx,
                    )
                }
            })
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .flex_grow()
            .min_h_0(),
        )
}

#[derive(Clone, Debug)]
enum ReviewNavListItem {
    QueueHeader {
        changed_files: i64,
    },
    QueueBucketHeader {
        bucket: ReviewQueueBucket,
        count: usize,
    },
    QueueRow(crate::review_queue::ReviewQueueItem),
    ChangedFilesHeader {
        count: usize,
    },
    ChangedFile(PullRequestFile),
    SemanticHeader {
        count: usize,
    },
    SemanticSection(SemanticDiffSection),
    TaskRouteHeader {
        title: String,
        count: usize,
    },
    TaskRouteStop {
        index: usize,
        location: ReviewLocation,
    },
    WaymarksHeader {
        title: String,
        count: usize,
    },
    Waymark(crate::review_session::ReviewWaymark),
    RecentLocation(ReviewLocation),
    Spacer,
}

fn prepare_review_nav_list_state(app_state: &AppState) -> ListState {
    let mode_key = app_state
        .active_review_session()
        .map(|session| session.center_mode.label())
        .unwrap_or("Diff");
    let state_key = format!(
        "{}:review-nav:{mode_key}",
        app_state.active_pr_key.as_deref().unwrap_or("detached"),
    );
    let mut list_states = app_state.review_nav_list_states.borrow_mut();
    list_states
        .entry(state_key)
        .or_insert_with(|| ListState::new(0, ListAlignment::Top, px(96.0)))
        .clone()
}

fn build_review_nav_items(
    detail: &PullRequestDetail,
    review_queue: &ReviewQueue,
    semantic_file: Option<&SemanticDiffFile>,
    review_session: &crate::review_session::ReviewSessionState,
) -> Vec<ReviewNavListItem> {
    let mut items = Vec::new();

    items.push(ReviewNavListItem::QueueHeader {
        changed_files: detail.changed_files,
    });
    append_review_nav_bucket(
        &mut items,
        ReviewQueueBucket::StartHere,
        &review_queue.start_here,
    );
    append_review_nav_bucket(
        &mut items,
        ReviewQueueBucket::NeedsScrutiny,
        &review_queue.needs_scrutiny,
    );
    append_review_nav_bucket(
        &mut items,
        ReviewQueueBucket::QuickPass,
        &review_queue.quick_pass,
    );

    if !detail.files.is_empty() {
        items.push(ReviewNavListItem::Spacer);
        items.push(ReviewNavListItem::ChangedFilesHeader {
            count: detail.files.len(),
        });
        items.extend(
            detail
                .files
                .iter()
                .cloned()
                .map(ReviewNavListItem::ChangedFile),
        );
    }

    if let Some(semantic_file) = semantic_file {
        items.push(ReviewNavListItem::Spacer);
        items.push(ReviewNavListItem::SemanticHeader {
            count: semantic_file.sections.len(),
        });
        items.extend(
            semantic_file
                .sections
                .iter()
                .cloned()
                .map(ReviewNavListItem::SemanticSection),
        );
    }

    if let Some(task_route) = review_session.task_route.as_ref() {
        items.push(ReviewNavListItem::Spacer);
        items.push(ReviewNavListItem::TaskRouteHeader {
            title: task_route.title.clone(),
            count: task_route.stops.len(),
        });
        items.extend(
            task_route
                .stops
                .iter()
                .enumerate()
                .map(|(index, location)| ReviewNavListItem::TaskRouteStop {
                    index,
                    location: location.clone(),
                }),
        );
    }

    if !review_session.waymarks.is_empty() {
        items.push(ReviewNavListItem::Spacer);
        items.push(ReviewNavListItem::WaymarksHeader {
            title: "Waypoints".to_string(),
            count: review_session.waymarks.len(),
        });
        items.extend(
            review_session
                .waymarks
                .iter()
                .cloned()
                .map(ReviewNavListItem::Waymark),
        );
    }

    if !review_session.route.is_empty() {
        items.push(ReviewNavListItem::Spacer);
        items.push(ReviewNavListItem::WaymarksHeader {
            title: "Recent Route".to_string(),
            count: review_session.route.len(),
        });
        items.extend(
            review_session
                .route
                .iter()
                .cloned()
                .map(ReviewNavListItem::RecentLocation),
        );
    }

    items.push(ReviewNavListItem::Spacer);
    items
}

fn append_review_nav_bucket(
    items: &mut Vec<ReviewNavListItem>,
    bucket: ReviewQueueBucket,
    bucket_items: &[crate::review_queue::ReviewQueueItem],
) {
    if bucket_items.is_empty() {
        return;
    }

    items.push(ReviewNavListItem::QueueBucketHeader {
        bucket,
        count: bucket_items.len(),
    });
    items.extend(
        bucket_items
            .iter()
            .cloned()
            .map(ReviewNavListItem::QueueRow),
    );
}

fn render_review_nav_list_item(
    state: &Entity<AppState>,
    item: &ReviewNavListItem,
    selected_path: Option<&str>,
    outline_path: &str,
    cx: &App,
) -> AnyElement {
    match item {
        ReviewNavListItem::QueueHeader { changed_files } => div()
            .px(px(14.0))
            .pt(px(14.0))
            .child(render_review_nav_panel_header(
                "REVIEW QUEUE",
                "Prioritized pass",
                changed_files.to_string(),
            ))
            .into_any_element(),
        ReviewNavListItem::QueueBucketHeader { bucket, count } => div()
            .px(px(14.0))
            .pt(px(10.0))
            .child(render_review_nav_bucket_header(*bucket, *count))
            .into_any_element(),
        ReviewNavListItem::QueueRow(queue_item) => div()
            .px(px(14.0))
            .pt(px(8.0))
            .child(render_review_queue_row(
                state,
                queue_item,
                selected_path == Some(queue_item.file_path.as_str()),
            ))
            .into_any_element(),
        ReviewNavListItem::ChangedFilesHeader { count } => div()
            .px(px(14.0))
            .pt(px(14.0))
            .child(render_review_nav_panel_header(
                "CHANGED FILES",
                "Whole PR",
                count.to_string(),
            ))
            .into_any_element(),
        ReviewNavListItem::ChangedFile(file) => div()
            .px(px(8.0))
            .pt(px(4.0))
            .child(render_file_tree_file_row(
                state.clone(),
                file.path.clone(),
                file.path
                    .rsplit('/')
                    .next()
                    .unwrap_or(file.path.as_str())
                    .to_string(),
                file.additions,
                file.deletions,
                0,
                selected_path,
                ReviewFileRowOpenMode::Diff,
            ))
            .into_any_element(),
        ReviewNavListItem::SemanticHeader { count } => div()
            .px(px(14.0))
            .pt(px(14.0))
            .child(render_review_nav_panel_header(
                "SYMBOL OUTLINE",
                "Semantic sections",
                count.to_string(),
            ))
            .into_any_element(),
        ReviewNavListItem::SemanticSection(section) => div()
            .px(px(14.0))
            .pt(px(8.0))
            .child(render_semantic_outline_row(
                state,
                outline_path,
                section,
                cx,
            ))
            .into_any_element(),
        ReviewNavListItem::TaskRouteHeader { title, count } => div()
            .px(px(14.0))
            .pt(px(14.0))
            .child(render_review_nav_panel_header(
                "TASK ROUTE",
                title,
                count.to_string(),
            ))
            .into_any_element(),
        ReviewNavListItem::TaskRouteStop { index, location } => div()
            .px(px(14.0))
            .pt(px(8.0))
            .child(render_task_route_stop_row(state, *index, location))
            .into_any_element(),
        ReviewNavListItem::WaymarksHeader { title, count } => div()
            .px(px(14.0))
            .pt(px(14.0))
            .child(render_review_nav_panel_header(
                &title.to_ascii_uppercase(),
                title,
                count.to_string(),
            ))
            .into_any_element(),
        ReviewNavListItem::Waymark(waymark) => div()
            .px(px(14.0))
            .pt(px(8.0))
            .child(render_waymark_row(state, waymark))
            .into_any_element(),
        ReviewNavListItem::RecentLocation(location) => div()
            .px(px(14.0))
            .pt(px(8.0))
            .child(render_recent_location_row(state, location))
            .into_any_element(),
        ReviewNavListItem::Spacer => div().h(px(6.0)).into_any_element(),
    }
}

fn render_review_nav_panel_header(
    eyebrow_label: &str,
    title: &str,
    count: String,
) -> impl IntoElement {
    nested_panel().child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_family(mono_font_family())
                            .text_color(fg_subtle())
                            .child(eyebrow_label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child(title.to_string()),
                    ),
            )
            .child(badge(&count)),
    )
}

fn render_review_nav_bucket_header(bucket: ReviewQueueBucket, count: usize) -> impl IntoElement {
    div()
        .pt(px(10.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_family(mono_font_family())
                        .text_color(fg_subtle())
                        .child(bucket.label().to_ascii_uppercase()),
                )
                .child(badge(&count.to_string())),
        )
}

fn render_review_queue_row(
    state: &Entity<AppState>,
    item: &crate::review_queue::ReviewQueueItem,
    is_selected: bool,
) -> impl IntoElement {
    let path = item.file_path.clone();
    let anchor = item.anchor.clone();
    let state = state.clone();

    div()
        .px(px(10.0))
        .py(px(9.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if is_selected {
            border_default()
        } else {
            border_muted()
        })
        .bg(if is_selected {
            bg_selected()
        } else {
            bg_surface()
        })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_review_diff_location(&state, path.clone(), anchor.clone(), window, cx);
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(item.file_path.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(fg_muted())
                                .child(item.reasons.join(" • ")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(accent())
                        .child(item.risk_label.clone()),
                ),
        )
        .child(
            div()
                .mt(px(8.0))
                .flex()
                .gap(px(6.0))
                .flex_wrap()
                .child(render_change_type_chip(&item.change_type))
                .child(queue_metric(
                    format!("+{}", item.additions),
                    success(),
                    success_muted(),
                ))
                .child(queue_metric(
                    format!("-{}", item.deletions),
                    danger(),
                    danger_muted(),
                ))
                .when(item.thread_count > 0, |el| {
                    el.child(queue_metric(
                        format!(
                            "{} thread{}",
                            item.thread_count,
                            if item.thread_count == 1 { "" } else { "s" }
                        ),
                        accent(),
                        accent_muted(),
                    ))
                }),
        )
}

fn render_semantic_outline_row(
    state: &Entity<AppState>,
    selected_path: &str,
    section: &SemanticDiffSection,
    cx: &App,
) -> impl IntoElement {
    let state_for_open = state.clone();
    let state_for_toggle = state.clone();
    let path = selected_path.to_string();
    let anchor = section.anchor.clone();
    let section_id = section.id.clone();
    let collapsed = state.read(cx).is_review_section_collapsed(&section.id);

    div()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w_0()
                        .cursor_pointer()
                        .hover(|style| style.text_color(fg_emphasis()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            open_review_diff_location(
                                &state_for_open,
                                path.clone(),
                                anchor.clone(),
                                window,
                                cx,
                            );
                        })
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(section.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(fg_muted())
                                .child(section.summary.clone()),
                        ),
                )
                .child(ghost_button(
                    if collapsed { "Expand" } else { "Fold" },
                    move |_, _, cx| {
                        state_for_toggle.update(cx, |state, cx| {
                            state.toggle_review_section_collapse(&section_id);
                            state.persist_active_review_session();
                            cx.notify();
                        });
                    },
                )),
        )
}

fn render_task_route_stop_row(
    state: &Entity<AppState>,
    index: usize,
    location: &ReviewLocation,
) -> impl IntoElement {
    let state = state.clone();
    let location = location.clone();
    let location_for_open = location.clone();

    div()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_review_location_card(&state, &location_for_open, window, cx);
        })
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(queue_metric(
                    format!("{:02}", index + 1),
                    accent(),
                    accent_muted(),
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(location.label.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(fg_muted())
                                .child(location.mode.label()),
                        ),
                ),
        )
}

fn render_recent_location_row(
    state: &Entity<AppState>,
    location: &ReviewLocation,
) -> impl IntoElement {
    let state = state.clone();
    let location = location.clone();
    let location_for_open = location.clone();

    div()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_review_location_card(&state, &location_for_open, window, cx);
        })
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg_emphasis())
                .child(location.label.clone()),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(11.0))
                .text_color(fg_muted())
                .child(location.mode.label()),
        )
}

fn render_waymark_row(
    state: &Entity<AppState>,
    waymark: &crate::review_session::ReviewWaymark,
) -> impl IntoElement {
    let state = state.clone();
    let location = waymark.location.clone();
    let location_for_open = location.clone();
    let waymark_name = waymark.name.clone();

    div()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_review_location_card(&state, &location_for_open, window, cx);
        })
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg_emphasis())
                .child(waymark_name),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(11.0))
                .text_color(fg_muted())
                .child(location.label.clone()),
        )
}

fn open_review_location_card(
    state: &Entity<AppState>,
    location: &ReviewLocation,
    window: &mut Window,
    cx: &mut App,
) {
    match location.mode {
        ReviewCenterMode::SemanticDiff => open_review_diff_location(
            state,
            location.file_path.clone(),
            location.anchor.clone(),
            window,
            cx,
        ),
        ReviewCenterMode::AiTour => {
            state.update(cx, |state, cx| {
                state.navigate_to_review_location(location.clone(), true);
                state.persist_active_review_session();
                cx.notify();
            });
            ensure_active_review_focus_loaded(state, window, cx);
            refresh_active_tour(state, window, cx, true);
        }
        ReviewCenterMode::Stack => {
            state.update(cx, |state, cx| {
                state.navigate_to_review_location(location.clone(), true);
                state.persist_active_review_session();
                cx.notify();
            });
            ensure_active_review_focus_loaded(state, window, cx);
        }
        ReviewCenterMode::SourceBrowser => open_review_source_location(
            state,
            location.file_path.clone(),
            location.source_line,
            location.source_reason.clone(),
            window,
            cx,
        ),
    }
}

fn default_waymark_name(
    selected_file_path: Option<&str>,
    selected_section: Option<&SemanticDiffSection>,
    selected_anchor: Option<&DiffAnchor>,
) -> String {
    if let Some(section) = selected_section {
        return format!("Check {}", section.title);
    }

    if let Some(line) = selected_anchor
        .and_then(|anchor| anchor.line)
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0)
    {
        if let Some(path) = selected_file_path {
            return format!("{path}:{line}");
        }
    }

    selected_file_path
        .map(|path| format!("Review {path}"))
        .unwrap_or_else(|| "Waypoint".to_string())
}

fn metric_pill(label: impl Into<String>, fg: gpui::Rgba, bg: gpui::Rgba) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(border_muted())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .font_family(mono_font_family())
        .text_color(fg)
        .child(label.into())
}

fn queue_metric(label: String, fg: gpui::Rgba, bg: gpui::Rgba) -> impl IntoElement {
    metric_pill(label, fg, bg)
}

fn render_stack_rail(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    stack: &ReviewStack,
    cx: &App,
) -> impl IntoElement {
    let session = state
        .read(cx)
        .active_review_session()
        .cloned()
        .unwrap_or_default();
    let stack_can_expand = stack.layers.len() > 1;
    let stack_rail_expanded = session.stack_rail_expanded && stack_can_expand;
    let selected_layer = stack
        .selected_layer(session.selected_stack_layer_id.as_deref())
        .cloned();
    let selected_layer_id = selected_layer.as_ref().map(|layer| layer.id.clone());
    let reviewed_layer_ids = session.reviewed_stack_layer_ids.clone();
    let ai_stack_unavailable = stack
        .warnings
        .iter()
        .any(|warning| warning.code == "ai-virtual-stack-unavailable");
    let stack_label = match (&stack.kind, stack.source) {
        (crate::stacks::model::StackKind::Real, _) => "Real stack".to_string(),
        (
            crate::stacks::model::StackKind::Virtual,
            crate::stacks::model::StackSource::VirtualAi,
        ) if ai_stack_unavailable => "AI stack unavailable".to_string(),
        (
            crate::stacks::model::StackKind::Virtual,
            crate::stacks::model::StackSource::VirtualAi,
        ) => format!(
            "Virtual stack · AI-assisted · {} layers",
            stack.layers.len()
        ),
        (crate::stacks::model::StackKind::Virtual, _) => "Virtual stack".to_string(),
    };
    let source_label = match (&stack.kind, stack.source) {
        (crate::stacks::model::StackKind::Real, _) => "Backed by GitHub PRs",
        (
            crate::stacks::model::StackKind::Virtual,
            crate::stacks::model::StackSource::VirtualAi,
        ) if ai_stack_unavailable => "No non-AI stack was generated",
        (
            crate::stacks::model::StackKind::Virtual,
            crate::stacks::model::StackSource::VirtualAi,
        ) => "Generated locally by Remiss as a review lens, not GitHub PRs",
        (crate::stacks::model::StackKind::Virtual, _) => "Generated locally by Remiss",
    };
    let stack_refs_loading = state
        .read(cx)
        .active_detail_state()
        .map(|detail_state| detail_state.stack_open_pull_requests_loading)
        .unwrap_or(false);
    let stack_refs_error = state
        .read(cx)
        .active_detail_state()
        .and_then(|detail_state| detail_state.stack_open_pull_requests_error.clone());
    let ai_stack_state = state
        .read(cx)
        .active_detail_state()
        .map(|detail_state| detail_state.ai_stack_state.clone())
        .unwrap_or_default();
    let review_intelligence_loading = state
        .read(cx)
        .active_detail_state()
        .map(|detail_state| detail_state.review_intelligence_loading)
        .unwrap_or(false);
    let ai_stack_busy = ai_stack_state.loading || ai_stack_state.generating;
    let show_stack_retry =
        ai_stack_state.error.is_some() && !ai_stack_busy && !review_intelligence_loading;
    let stack_warning = stack
        .warnings
        .first()
        .map(|warning| warning.message.clone());
    let state_for_stack_retry = state.clone();
    let state_for_stack_toggle = state.clone();

    div()
        .px(px(8.0))
        .py(px(8.0))
        .border_b(px(1.0))
        .border_color(border_muted())
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .px(px(4.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(stack_label),
                        )
                        .child(
                            div()
                                .mt(px(2.0))
                                .text_size(px(10.0))
                                .text_color(fg_muted())
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(source_label),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .items_center()
                        .child(badge(&stack.layers.len().to_string()))
                        .when(stack_refs_loading, |el| el.child(badge("Checking")))
                        .when(ai_stack_busy, |el| el.child(badge("Generating")))
                        .when(stack_can_expand, |el| {
                            el.child(render_stack_rail_toggle(
                                state_for_stack_toggle,
                                stack_rail_expanded,
                            ))
                        }),
                ),
        )
        .when(stack_rail_expanded, |el| {
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .id("stack-layer-scroll")
                    .overflow_y_scroll()
                    .max_h(px(280.0))
                    .children(stack.layers.iter().map(|layer| {
                        render_stack_layer_row(
                            state,
                            detail,
                            stack,
                            layer,
                            selected_layer_id.as_deref() == Some(layer.id.as_str()),
                            reviewed_layer_ids.contains(&layer.id),
                        )
                    })),
            )
        })
        .when(!stack_rail_expanded, |el| {
            if let Some(layer) = selected_layer.as_ref() {
                el.child(render_stack_layer_summary_row(
                    state,
                    layer,
                    stack_can_expand,
                    reviewed_layer_ids.contains(&layer.id),
                ))
            } else {
                el
            }
        })
        .when_some(stack_warning, |el, warning_message| {
            el.child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(radius_sm())
                    .bg(warning_muted())
                    .border_1()
                    .border_color(warning())
                    .child(
                        div()
                            .min_w_0()
                            .flex_grow()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(fg_emphasis())
                            .child(warning_message),
                    )
                    .when(show_stack_retry, |el| {
                        el.child(div().flex_shrink_0().child(review_button(
                            "Retry stack",
                            move |_, window, cx| {
                                review_intelligence::trigger_review_intelligence(
                                    &state_for_stack_retry,
                                    window,
                                    cx,
                                    review_intelligence::ReviewIntelligenceScope::StackOnly,
                                    true,
                                );
                            },
                        )))
                    }),
            )
        })
        .when_some(stack_refs_error, |el, error| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(radius_sm())
                    .bg(bg_subtle())
                    .border_1()
                    .border_color(border_muted())
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(fg_muted())
                    .child(format!("Real stack lookup unavailable: {error}")),
            )
        })
}

fn render_stack_rail_toggle(state: Entity<AppState>, expanded: bool) -> impl IntoElement {
    toolbar_icon_button(
        "stack-rail-toggle",
        if expanded {
            "Hide stack tree"
        } else {
            "Show stack tree"
        },
        expanded,
        false,
        render_stack_tree_toggle_icon(expanded),
        move |_, _, cx| {
            state.update(cx, |state, cx| {
                state.set_stack_rail_expanded(!expanded);
                state.persist_active_review_session();
                cx.notify();
            });
        },
    )
}

fn render_stack_layer_summary_row(
    state: &Entity<AppState>,
    layer: &ReviewStackLayer,
    can_expand: bool,
    is_reviewed: bool,
) -> impl IntoElement {
    let state_for_open = state.clone();
    let line_count = layer.metrics.changed_lines;
    let thread_count = layer.metrics.unresolved_thread_count;
    let confidence = layer.confidence;

    div()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(focus_border())
        .bg(bg_selected())
        .when(can_expand, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    state_for_open.update(cx, |state, cx| {
                        state.set_stack_rail_expanded(true);
                        state.persist_active_review_session();
                        cx.notify();
                    });
                })
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w_0()
                        .child(
                            div()
                                .w(px(18.0))
                                .h(px(18.0))
                                .rounded(radius_sm())
                                .bg(accent_muted())
                                .border_1()
                                .border_color(accent())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(10.0))
                                .font_family(mono_font_family())
                                .text_color(accent())
                                .child((layer.index + 1).to_string()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(layer.title.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(if is_reviewed { success() } else { fg_subtle() })
                        .child(if is_reviewed {
                            "done"
                        } else {
                            layer.status.label()
                        }),
                ),
        )
        .child(
            div()
                .mt(px(6.0))
                .flex()
                .gap(px(4.0))
                .flex_wrap()
                .child(subtle_stack_chip(&format!(
                    "{} file{}",
                    layer.metrics.file_count,
                    if layer.metrics.file_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                )))
                .child(subtle_stack_chip(&format!(
                    "{line_count} line{}",
                    if line_count == 1 { "" } else { "s" }
                )))
                .when(thread_count > 0, |el| {
                    el.child(subtle_stack_chip(&format!(
                        "{thread_count} thread{}",
                        if thread_count == 1 { "" } else { "s" }
                    )))
                })
                .when(confidence != Confidence::High, |el| {
                    el.child(subtle_stack_chip(match confidence {
                        Confidence::High => "high",
                        Confidence::Medium => "medium",
                        Confidence::Low => "low",
                    }))
                })
                .when(can_expand, |el| el.child(subtle_stack_chip("Open stack"))),
        )
}

fn render_stack_layer_row(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    stack: &ReviewStack,
    layer: &ReviewStackLayer,
    is_active: bool,
    is_reviewed: bool,
) -> impl IntoElement {
    let state_for_open = state.clone();
    let layer_id = layer.id.clone();
    let first_file = stack.first_file_for_layer(layer);
    let line_count = layer.metrics.changed_lines;
    let thread_count = layer.metrics.unresolved_thread_count;
    let confidence = layer.confidence;
    let is_current_pr_layer = layer
        .pr
        .as_ref()
        .map(|pr| pr.number == detail.number)
        .unwrap_or(true);

    div()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if is_active {
            focus_border()
        } else {
            border_muted()
        })
        .bg(if is_active {
            bg_selected()
        } else {
            bg_surface()
        })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            state_for_open.update(cx, |state, cx| {
                state.set_selected_stack_layer(Some(layer_id.clone()));
                state.set_stack_diff_mode(StackDiffMode::CurrentLayerOnly);
                state.set_review_center_mode(ReviewCenterMode::Stack);
                if is_current_pr_layer {
                    if let Some(path) = first_file.clone() {
                        state.selected_file_path = Some(path);
                        state.selected_diff_anchor = None;
                    }
                }
                state.persist_active_review_session();
                cx.notify();
            });
            if is_current_pr_layer {
                ensure_selected_file_content_loaded(&state_for_open, window, cx);
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w_0()
                        .child(
                            div()
                                .w(px(18.0))
                                .h(px(18.0))
                                .rounded(radius_sm())
                                .bg(if is_active {
                                    accent_muted()
                                } else {
                                    bg_subtle()
                                })
                                .border_1()
                                .border_color(if is_active { accent() } else { border_muted() })
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(10.0))
                                .font_family(mono_font_family())
                                .text_color(if is_active { accent() } else { fg_muted() })
                                .child((layer.index + 1).to_string()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if is_active {
                                    fg_emphasis()
                                } else {
                                    fg_default()
                                })
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(layer.title.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(if is_reviewed { success() } else { fg_subtle() })
                        .child(if is_reviewed {
                            "done"
                        } else {
                            layer.status.label()
                        }),
                ),
        )
        .child(
            div()
                .mt(px(6.0))
                .flex()
                .gap(px(4.0))
                .flex_wrap()
                .child(subtle_stack_chip(&format!(
                    "{} file{}",
                    layer.metrics.file_count,
                    if layer.metrics.file_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                )))
                .child(subtle_stack_chip(&format!(
                    "{line_count} line{}",
                    if line_count == 1 { "" } else { "s" }
                )))
                .when(thread_count > 0, |el| {
                    el.child(subtle_stack_chip(&format!(
                        "{thread_count} thread{}",
                        if thread_count == 1 { "" } else { "s" }
                    )))
                })
                .when(confidence != Confidence::High, |el| {
                    el.child(subtle_stack_chip(match confidence {
                        Confidence::High => "high",
                        Confidence::Medium => "medium",
                        Confidence::Low => "low",
                    }))
                }),
        )
}

fn subtle_stack_chip(label: &str) -> impl IntoElement {
    div()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(999.0))
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .text_size(px(9.0))
        .font_family(mono_font_family())
        .text_color(fg_muted())
        .child(label.to_string())
}

fn render_file_tree(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    cx: &App,
) -> impl IntoElement {
    let (tree_rows, file_tree_label, visible_file_count, visible_additions, visible_deletions) = {
        let app_state = state.read(cx);
        let (file_count, additions, deletions) = review_file_tree_totals(detail, None);
        let label = "Files".to_string();

        (
            prepare_review_file_tree_rows(&app_state, detail, None),
            label,
            file_count,
            additions,
            deletions,
        )
    };
    let list_state = {
        let app_state = state.read(cx);
        prepare_review_file_tree_list_state(&app_state)
    };
    if list_state.item_count() != tree_rows.len() {
        list_state.reset(tree_rows.len());
    }
    let selected_path = selected_path.map(str::to_string);

    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_r(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .border_b(px(1.0))
                .border_color(diff_annotation_border())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(file_tree_label),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family(mono_font_family())
                        .flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .text_color(fg_muted())
                                .child(visible_file_count.to_string()),
                        )
                        .child(div().text_color(fg_subtle()).child("\u{2022}"))
                        .child(
                            div()
                                .text_color(success())
                                .child(format!("+{visible_additions}")),
                        )
                        .child(div().text_color(fg_subtle()).child("/"))
                        .child(
                            div()
                                .text_color(danger())
                                .child(format!("-{visible_deletions}")),
                        ),
                ),
        )
        .child(
            div()
                .id("file-tree-scroll")
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                .px(px(6.0))
                .py(px(6.0))
                .child(
                    list(list_state, {
                        let state = state.clone();
                        let tree_rows = tree_rows.clone();
                        let selected_path = selected_path.clone();
                        move |ix, _window, _cx| match tree_rows[ix].clone() {
                            ReviewFileTreeRow::Directory { name, depth } => {
                                render_file_tree_directory_row(name, depth).into_any_element()
                            }
                            ReviewFileTreeRow::File {
                                path,
                                name,
                                depth,
                                additions,
                                deletions,
                            } => render_file_tree_file_row(
                                state.clone(),
                                path,
                                name,
                                additions,
                                deletions,
                                depth,
                                selected_path.as_deref(),
                                ReviewFileRowOpenMode::Source,
                            )
                            .into_any_element(),
                        }
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                ),
        )
}

const REVIEW_FILE_TREE_ROW_HEIGHT: f32 = 30.0;

fn prepare_review_file_tree_list_state(app_state: &AppState) -> ListState {
    let key = review_cache_key(app_state.active_pr_key.as_deref(), "review-file-tree");
    let mut list_states = app_state.review_file_tree_list_states.borrow_mut();
    list_states
        .entry(key)
        .or_insert_with(|| ListState::new(0, ListAlignment::Top, px(REVIEW_FILE_TREE_ROW_HEIGHT)))
        .clone()
}

fn prepare_review_file_tree_rows(
    app_state: &AppState,
    detail: &PullRequestDetail,
    visible_paths: Option<&BTreeSet<String>>,
) -> Arc<Vec<ReviewFileTreeRow>> {
    let cache_key = review_cache_key(
        app_state.active_pr_key.as_deref(),
        &review_file_tree_cache_scope(visible_paths),
    );
    let revision = detail.updated_at.clone();

    if let Some(cached) = app_state
        .review_file_tree_cache
        .borrow()
        .get(&cache_key)
        .filter(|cached| cached.revision == revision)
        .cloned()
    {
        return cached.rows;
    }

    let rows = Arc::new(build_review_file_tree_rows(detail, visible_paths));
    app_state.review_file_tree_cache.borrow_mut().insert(
        cache_key,
        CachedReviewFileTree {
            revision,
            rows: rows.clone(),
        },
    );
    rows
}

fn review_file_tree_cache_scope(visible_paths: Option<&BTreeSet<String>>) -> String {
    match visible_paths {
        None => "review-file-tree-rows:all".to_string(),
        Some(paths) => {
            let mut hasher = DefaultHasher::new();
            paths.hash(&mut hasher);
            format!(
                "review-file-tree-rows:stack-filter:{}:{:x}",
                paths.len(),
                hasher.finish()
            )
        }
    }
}

fn review_file_tree_label(mode: StackDiffMode) -> &'static str {
    match mode {
        StackDiffMode::WholePr => "Files",
        StackDiffMode::CurrentLayerOnly => "Layer Files",
        StackDiffMode::UpToCurrentLayer => "Stack Files",
        StackDiffMode::CurrentAndDependents => "Dependent Files",
        StackDiffMode::SinceLastReviewed => "Unreviewed Files",
    }
}

fn stack_file_paths_for_filter(
    review_stack: &ReviewStack,
    filter: &LayerDiffFilter,
) -> BTreeSet<String> {
    filter
        .visible_atom_ids
        .iter()
        .filter_map(|atom_id| review_stack.atom(atom_id))
        .filter(|atom| !atom.path.is_empty())
        .map(|atom| atom.path.clone())
        .collect()
}

fn review_file_tree_totals(
    detail: &PullRequestDetail,
    visible_paths: Option<&BTreeSet<String>>,
) -> (usize, i64, i64) {
    detail
        .files
        .iter()
        .filter(|file| {
            visible_paths
                .map(|paths| paths.contains(&file.path))
                .unwrap_or(true)
        })
        .fold(
            (0usize, 0i64, 0i64),
            |(count, additions, deletions), file| {
                (
                    count + 1,
                    additions + file.additions,
                    deletions + file.deletions,
                )
            },
        )
}

#[derive(Default)]
struct ReviewFileTreeNode {
    name: String,
    children: std::collections::BTreeMap<String, ReviewFileTreeNode>,
    files: Vec<ReviewFileTreeRow>,
}

fn build_review_file_tree_rows(
    detail: &PullRequestDetail,
    visible_paths: Option<&BTreeSet<String>>,
) -> Vec<ReviewFileTreeRow> {
    let mut root = ReviewFileTreeNode::default();
    for file in &detail.files {
        if visible_paths
            .map(|paths| !paths.contains(&file.path))
            .unwrap_or(false)
        {
            continue;
        }

        let mut cursor = &mut root;
        let mut segments = file.path.split('/').peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_some() {
                cursor = cursor
                    .children
                    .entry(segment.to_string())
                    .or_insert_with(|| ReviewFileTreeNode {
                        name: segment.to_string(),
                        ..ReviewFileTreeNode::default()
                    });
            } else {
                cursor.files.push(ReviewFileTreeRow::File {
                    path: file.path.clone(),
                    name: segment.to_string(),
                    depth: 0,
                    additions: file.additions,
                    deletions: file.deletions,
                });
            }
        }
    }

    let mut rows = Vec::new();
    flatten_review_file_tree(&root, 0, &mut rows);
    rows
}

fn flatten_review_file_tree(
    node: &ReviewFileTreeNode,
    depth: usize,
    rows: &mut Vec<ReviewFileTreeRow>,
) {
    if depth > 0 {
        rows.push(ReviewFileTreeRow::Directory {
            name: node.name.clone(),
            depth,
        });
    }

    for child in node.children.values() {
        flatten_review_file_tree(child, depth + 1, rows);
    }

    let file_depth = if depth == 0 { 0 } else { depth + 1 };
    for file in &node.files {
        if let ReviewFileTreeRow::File {
            path,
            name,
            additions,
            deletions,
            ..
        } = file
        {
            rows.push(ReviewFileTreeRow::File {
                path: path.clone(),
                name: name.clone(),
                depth: file_depth,
                additions: *additions,
                deletions: *deletions,
            });
        }
    }
}

const REVIEW_FILE_TREE_INDENT_STEP: f32 = 12.0;

fn review_file_tree_indent(depth: usize) -> Pixels {
    px(depth as f32 * REVIEW_FILE_TREE_INDENT_STEP)
}

fn render_file_tree_diff_summary(additions: i64, deletions: i64) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .flex_shrink_0()
        .child(
            div()
                .text_size(px(9.0))
                .font_family(mono_font_family())
                .text_color(success())
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_size(px(9.0))
                .font_family(mono_font_family())
                .text_color(fg_subtle())
                .child("/"),
        )
        .child(
            div()
                .text_size(px(9.0))
                .font_family(mono_font_family())
                .text_color(danger())
                .child(format!("-{deletions}")),
        )
}

fn render_file_tree_directory_icon() -> impl IntoElement {
    lucide_icon(LucideIcon::Folder, 12.0, fg_subtle())
}

fn render_file_tree_directory_row(name: String, depth: usize) -> impl IntoElement {
    div()
        .w_full()
        .flex_shrink_0()
        .mb(px(1.0))
        .px(px(6.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .hover(|style| style.bg(hover_bg()))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_grow()
                        .min_w_0()
                        .gap(px(4.0))
                        .pl(review_file_tree_indent(depth))
                        .child(render_file_tree_directory_icon())
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_default())
                                .min_w_0()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(name),
                        ),
                ),
        )
}

#[derive(Clone, Copy)]
enum ReviewFileRowOpenMode {
    Diff,
    Source,
}

fn render_file_tree_file_row(
    state: Entity<AppState>,
    path: String,
    file_name: String,
    additions: i64,
    deletions: i64,
    depth: usize,
    selected_path: Option<&str>,
    open_mode: ReviewFileRowOpenMode,
) -> impl IntoElement {
    let is_active = selected_path == Some(path.as_str());
    let file_name_for_tooltip = file_name.clone();
    let file_name_id = path.bytes().fold(5381usize, |acc, byte| {
        acc.wrapping_mul(33).wrapping_add(byte as usize)
    });
    let state_for_open = state.clone();
    let indent = review_file_tree_indent(depth);

    div()
        .w_full()
        .flex_shrink_0()
        .mb(px(1.0))
        .px(px(6.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if is_active {
            diff_selected_edge()
        } else {
            transparent()
        })
        .bg(if is_active {
            diff_line_hover_bg()
        } else {
            transparent()
        })
        .cursor_pointer()
        .hover(|style| style.bg(diff_line_hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            state_for_open.update(cx, |state, cx| {
                state.selected_file_path = Some(path.clone());
                state.selected_diff_anchor = None;
                match open_mode {
                    ReviewFileRowOpenMode::Diff => {
                        state.set_review_center_mode(ReviewCenterMode::SemanticDiff);
                    }
                    ReviewFileRowOpenMode::Source => {
                        state.set_review_source_target(ReviewSourceTarget {
                            path: path.clone(),
                            line: None,
                            reason: Some("Selected from file tree".to_string()),
                        });
                    }
                }
                state.persist_active_review_session();
                cx.notify();
            });
            match open_mode {
                ReviewFileRowOpenMode::Diff => {
                    ensure_selected_file_content_loaded(&state_for_open, window, cx);
                }
                ReviewFileRowOpenMode::Source => {
                    ensure_active_review_focus_loaded(&state_for_open, window, cx);
                }
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .min_w_0()
                        .pl(indent)
                        .child(
                            div()
                                .id(("file-tree-file-name", file_name_id))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if is_active {
                                    fg_emphasis()
                                } else {
                                    fg_default()
                                })
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .tooltip(move |_, cx| {
                                    build_text_tooltip(
                                        SharedString::from(file_name_for_tooltip.clone()),
                                        cx,
                                    )
                                })
                                .child(file_name),
                        ),
                )
                .child(render_file_tree_diff_summary(additions, deletions)),
        )
}

pub fn ensure_selected_file_content_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            load_pull_request_file_content_flow(model, None, cx).await;
        })
        .detach();
}

pub async fn load_pull_request_file_content_flow(
    model: Entity<AppState>,
    requested_path: Option<String>,
    cx: &mut AsyncWindowContext,
) {
    let request = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let lsp_session_manager = state.lsp_session_manager.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            let selected_path = requested_path
                .clone()
                .or_else(|| state.selected_file_path.clone())
                .or_else(|| detail.files.first().map(|file| file.path.clone()))?;
            let selected_file = detail
                .files
                .iter()
                .find(|file| file.path == selected_path)
                .cloned()?;
            let parsed = find_parsed_diff_file(&detail.parsed_diff, &selected_file.path).cloned();
            let request = build_file_content_request(&detail, &selected_file, parsed.as_ref())?;
            let detail_state = state.detail_states.get(&detail_key);

            let file_content_loaded =
                is_local_checkout_file_loaded(detail_state, &request.path, &request.request_key);
            let lsp_loaded = is_lsp_status_loaded(detail_state, &selected_file.path);
            let already_loaded = file_content_loaded && lsp_loaded;

            Some((
                cache,
                lsp_session_manager,
                detail_key,
                detail,
                selected_file,
                request,
                already_loaded,
                existing_local_repo_status,
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        lsp_session_manager,
        detail_key,
        detail,
        selected_file,
        request,
        already_loaded,
        existing_local_repo_status,
    )) = request
    else {
        return;
    };

    if already_loaded {
        return;
    }

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let file_state = detail_state
                    .file_content_states
                    .entry(request.path.clone())
                    .or_default();
                file_state.request_key = Some(request.request_key.clone());
                file_state.document = None;
                file_state.prepared = None;
                file_state.loading = true;
                file_state.error = None;
                detail_state.local_repository_loading = existing_local_repo_status
                    .as_ref()
                    .map(|status| !status.ready_for_snapshot_features())
                    .unwrap_or(true);
                detail_state.local_repository_error = None;
                detail_state
                    .lsp_loading_paths
                    .insert(selected_file.path.clone());
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let local_load_result = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                cx.background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = detail.repository.clone();
                        let path = request.path.clone();
                        let reference = request.local_reference.clone();
                        let prefer_worktree =
                            request.prefer_worktree && status.should_prefer_worktree_contents();
                        let root = std::path::PathBuf::from(root);
                        async move {
                            local_documents::load_local_repository_file_content(
                                &cache,
                                &repository,
                                &root,
                                &reference,
                                &path,
                                prefer_worktree,
                            )
                        }
                    })
                    .await
            } else {
                Err(status.message.clone())
            }
        } else {
            Err(local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
        }
    } else {
        Err(local_repo_error
            .clone()
            .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
    };

    let load_result = match local_load_result {
        Ok(document) => Ok(document),
        Err(local_error) => cx
            .background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let path = request.path.clone();
                let reference = request.reference.clone();
                async move {
                    github::load_pull_request_file_content(&cache, &repository, &reference, &path)
                        .map_err(|github_error| {
                            format!(
                                "{local_error}\nGitHub fallback also failed for {repository}@{reference}:{path}: {github_error}"
                            )
                        })
                }
            })
            .await,
    };
    let lsp_status = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                cx.background_executor()
                    .spawn({
                        let lsp_session_manager = lsp_session_manager.clone();
                        let root = std::path::PathBuf::from(root);
                        let file_path = selected_file.path.clone();
                        async move { lsp_session_manager.status_for_file(&root, &file_path) }
                    })
                    .await
            } else {
                lsp::LspServerStatus::checkout_unavailable(status.message.clone())
            }
        } else {
            lsp::LspServerStatus::checkout_unavailable(
                local_repo_error
                    .clone()
                    .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
            )
        }
    } else {
        lsp::LspServerStatus::checkout_unavailable(
            local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
        )
    };

    let prepared_result = load_result.map(|document| {
        let prepared = prepare_file_content(&selected_file.path, &request.reference, &document);
        (document, prepared)
    });

    model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return;
            };
            let Some(file_state) = detail_state.file_content_states.get_mut(&request.path) else {
                return;
            };
            if file_state.request_key.as_deref() != Some(&request.request_key) {
                return;
            }

            file_state.loading = false;
            detail_state.local_repository_loading = false;
            detail_state.local_repository_status = local_repo_status.clone();
            detail_state.local_repository_error = local_repo_error.clone();
            detail_state.lsp_loading_paths.remove(&selected_file.path);
            detail_state
                .lsp_statuses
                .insert(selected_file.path.clone(), lsp_status.clone());
            match prepared_result {
                Ok((document, prepared)) => {
                    file_state.document = Some(document);
                    file_state.prepared = Some(prepared);
                    file_state.error = None;
                }
                Err(error) => {
                    file_state.document = None;
                    file_state.prepared = None;
                    file_state.error = Some(error);
                }
            }

            cx.notify();
        })
        .ok();
}

pub async fn load_local_source_file_content_flow(
    model: Entity<AppState>,
    requested_path: String,
    cx: &mut AsyncWindowContext,
) {
    let request = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let lsp_session_manager = state.lsp_session_manager.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            let request = build_head_file_content_request(&detail, &requested_path)?;
            let detail_state = state.detail_states.get(&detail_key);

            let file_content_loaded =
                is_local_checkout_file_loaded(detail_state, &request.path, &request.request_key);
            let lsp_loaded = is_lsp_status_loaded(detail_state, &request.path);
            let already_loaded = file_content_loaded && lsp_loaded;

            Some((
                cache,
                lsp_session_manager,
                detail_key,
                detail,
                request,
                already_loaded,
                existing_local_repo_status,
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        lsp_session_manager,
        detail_key,
        detail,
        request,
        already_loaded,
        existing_local_repo_status,
    )) = request
    else {
        return;
    };

    if already_loaded {
        return;
    }

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let file_state = detail_state
                    .file_content_states
                    .entry(request.path.clone())
                    .or_default();
                file_state.request_key = Some(request.request_key.clone());
                file_state.document = None;
                file_state.prepared = None;
                file_state.loading = true;
                file_state.error = None;
                detail_state.local_repository_loading = existing_local_repo_status
                    .as_ref()
                    .map(|status| !status.ready_for_snapshot_features())
                    .unwrap_or(true);
                detail_state.local_repository_error = None;
                detail_state.lsp_loading_paths.insert(request.path.clone());
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let local_load_result = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                cx.background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = detail.repository.clone();
                        let path = request.path.clone();
                        let reference = request.local_reference.clone();
                        let prefer_worktree =
                            request.prefer_worktree && status.should_prefer_worktree_contents();
                        let root = std::path::PathBuf::from(root);
                        async move {
                            local_documents::load_local_repository_file_content(
                                &cache,
                                &repository,
                                &root,
                                &reference,
                                &path,
                                prefer_worktree,
                            )
                        }
                    })
                    .await
            } else {
                Err(status.message.clone())
            }
        } else {
            Err(local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
        }
    } else {
        Err(local_repo_error
            .clone()
            .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
    };

    let load_result = match local_load_result {
        Ok(document) => Ok(document),
        Err(local_error) => cx
            .background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let path = request.path.clone();
                let reference = request.reference.clone();
                async move {
                    github::load_pull_request_file_content(&cache, &repository, &reference, &path)
                        .map_err(|github_error| {
                            format!(
                                "{local_error}\nGitHub fallback also failed for {repository}@{reference}:{path}: {github_error}"
                            )
                        })
                }
            })
            .await,
    };
    let lsp_status = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                cx.background_executor()
                    .spawn({
                        let lsp_session_manager = lsp_session_manager.clone();
                        let root = std::path::PathBuf::from(root);
                        let file_path = request.path.clone();
                        async move { lsp_session_manager.status_for_file(&root, &file_path) }
                    })
                    .await
            } else {
                lsp::LspServerStatus::checkout_unavailable(status.message.clone())
            }
        } else {
            lsp::LspServerStatus::checkout_unavailable(
                local_repo_error
                    .clone()
                    .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
            )
        }
    } else {
        lsp::LspServerStatus::checkout_unavailable(
            local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
        )
    };

    let prepared_result = load_result.map(|document| {
        let prepared = prepare_file_content(&request.path, &request.reference, &document);
        (document, prepared)
    });

    model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return;
            };
            let Some(file_state) = detail_state.file_content_states.get_mut(&request.path) else {
                return;
            };
            if file_state.request_key.as_deref() != Some(&request.request_key) {
                return;
            }

            file_state.loading = false;
            detail_state.local_repository_loading = false;
            detail_state.local_repository_status = local_repo_status.clone();
            detail_state.local_repository_error = local_repo_error.clone();
            detail_state.lsp_loading_paths.remove(&request.path);
            detail_state
                .lsp_statuses
                .insert(request.path.clone(), lsp_status.clone());
            match prepared_result {
                Ok((document, prepared)) => {
                    file_state.document = Some(document);
                    file_state.prepared = Some(prepared);
                    file_state.error = None;
                }
                Err(error) => {
                    file_state.document = None;
                    file_state.prepared = None;
                    file_state.error = Some(error);
                }
            }

            cx.notify();
        })
        .ok();
}

#[derive(Clone)]
struct FileContentRequest {
    path: String,
    reference: String,
    local_reference: String,
    prefer_worktree: bool,
    request_key: String,
}

fn build_file_content_request(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
) -> Option<FileContentRequest> {
    let (path, reference, local_reference, prefer_worktree) = if file.change_type == "DELETED" {
        (
            parsed
                .and_then(|parsed| parsed.previous_path.clone())
                .unwrap_or_else(|| file.path.clone()),
            detail
                .base_ref_oid
                .clone()
                .unwrap_or_else(|| detail.base_ref_name.clone()),
            detail
                .base_ref_oid
                .clone()
                .unwrap_or_else(|| detail.base_ref_name.clone()),
            false,
        )
    } else {
        (
            file.path.clone(),
            detail
                .head_ref_oid
                .clone()
                .unwrap_or_else(|| detail.head_ref_name.clone()),
            detail
                .head_ref_oid
                .clone()
                .unwrap_or_else(|| "HEAD".to_string()),
            true,
        )
    };

    if path.is_empty() || reference.is_empty() || local_reference.is_empty() {
        return None;
    }

    Some(FileContentRequest {
        request_key: format!(
            "{}:{reference}:{path}:{}",
            detail.updated_at, detail.repository
        ),
        path,
        reference,
        local_reference,
        prefer_worktree,
    })
}

fn build_head_file_content_request(
    detail: &PullRequestDetail,
    path: &str,
) -> Option<FileContentRequest> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    let reference = detail
        .head_ref_oid
        .clone()
        .unwrap_or_else(|| detail.head_ref_name.clone());
    let local_reference = detail
        .head_ref_oid
        .clone()
        .unwrap_or_else(|| "HEAD".to_string());

    if reference.is_empty() || local_reference.is_empty() {
        return None;
    }

    Some(FileContentRequest {
        request_key: format!(
            "{}:{reference}:{path}:{}",
            detail.updated_at, detail.repository
        ),
        path: path.to_string(),
        reference,
        local_reference,
        prefer_worktree: true,
    })
}

fn is_local_checkout_file_loaded(
    detail_state: Option<&DetailState>,
    path: &str,
    request_key: &str,
) -> bool {
    detail_state
        .and_then(|detail_state| detail_state.file_content_states.get(path))
        .map(|file_state| {
            file_state.request_key.as_deref() == Some(request_key)
                && (file_state.loading
                    || file_state
                        .document
                        .as_ref()
                        .map(|document| document.source == REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT)
                        .unwrap_or(false))
        })
        .unwrap_or(false)
}

fn is_lsp_status_loaded(detail_state: Option<&DetailState>, path: &str) -> bool {
    detail_state
        .map(|detail_state| {
            detail_state.lsp_loading_paths.contains(path)
                || detail_state.lsp_statuses.contains_key(path)
        })
        .unwrap_or(false)
}

fn prepare_file_content(
    file_path: &str,
    reference: &str,
    document: &RepositoryFileContent,
) -> PreparedFileContent {
    let lines = document.content.as_deref().unwrap_or_default();
    let text_lines = if lines.is_empty() {
        Vec::new()
    } else {
        lines.lines().map(str::to_string).collect::<Vec<_>>()
    };
    let spans = if document.is_binary || document.size_bytes > syntax::MAX_HIGHLIGHT_BYTES {
        text_lines
            .iter()
            .map(|_| Vec::new())
            .collect::<Vec<Vec<SyntaxSpan>>>()
    } else {
        syntax::highlight_lines(file_path, text_lines.iter().map(|line| line.as_str()))
    };

    let prepared_lines = text_lines
        .into_iter()
        .zip(spans)
        .enumerate()
        .map(|(index, (text, spans))| PreparedFileLine {
            line_number: index + 1,
            text,
            spans,
        })
        .collect::<Vec<_>>();

    PreparedFileContent {
        path: file_path.to_string(),
        reference: reference.to_string(),
        is_binary: document.is_binary,
        size_bytes: document.size_bytes,
        text: Arc::<str>::from(document.content.as_deref().unwrap_or_default()),
        lines: Arc::new(prepared_lines),
    }
}

fn render_diff_panel(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
    semantic_file: Option<&SemanticDiffFile>,
    review_stack: Arc<ReviewStack>,
    cx: &App,
) -> impl IntoElement {
    let files = &detail.files;
    let selected_file = selected_path
        .and_then(|p| files.iter().find(|f| f.path == p))
        .or(files.first());

    let selected_parsed =
        selected_file.and_then(|file| find_parsed_diff_file(&detail.parsed_diff, &file.path));
    let file_thread_count = selected_file
        .map(|file| {
            detail
                .review_threads
                .iter()
                .filter(|thread| thread.path == file.path)
                .count()
        })
        .unwrap_or(0);
    let diff_view_state =
        selected_file.map(|file| prepare_diff_view_state(app_state, detail, &file.path));
    let file_content_state = selected_file.and_then(|file| {
        app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.file_content_states.get(&file.path))
            .cloned()
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
    let file_document = file_content_state
        .as_ref()
        .and_then(|state| state.document.as_ref());
    let lsp_status = selected_file.and_then(|file| {
        app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.lsp_statuses.get(&file.path))
    });
    let lsp_loading = selected_file
        .map(|file| {
            app_state
                .active_detail_state()
                .map(|detail_state| detail_state.lsp_loading_paths.contains(&file.path))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let review_session = app_state
        .active_review_session()
        .cloned()
        .unwrap_or_default();
    let center_mode = review_session.center_mode;
    let stack_filter = (center_mode == ReviewCenterMode::Stack)
        .then(|| {
            build_layer_diff_filter(
                review_stack.as_ref(),
                review_session.stack_diff_mode,
                review_session.selected_stack_layer_id.as_deref(),
                &review_session.reviewed_stack_atom_ids,
            )
        })
        .flatten();
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

    div()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .bg(diff_editor_bg())
        .child(render_diff_toolbar(
            files.len(),
            selected_file,
            selected_parsed,
            semantic_file,
            file_thread_count,
            file_document,
            local_repo_status,
            local_repo_loading,
            local_repo_error,
            lsp_status,
            lsp_loading,
            selected_anchor,
        ))
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .bg(diff_editor_bg())
                .flex()
                .flex_col()
                .child(if center_mode == ReviewCenterMode::SourceBrowser {
                    source_target
                        .as_ref()
                        .map(|target| render_source_browser(state, target, source_parsed, cx))
                        .unwrap_or_else(|| {
                            panel_state_text(
                                "Select a file or definition to open the source browser.",
                            )
                            .into_any_element()
                        })
                } else if center_mode == ReviewCenterMode::AiTour {
                    render_ai_tour_view(state, detail, cx)
                } else if let (Some(file), Some(diff_view_state)) = (selected_file, diff_view_state)
                {
                    render_file_diff(
                        state,
                        file,
                        selected_parsed,
                        semantic_file,
                        file_content_state
                            .as_ref()
                            .and_then(|state| state.prepared.as_ref()),
                        selected_anchor,
                        diff_view_state,
                        review_stack.clone(),
                        stack_filter.clone(),
                        cx,
                    )
                    .into_any_element()
                } else {
                    panel_state_text("No files returned for this pull request.").into_any_element()
                }),
        )
}

fn render_diff_toolbar(
    total_files: usize,
    selected_file: Option<&PullRequestFile>,
    selected_parsed: Option<&ParsedDiffFile>,
    semantic_file: Option<&SemanticDiffFile>,
    file_thread_count: usize,
    file_document: Option<&RepositoryFileContent>,
    local_repo_status: Option<&local_repo::LocalRepositoryStatus>,
    local_repo_loading: bool,
    _local_repo_error: Option<&str>,
    lsp_status: Option<&lsp::LspServerStatus>,
    lsp_loading: bool,
    selected_anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    let selected_section =
        semantic_file.and_then(|semantic| semantic.section_for_anchor(selected_anchor));
    let focus_title = selected_file
        .map(|file| file.path.clone())
        .unwrap_or_else(|| format!("{total_files} changed files"));
    let mut focus_meta = Vec::new();
    if let Some(file) = selected_file {
        focus_meta.push(format!("{} file", label_for_change_type(&file.change_type)));
        focus_meta.push(format!("+{} / -{}", file.additions, file.deletions));
        focus_meta.push(
            selected_section
                .map(|section| format!("{}: {}", section.kind.label(), section.title.as_str()))
                .unwrap_or_else(|| format!("{total_files} files changed")),
        );
    } else {
        focus_meta.push(format!("{total_files} changed files"));
    }
    if file_thread_count > 0 {
        focus_meta.push(format!(
            "{} open thread{}",
            file_thread_count,
            if file_thread_count == 1 { "" } else { "s" }
        ));
    }
    if selected_parsed
        .map(|parsed| parsed.is_binary)
        .unwrap_or(false)
    {
        focus_meta.push("binary diff".to_string());
    }
    if local_repo_loading {
        focus_meta.push("preparing checkout".to_string());
    } else if file_document
        .map(|document| document.source != REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT)
        .unwrap_or(false)
    {
        focus_meta.push("GitHub snapshot".to_string());
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
    if lsp_loading {
        focus_meta.push("indexing symbols".to_string());
    } else if let Some(status) = lsp_status.filter(|status| !status.is_ready()) {
        focus_meta.push(status.badge_label().to_string());
    }
    let focus_summary = focus_meta.join(" / ");

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
                        .child(focus_title),
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
}

fn render_ai_tour_view(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    cx: &App,
) -> AnyElement {
    let (
        provider,
        provider_status,
        provider_loading,
        provider_error,
        local_repo_loading,
        local_repo_error,
        tour_loading,
        tour_generating,
        tour_progress_summary,
        tour_progress_detail,
        tour_error,
        tour_message,
        tour_success,
        generated_tour,
        ai_tour_section_list_state,
    ) = {
        let app_state = state.read(cx);
        let detail_state = app_state.active_detail_state();
        let tour_state = app_state.active_tour_state();

        (
            app_state.selected_tour_provider(),
            app_state.selected_tour_provider_status().cloned(),
            app_state.code_tour_provider_loading,
            app_state.code_tour_provider_error.clone(),
            detail_state
                .map(|state| state.local_repository_loading)
                .unwrap_or(false),
            detail_state.and_then(|state| state.local_repository_error.clone()),
            tour_state.map(|state| state.loading).unwrap_or(false),
            tour_state.map(|state| state.generating).unwrap_or(false),
            tour_state.and_then(|state| state.progress_summary.clone()),
            tour_state.and_then(|state| state.progress_detail.clone()),
            tour_state.and_then(|state| state.error.clone()),
            tour_state.and_then(|state| state.message.clone()),
            tour_state.map(|state| state.success).unwrap_or(false),
            tour_state.and_then(|state| state.document.clone()),
            app_state.ai_tour_section_list_state.clone(),
        )
    };

    let state_for_generate = state.clone();
    let generate_label = ai_tour_generate_label(provider, generated_tour.as_ref(), tour_generating);
    let has_status_messages = provider_error.is_some()
        || local_repo_error.is_some()
        || tour_error.is_some()
        || tour_message.is_some();

    let shell = div().flex_grow().min_h_0().flex().flex_col().bg(bg_inset());

    match generated_tour {
        Some(tour) => {
            let section_count = tour.sections.len();
            let mut items = Vec::new();
            if section_count > 0 {
                items.push(AiTourContentItem::SemanticOverview);
            }
            if tour_generating || tour_loading || provider_loading {
                items.push(AiTourContentItem::Progress);
            }
            if has_status_messages {
                items.push(AiTourContentItem::StatusMessages);
            }
            if section_count == 0 {
                items.push(AiTourContentItem::Empty);
            } else {
                items.extend((0..section_count).map(AiTourContentItem::Section));
            }
            items.push(AiTourContentItem::Spacer);

            if ai_tour_section_list_state.item_count() != items.len() {
                ai_tour_section_list_state.reset(items.len());
            }
            let section_targets = items
                .iter()
                .enumerate()
                .filter_map(|(item_ix, item)| match item {
                    AiTourContentItem::Section(section_ix) => Some((*section_ix, item_ix)),
                    _ => None,
                })
                .collect::<Vec<_>>();

            let tour = Arc::new(tour);
            let detail = Arc::new(detail.clone());
            let state_for_sections = state.clone();
            let tour_for_sections = tour.clone();
            let detail_for_sections = detail.clone();
            let list_state_for_overview = ai_tour_section_list_state.clone();
            let tour_for_overview = tour.clone();
            let section_targets_for_overview = Arc::new(section_targets);
            let items = Arc::new(items);

            shell
                .child(
                    list(
                        ai_tour_section_list_state.clone(),
                        move |ix, _window, cx| match items[ix] {
                            AiTourContentItem::SemanticOverview => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_ai_tour_semantic_overview(
                                    tour_for_overview.as_ref(),
                                    provider,
                                    provider_status.as_ref(),
                                    local_repo_loading,
                                    &generate_label,
                                    list_state_for_overview.clone(),
                                    section_targets_for_overview.clone(),
                                    {
                                        let state = state_for_generate.clone();
                                        move |_, window, cx| {
                                            trigger_generate_tour(&state, window, cx, false)
                                        }
                                    },
                                ))
                                .into_any_element(),
                            AiTourContentItem::Pending => div().into_any_element(),
                            AiTourContentItem::Progress => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_ai_tour_progress_panel(
                                    provider,
                                    provider_loading,
                                    tour_loading,
                                    tour_generating,
                                    tour_progress_summary.as_deref(),
                                    tour_progress_detail.as_deref(),
                                ))
                                .into_any_element(),
                            AiTourContentItem::StatusMessages => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_ai_tour_status_messages(
                                    provider_error.as_deref(),
                                    local_repo_error.as_deref(),
                                    tour_error.as_deref(),
                                    tour_message.as_deref(),
                                    tour_success,
                                ))
                                .into_any_element(),
                            AiTourContentItem::Empty => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(nested_panel().child(panel_state_text(
                                    "No AI tour sections were returned for this pull request.",
                                )))
                                .into_any_element(),
                            AiTourContentItem::Section(section_ix) => {
                                let section = &tour_for_sections.sections[section_ix];
                                div()
                                    .px(px(18.0))
                                    .pb(px(14.0))
                                    .child(render_ai_tour_section(
                                        &state_for_sections,
                                        detail_for_sections.as_ref(),
                                        tour_for_sections.as_ref(),
                                        section,
                                        cx,
                                    ))
                                    .into_any_element()
                            }
                            AiTourContentItem::Spacer => {
                                div().h(px(18.0)).w_full().into_any_element()
                            }
                        },
                    )
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                )
                .into_any_element()
        }
        None => {
            let mut items = vec![AiTourContentItem::Pending];
            if has_status_messages {
                items.push(AiTourContentItem::StatusMessages);
            }
            items.push(AiTourContentItem::Spacer);

            if ai_tour_section_list_state.item_count() != items.len() {
                ai_tour_section_list_state.reset(items.len());
            }

            let items = Arc::new(items);

            shell
                .child(
                    list(
                        ai_tour_section_list_state.clone(),
                        move |ix, _window, _cx| match items[ix] {
                            AiTourContentItem::Pending => div()
                                .pt(px(18.0))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_ai_tour_pending_panel(
                                    provider,
                                    provider_status.as_ref(),
                                    provider_loading,
                                    local_repo_loading,
                                    tour_loading,
                                    tour_generating,
                                    tour_progress_summary.as_deref(),
                                    tour_progress_detail.as_deref(),
                                    &generate_label,
                                    {
                                        let state = state_for_generate.clone();
                                        move |_, window, cx| {
                                            trigger_generate_tour(&state, window, cx, false)
                                        }
                                    },
                                ))
                                .into_any_element(),
                            AiTourContentItem::StatusMessages => div()
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_ai_tour_status_messages(
                                    provider_error.as_deref(),
                                    local_repo_error.as_deref(),
                                    tour_error.as_deref(),
                                    tour_message.as_deref(),
                                    tour_success,
                                ))
                                .into_any_element(),
                            AiTourContentItem::Spacer => {
                                div().h(px(18.0)).w_full().into_any_element()
                            }
                            _ => div().into_any_element(),
                        },
                    )
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                )
                .into_any_element()
        }
    }
}

#[derive(Clone, Copy)]
enum AiTourContentItem {
    SemanticOverview,
    Pending,
    Progress,
    StatusMessages,
    Empty,
    Section(usize),
    Spacer,
}

fn ai_tour_generate_label(
    provider: CodeTourProvider,
    generated_tour: Option<&GeneratedCodeTour>,
    generating: bool,
) -> String {
    if generating {
        format!("Generating with {}...", provider.label())
    } else if generated_tour
        .map(|tour| tour.provider == provider)
        .unwrap_or(false)
    {
        format!("Regenerate with {}", provider.label())
    } else {
        format!("Generate with {}", provider.label())
    }
}

fn ai_tour_provider_status_label(status: &CodeTourProviderStatus) -> &'static str {
    if status.available && status.authenticated {
        "ready"
    } else if status.available {
        "needs auth"
    } else {
        "unavailable"
    }
}

fn render_ai_tour_pending_panel(
    provider: CodeTourProvider,
    provider_status: Option<&CodeTourProviderStatus>,
    provider_loading: bool,
    local_repo_loading: bool,
    tour_loading: bool,
    tour_generating: bool,
    progress_summary: Option<&str>,
    progress_detail: Option<&str>,
    generate_label: &str,
    on_generate: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (title, body) = if provider_loading {
        (
            "Checking AI provider".to_string(),
            "The app is checking the provider configured in Settings.".to_string(),
        )
    } else if tour_loading {
        (
            progress_summary
                .map(str::to_string)
                .unwrap_or_else(|| "Looking for a cached AI tour".to_string()),
            progress_detail.map(str::to_string).unwrap_or_else(|| {
                "The app is checking whether this pull request head already has a stored tour."
                    .to_string()
            }),
        )
    } else if tour_generating {
        (
            progress_summary
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} is building the AI tour", provider.label())),
            progress_detail.map(str::to_string).unwrap_or_else(|| {
                "The provider is reading the diff, review threads, and local checkout.".to_string()
            }),
        )
    } else if let Some(status) = provider_status {
        if !status.available || !status.authenticated {
            (
                "AI provider needs attention".to_string(),
                status.message.clone(),
            )
        } else {
            (
                "Generate an AI tour".to_string(),
                "Create a short guided walkthrough that groups related changes and shows the matching diff under each explanation.".to_string(),
            )
        }
    } else {
        (
            "Preparing AI tour".to_string(),
            "Waiting for the provider configured in Settings to finish loading.".to_string(),
        )
    };

    nested_panel().child(
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .flex_wrap()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .min_w_0()
                    .child(eyebrow("AI tour"))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(fg_muted())
                            .max_w(px(720.0))
                            .child(body),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .flex_wrap()
                    .child(badge(provider.label()))
                    .when_some(provider_status, |el, status| {
                        el.child(badge(ai_tour_provider_status_label(status)))
                    })
                    .when(provider_loading, |el| el.child(badge("Checking provider")))
                    .when(local_repo_loading, |el| {
                        el.child(badge("Preparing checkout"))
                    })
                    .child(review_button(generate_label, on_generate)),
            ),
    )
}

fn render_ai_tour_progress_panel(
    provider: CodeTourProvider,
    provider_loading: bool,
    tour_loading: bool,
    tour_generating: bool,
    progress_summary: Option<&str>,
    progress_detail: Option<&str>,
) -> impl IntoElement {
    let title = if provider_loading {
        "Checking provider".to_string()
    } else if tour_loading {
        progress_summary
            .map(str::to_string)
            .unwrap_or_else(|| "Loading cached tour".to_string())
    } else if tour_generating {
        progress_summary
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} is updating the tour", provider.label()))
    } else {
        "Preparing AI tour".to_string()
    };
    let body = progress_detail
        .map(str::to_string)
        .unwrap_or_else(|| "The AI tour will update here when the provider returns.".to_string());

    nested_panel()
        .child(eyebrow("Status"))
        .child(
            div()
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .mt(px(8.0))
                .child(body),
        )
}

fn render_ai_tour_status_messages(
    provider_error: Option<&str>,
    local_repo_error: Option<&str>,
    tour_error: Option<&str>,
    tour_message: Option<&str>,
    tour_success: bool,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when_some(provider_error, |el, error| el.child(error_text(error)))
        .when_some(local_repo_error, |el, error| el.child(error_text(error)))
        .when_some(tour_error, |el, error| el.child(error_text(error)))
        .when_some(tour_message, |el, message| {
            if tour_success {
                el.child(success_text(message))
            } else {
                el.child(error_text(message))
            }
        })
}

fn render_ai_tour_semantic_overview(
    tour: &GeneratedCodeTour,
    provider: CodeTourProvider,
    provider_status: Option<&CodeTourProviderStatus>,
    local_repo_loading: bool,
    generate_label: &str,
    list_state: ListState,
    section_targets: Arc<Vec<(usize, usize)>>,
    on_generate: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .rounded(radius())
        .bg(bg_overlay())
        .border_1()
        .border_color(transparent())
        .shadow_sm()
        .overflow_hidden()
        .child(material_surface("review-map").h(px(50.0)).w_full())
        .child(
            div()
                .p(px(20.0))
                .pb(px(12.0))
                .flex()
                .items_start()
                .justify_between()
                .gap(px(16.0))
                .flex_wrap()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(eyebrow("Semantic groups"))
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Review map"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(8.0))
                        .flex_wrap()
                        .child(badge(&format!(
                            "{} group{}",
                            tour.sections.len(),
                            if tour.sections.len() == 1 { "" } else { "s" }
                        )))
                        .when(!tour.open_questions.is_empty(), |el| {
                            el.child(badge(&format!(
                                "{} open question{}",
                                tour.open_questions.len(),
                                if tour.open_questions.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            )))
                        })
                        .when(!tour.warnings.is_empty(), |el| {
                            el.child(badge(&format!(
                                "{} warning{}",
                                tour.warnings.len(),
                                if tour.warnings.len() == 1 { "" } else { "s" }
                            )))
                        })
                        .child(badge(provider.label()))
                        .when_some(provider_status, |el, status| {
                            el.child(badge(ai_tour_provider_status_label(status)))
                        })
                        .when(local_repo_loading, |el| {
                            el.child(badge("Preparing checkout"))
                        })
                        .child(review_button(generate_label, on_generate)),
                ),
        )
        .child(
            div()
                .px(px(20.0))
                .pb(px(20.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(
                    tour.sections
                        .iter()
                        .enumerate()
                        .map(|(section_ix, section)| {
                            render_ai_tour_semantic_overview_row(
                                tour,
                                section,
                                section_ix,
                                section_ix > 0,
                                list_state.clone(),
                                section_targets.clone(),
                            )
                        }),
                ),
        )
}

fn render_ai_tour_semantic_overview_row(
    tour: &GeneratedCodeTour,
    section: &TourSection,
    section_ix: usize,
    _show_divider: bool,
    list_state: ListState,
    section_targets: Arc<Vec<(usize, usize)>>,
) -> impl IntoElement {
    let metrics = ai_tour_section_metrics(tour, section);
    let material_key = format!("tour-section-{}-{}", section_ix, section.title);
    let target_index = section_targets
        .iter()
        .find(|(candidate_ix, _)| *candidate_ix == section_ix)
        .map(|(_, item_ix)| *item_ix)
        .unwrap_or(0);

    div()
        .min_h(px(72.0))
        .rounded(radius_sm())
        .bg(bg_surface())
        .border_1()
        .border_color(border_muted())
        .shadow_sm()
        .overflow_hidden()
        .flex()
        .cursor_pointer()
        .hover(|style| style.bg(bg_overlay()).border_color(border_default()))
        .on_mouse_down(MouseButton::Left, move |_, _, _| {
            list_state.scroll_to(ListOffset {
                item_ix: target_index,
                offset_in_item: px(0.0),
            });
        })
        .child(material_surface(&material_key).w(px(10.0)).flex_shrink_0())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .min_w_0()
                .flex_grow()
                .p(px(12.0))
                .child(render_ai_tour_category_icon(section.category, 34.0, 17.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .min_w_0()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(section.title.clone()),
                                )
                                .child(render_ai_tour_priority_chip(section.priority)),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .line_clamp(1)
                                .child(section.summary.clone()),
                        ),
                )
                .child(render_ai_tour_section_metrics(metrics)),
        )
}

fn render_ai_tour_section_metrics(metrics: AiTourSectionMetrics) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.0))
        .flex_wrap()
        .max_w(px(280.0))
        .child(ai_tour_metric_chip(&format!(
            "{} file{}",
            metrics.file_count,
            if metrics.file_count == 1 { "" } else { "s" }
        )))
        .child(ai_tour_metric_chip(&format!(
            "{} thread{}",
            metrics.unresolved_thread_count,
            if metrics.unresolved_thread_count == 1 {
                ""
            } else {
                "s"
            }
        )))
        .child(ai_tour_metric_chip(&format!(
            "+{} / -{}",
            metrics.additions, metrics.deletions
        )))
}

fn render_ai_tour_category_icon(
    category: TourSectionCategory,
    tile_size: f32,
    icon_size: f32,
) -> impl IntoElement {
    div()
        .w(px(tile_size))
        .h(px(tile_size))
        .rounded(radius_sm())
        .border_1()
        .border_color(ai_tour_category_border(category))
        .bg(ai_tour_category_bg(category))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(lucide_icon(
            ai_tour_category_lucide_icon(category),
            icon_size,
            ai_tour_category_fg(category),
        ))
}

fn render_ai_tour_priority_chip(priority: TourSectionPriority) -> impl IntoElement {
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(ai_tour_priority_bg(priority))
        .border_1()
        .border_color(ai_tour_priority_border(priority))
        .flex_shrink_0()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .font_family(mono_font_family())
        .text_color(ai_tour_priority_fg(priority))
        .child(priority.label())
}

fn ai_tour_metric_chip(text: &str) -> impl IntoElement {
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg_subtle())
        .border_1()
        .border_color(border_muted())
        .text_size(px(10.0))
        .font_family(mono_font_family())
        .text_color(fg_muted())
        .whitespace_nowrap()
        .child(text.to_string())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct AiTourSectionMetrics {
    file_count: usize,
    additions: i64,
    deletions: i64,
    unresolved_thread_count: i64,
}

fn ai_tour_section_metrics(
    tour: &GeneratedCodeTour,
    section: &TourSection,
) -> AiTourSectionMetrics {
    let mut metrics = AiTourSectionMetrics::default();

    for step_id in &section.step_ids {
        if let Some(step) = tour.steps.iter().find(|step| step.id == *step_id) {
            metrics.file_count += 1;
            metrics.additions += step.additions;
            metrics.deletions += step.deletions;
            metrics.unresolved_thread_count += step.unresolved_thread_count;
        }
    }

    metrics
}

fn ai_tour_category_lucide_icon(category: TourSectionCategory) -> LucideIcon {
    match category {
        TourSectionCategory::AuthSecurity => LucideIcon::ShieldCheck,
        TourSectionCategory::DataState => LucideIcon::Database,
        TourSectionCategory::ApiIo => LucideIcon::Plug,
        TourSectionCategory::UiUx => LucideIcon::Palette,
        TourSectionCategory::Tests => LucideIcon::FlaskConical,
        TourSectionCategory::Docs => LucideIcon::BookOpenText,
        TourSectionCategory::Config => LucideIcon::SlidersHorizontal,
        TourSectionCategory::Infra => LucideIcon::ServerCog,
        TourSectionCategory::Refactor => LucideIcon::GitCompareArrows,
        TourSectionCategory::Performance => LucideIcon::Gauge,
        TourSectionCategory::Reliability => LucideIcon::BadgeCheck,
        TourSectionCategory::Other => LucideIcon::CircleHelp,
    }
}

fn ai_tour_category_fg(category: TourSectionCategory) -> Rgba {
    match category {
        TourSectionCategory::AuthSecurity => danger(),
        TourSectionCategory::DataState => accent(),
        TourSectionCategory::ApiIo => warning(),
        TourSectionCategory::UiUx => fg_emphasis(),
        TourSectionCategory::Tests => success(),
        TourSectionCategory::Docs => fg_muted(),
        TourSectionCategory::Config => warning(),
        TourSectionCategory::Infra => accent(),
        TourSectionCategory::Refactor => fg_default(),
        TourSectionCategory::Performance => warning(),
        TourSectionCategory::Reliability => success(),
        TourSectionCategory::Other => fg_muted(),
    }
}

fn ai_tour_category_bg(category: TourSectionCategory) -> Rgba {
    match category {
        TourSectionCategory::AuthSecurity => danger_muted(),
        TourSectionCategory::DataState => accent_muted(),
        TourSectionCategory::ApiIo => warning_muted(),
        TourSectionCategory::UiUx => bg_emphasis(),
        TourSectionCategory::Tests => success_muted(),
        TourSectionCategory::Docs => bg_subtle(),
        TourSectionCategory::Config => warning_muted(),
        TourSectionCategory::Infra => accent_muted(),
        TourSectionCategory::Refactor => bg_subtle(),
        TourSectionCategory::Performance => warning_muted(),
        TourSectionCategory::Reliability => success_muted(),
        TourSectionCategory::Other => bg_subtle(),
    }
}

fn ai_tour_category_border(category: TourSectionCategory) -> Rgba {
    match category {
        TourSectionCategory::AuthSecurity => danger(),
        TourSectionCategory::DataState => accent(),
        TourSectionCategory::ApiIo => warning(),
        TourSectionCategory::UiUx => border_default(),
        TourSectionCategory::Tests => success(),
        TourSectionCategory::Docs => border_muted(),
        TourSectionCategory::Config => warning(),
        TourSectionCategory::Infra => accent(),
        TourSectionCategory::Refactor => border_default(),
        TourSectionCategory::Performance => warning(),
        TourSectionCategory::Reliability => success(),
        TourSectionCategory::Other => border_muted(),
    }
}

fn ai_tour_priority_fg(priority: TourSectionPriority) -> Rgba {
    match priority {
        TourSectionPriority::Low => success(),
        TourSectionPriority::Medium => warning(),
        TourSectionPriority::High => danger(),
    }
}

fn ai_tour_priority_bg(priority: TourSectionPriority) -> Rgba {
    match priority {
        TourSectionPriority::Low => success_muted(),
        TourSectionPriority::Medium => warning_muted(),
        TourSectionPriority::High => danger_muted(),
    }
}

fn ai_tour_priority_border(priority: TourSectionPriority) -> Rgba {
    match priority {
        TourSectionPriority::Low => success(),
        TourSectionPriority::Medium => warning(),
        TourSectionPriority::High => danger(),
    }
}

fn render_ai_tour_section(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    generated_tour: &GeneratedCodeTour,
    section: &TourSection,
    cx: &App,
) -> impl IntoElement {
    let section_steps = section
        .step_ids
        .iter()
        .filter_map(|step_id| {
            generated_tour
                .steps
                .iter()
                .find(|step| step.id.as_str() == step_id.as_str())
        })
        .collect::<Vec<_>>();
    let metrics = ai_tour_section_metrics(generated_tour, section);

    nested_panel()
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .min_w_0()
                        .child(eyebrow(section.category.label()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .min_w_0()
                                .child(render_ai_tour_category_icon(section.category, 34.0, 17.0))
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .min_w_0()
                                        .line_clamp(2)
                                        .child(section.title.clone()),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(render_ai_tour_priority_chip(section.priority))
                        .child(badge(&section.badge))
                        .child(ai_tour_metric_chip(&format!(
                            "{} file{}",
                            metrics.file_count,
                            if metrics.file_count == 1 { "" } else { "s" }
                        )))
                        .child(ai_tour_metric_chip(&format!(
                            "+{} / -{}",
                            metrics.additions, metrics.deletions
                        ))),
                ),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(fg_default())
                .mt(px(10.0))
                .child(SelectableText::new(
                    format!("ai-tour-section-summary-{}", section.id),
                    section.summary.clone(),
                )),
        )
        .when(!section.detail.trim().is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(fg_muted())
                    .mt(px(8.0))
                    .child(SelectableText::new(
                        format!("ai-tour-section-detail-{}", section.id),
                        section.detail.clone(),
                    )),
            )
        })
        .when(!section.review_points.is_empty(), |el| {
            el.child(render_ai_tour_review_points(
                &section.id,
                &section.review_points,
            ))
        })
        .child(
            div()
                .mt(px(16.0))
                .pt(px(16.0))
                .border_t(px(1.0))
                .border_color(border_muted())
                .flex()
                .flex_col()
                .gap(px(14.0))
                .children(section_steps.into_iter().map(|step| {
                    render_ai_tour_step_diff(state, detail, section, step, cx).into_any_element()
                })),
        )
}

fn render_ai_tour_review_points(section_id: &str, review_points: &[String]) -> impl IntoElement {
    div().mt(px(12.0)).flex().flex_col().gap(px(8.0)).children(
        review_points.iter().enumerate().map(|(point_ix, point)| {
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(SelectableText::new(
                    format!("ai-tour-review-point-{section_id}-{point_ix}"),
                    point.clone(),
                ))
        }),
    )
}

fn render_ai_tour_step_diff(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    section: &TourSection,
    step: &TourStep,
    cx: &App,
) -> impl IntoElement {
    let preview_key = format!("ai-tour:{}:{}", section.id, step.id);

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .font_family(mono_font_family())
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(step.title.clone()),
                        )
                        .when(!step.summary.trim().is_empty(), |el| {
                            el.child(div().text_size(px(12.0)).text_color(fg_muted()).child(
                                SelectableText::new(
                                    format!("ai-tour-step-summary-{}", step.id),
                                    step.summary.clone(),
                                ),
                            ))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .child(badge(&format!("+{}", step.additions)))
                        .child(badge(&format!("-{}", step.deletions)))
                        .when(step.unresolved_thread_count > 0, |el| {
                            el.child(badge(&format!(
                                "{} thread{}",
                                step.unresolved_thread_count,
                                if step.unresolved_thread_count == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            )))
                        }),
                ),
        )
        .child(render_tour_diff_file_compact(
            state,
            detail,
            &preview_key,
            step.file_path.as_deref(),
            step.snippet.as_deref(),
            step.anchor.as_ref(),
            cx,
        ))
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
    let selected_edge_transparent = with_alpha(diff_selected_edge(), 0.0);

    div()
        .id(id)
        .w(px(22.0))
        .h(px(22.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            diff_selected_edge()
        } else {
            transparent()
        })
        .bg(if active {
            diff_line_hover_bg()
        } else {
            transparent()
        })
        .opacity(if disabled { 0.42 } else { 1.0 })
        .flex()
        .items_center()
        .justify_center()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .when(!disabled, move |el| {
            el.cursor_pointer()
                .hover(|style| {
                    style
                        .bg(diff_line_hover_bg())
                        .border_color(diff_annotation_border())
                })
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(icon)
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), diff_line_hover_bg(), progress))
                    .border_color(mix_rgba(
                        selected_edge_transparent,
                        diff_selected_edge(),
                        progress,
                    ))
            },
        )
}

fn render_file_tree_toggle_icon(active: bool) -> AnyElement {
    let color = if active { accent() } else { fg_muted() };

    lucide_icon(
        if active {
            LucideIcon::PanelLeftClose
        } else {
            LucideIcon::PanelLeftOpen
        },
        14.0,
        color,
    )
    .into_any_element()
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
    let focus_border_transparent = with_alpha(focus_border(), 0.0);

    div()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
        .bg(if active {
            diff_line_hover_bg()
        } else {
            transparent()
        })
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(diff_line_hover_bg())
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), diff_line_hover_bg(), progress))
                    .border_color(mix_rgba(focus_border_transparent, focus_border(), progress))
                    .text_color(mix_rgba(fg_muted(), fg_emphasis(), progress))
            },
        )
}

fn render_file_diff(
    state: &Entity<AppState>,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    semantic_file: Option<&SemanticDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    selected_anchor: Option<&DiffAnchor>,
    diff_view_state: DiffFileViewState,
    review_stack: Arc<ReviewStack>,
    stack_filter: Option<LayerDiffFilter>,
    cx: &App,
) -> impl IntoElement {
    let rows = diff_view_state.rows.clone();
    let parsed_file_index = diff_view_state.parsed_file_index;
    let highlighted_hunks = diff_view_state.highlighted_hunks.clone();
    let reserve_waypoint_slot = state
        .read(cx)
        .active_review_session()
        .map(|session| {
            session.waymarks.iter().any(|waymark| {
                waymark.location.mode == ReviewCenterMode::SemanticDiff
                    && waymark.location.file_path == file.path
            })
        })
        .unwrap_or(false);
    let gutter_layout = diff_gutter_layout(file, parsed, reserve_waypoint_slot);
    let selected_anchor = selected_anchor.cloned();
    let list_state = diff_view_state.list_state.clone();
    let prepared_file = prepared_file.cloned();
    let file_lsp_context =
        build_diff_file_lsp_context(state, file.path.as_str(), prepared_file.as_ref(), cx);
    let semantic_sections =
        semantic_file.map(|semantic_file| Arc::new(semantic_file.sections.clone()));
    let stack_visibility = stack_filter
        .as_ref()
        .map(|filter| stack_file_visibility(review_stack.as_ref(), filter, &file.path, parsed));

    let items = build_diff_view_items(
        state,
        file,
        parsed,
        semantic_file,
        prepared_file.as_ref(),
        &rows,
        stack_visibility.as_ref(),
        cx,
    );

    if list_state.item_count() != items.len() {
        list_state.reset(items.len());
    }

    if let Some(active_pr_key) = state.read(cx).active_pr_key.clone() {
        let state_for_scroll = state.clone();
        let list_state_for_scroll = list_state.clone();
        list_state.set_scroll_handler(move |_, window, _| {
            let state = state_for_scroll.clone();
            let list_state = list_state_for_scroll.clone();
            let active_pr_key = active_pr_key.clone();
            window.on_next_frame(move |_, cx| {
                let scroll_top = list_state.logical_scroll_top();
                let compact = scroll_top.item_ix > 0 || scroll_top.offset_in_item > px(0.0);
                state.update(cx, |state, cx| {
                    if state.active_surface != PullRequestSurface::Files
                        || state.active_pr_key.as_deref() != Some(active_pr_key.as_str())
                        || state.pr_header_compact == compact
                    {
                        return;
                    }

                    state.pr_header_compact = compact;
                    cx.notify();
                });
            });
        });
    }

    let items = Arc::new(items);
    let state = state.clone();

    div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .bg(diff_editor_bg())
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow()
                .min_h_0()
                .bg(diff_editor_bg())
                .when_some(stack_visibility.clone(), |el, visibility| {
                    el.child(render_stack_layer_diff_notice(
                        review_stack.as_ref(),
                        &visibility,
                    ))
                })
                .child(
                    render_virtualized_diff_rows(
                        &state,
                        rows,
                        semantic_sections,
                        gutter_layout,
                        parsed_file_index,
                        highlighted_hunks,
                        file_lsp_context,
                        selected_anchor,
                        list_state,
                        items,
                    )
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                ),
        )
}

fn render_virtualized_diff_rows(
    state: &Entity<AppState>,
    rows: Arc<Vec<DiffRenderRow>>,
    semantic_sections: Option<Arc<Vec<SemanticDiffSection>>>,
    gutter_layout: DiffGutterLayout,
    parsed_file_index: Option<usize>,
    highlighted_hunks: Option<Arc<Vec<Vec<DiffLineHighlight>>>>,
    file_lsp_context: Option<DiffFileLspContext>,
    selected_anchor: Option<DiffAnchor>,
    list_state: ListState,
    items: Arc<Vec<DiffViewItem>>,
) -> List {
    let state = state.clone();

    list(list_state, move |ix, _window, cx| match items[ix] {
        DiffViewItem::SemanticSection(section_ix) => semantic_sections
            .as_ref()
            .and_then(|sections| sections.get(section_ix))
            .map(|section| {
                render_semantic_section_header(&state, section, selected_anchor.as_ref(), cx)
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffViewItem::Gap(gap) => render_diff_gap_row(gap, gutter_layout).into_any_element(),
        DiffViewItem::StackLayerEmpty => render_diff_state_row(
            "No changed hunks in this file belong to the selected stack layer.",
        )
        .into_any_element(),
        DiffViewItem::Row(row_ix) => render_virtualized_diff_row(
            &state,
            gutter_layout,
            parsed_file_index,
            highlighted_hunks.as_deref(),
            file_lsp_context.as_ref(),
            &rows[row_ix],
            selected_anchor.as_ref(),
            cx,
        )
        .into_any_element(),
    })
}

#[derive(Clone, Copy)]
enum DiffViewItem {
    Row(usize),
    Gap(DiffGapSummary),
    SemanticSection(usize),
    StackLayerEmpty,
}

#[derive(Clone)]
struct StackFileVisibility {
    mode: StackDiffMode,
    layer_id: Option<String>,
    layer_title: String,
    layer_rationale: String,
    layer_warnings: Vec<crate::stacks::model::StackWarning>,
    confidence: Confidence,
    ai_assisted: bool,
    visible_hunk_indices: Option<BTreeSet<usize>>,
    visible_atom_count: usize,
    total_hunk_count: usize,
    hidden_hunk_count: usize,
    file_has_visible_atoms: bool,
}

fn build_layer_diff_filter(
    stack: &ReviewStack,
    mode: StackDiffMode,
    selected_layer_id: Option<&str>,
    reviewed_atom_ids: &std::collections::HashSet<ChangeAtomId>,
) -> Option<LayerDiffFilter> {
    if mode == StackDiffMode::WholePr {
        return None;
    }

    let selected_index = stack.selected_layer_index(selected_layer_id)?;
    let mut visible_atom_ids = BTreeSet::<ChangeAtomId>::new();

    for (index, layer) in stack.layers.iter().enumerate() {
        let include = match mode {
            StackDiffMode::WholePr => false,
            StackDiffMode::CurrentLayerOnly => index == selected_index,
            StackDiffMode::UpToCurrentLayer => index <= selected_index,
            StackDiffMode::CurrentAndDependents => index >= selected_index,
            StackDiffMode::SinceLastReviewed => true,
        };

        if !include {
            continue;
        }

        for atom_id in &layer.atom_ids {
            if mode == StackDiffMode::SinceLastReviewed && reviewed_atom_ids.contains(atom_id) {
                continue;
            }
            visible_atom_ids.insert(atom_id.clone());
        }
    }

    if visible_atom_ids.is_empty() && stack.kind == crate::stacks::model::StackKind::Real {
        return None;
    }

    Some(LayerDiffFilter {
        mode,
        selected_layer_id: stack
            .selected_layer(selected_layer_id)
            .map(|layer| layer.id.clone()),
        visible_atom_ids,
    })
}

fn stack_file_visibility(
    stack: &ReviewStack,
    filter: &LayerDiffFilter,
    file_path: &str,
    parsed: Option<&ParsedDiffFile>,
) -> StackFileVisibility {
    let selected_layer = stack.selected_layer(filter.selected_layer_id.as_deref());
    let mut visible_hunks = BTreeSet::<usize>::new();
    let mut show_whole_file = false;
    let mut visible_atom_count = 0usize;

    for atom_id in &filter.visible_atom_ids {
        let Some(atom) = stack.atom(atom_id) else {
            continue;
        };
        if atom.path != file_path {
            continue;
        }
        visible_atom_count += 1;
        if atom.hunk_indices.is_empty() {
            show_whole_file = true;
        } else {
            visible_hunks.extend(atom.hunk_indices.iter().copied());
        }
    }

    let total_hunk_count = parsed.map(|parsed| parsed.hunks.len()).unwrap_or(0);
    let file_has_visible_atoms = visible_atom_count > 0;
    let visible_hunk_indices = if show_whole_file {
        None
    } else {
        Some(visible_hunks)
    };
    let visible_hunk_count = visible_hunk_indices
        .as_ref()
        .map(|hunks| hunks.len())
        .unwrap_or(total_hunk_count);
    let hidden_hunk_count = if file_has_visible_atoms {
        total_hunk_count.saturating_sub(visible_hunk_count)
    } else {
        total_hunk_count
    };

    StackFileVisibility {
        mode: filter.mode,
        layer_id: selected_layer.map(|layer| layer.id.clone()),
        layer_title: selected_layer
            .map(|layer| layer.title.clone())
            .unwrap_or_else(|| "Stack layer".to_string()),
        layer_rationale: selected_layer
            .map(|layer| layer.rationale.clone())
            .unwrap_or_default(),
        layer_warnings: selected_layer
            .map(|layer| layer.warnings.clone())
            .unwrap_or_default(),
        confidence: selected_layer
            .map(|layer| layer.confidence)
            .unwrap_or(stack.confidence),
        ai_assisted: stack.source == crate::stacks::model::StackSource::VirtualAi,
        visible_hunk_indices,
        visible_atom_count,
        total_hunk_count,
        hidden_hunk_count,
        file_has_visible_atoms,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffGapPosition {
    Start,
    Between,
    End,
}

#[derive(Clone, Copy)]
struct DiffGapSummary {
    position: DiffGapPosition,
    hidden_count: usize,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

#[derive(Clone)]
struct DiffFileLspContext {
    state: Entity<AppState>,
    detail_key: String,
    lsp_session_manager: Arc<lsp::LspSessionManager>,
    repo_root: PathBuf,
    file_path: String,
    reference: String,
    document_text: Arc<str>,
}

fn build_diff_file_lsp_context(
    state: &Entity<AppState>,
    file_path: &str,
    prepared_file: Option<&PreparedFileContent>,
    cx: &App,
) -> Option<DiffFileLspContext> {
    let prepared_file = prepared_file?;
    if prepared_file.is_binary || prepared_file.text.is_empty() {
        return None;
    }

    let app_state = state.read(cx);
    let detail_key = app_state.active_pr_key.clone()?;
    let detail_state = app_state.detail_states.get(&detail_key)?;
    let local_repo_status = detail_state.local_repository_status.as_ref()?;
    if !local_repo_status.ready_for_snapshot_features() {
        return None;
    }

    let repo_root = PathBuf::from(local_repo_status.path.as_ref()?);
    let lsp_status = detail_state.lsp_statuses.get(file_path)?;
    if !lsp_status.is_ready()
        || (!lsp_status.capabilities.hover_supported
            && !lsp_status.capabilities.signature_help_supported
            && !lsp_status.capabilities.definition_supported)
    {
        return None;
    }

    Some(DiffFileLspContext {
        state: state.clone(),
        detail_key,
        lsp_session_manager: app_state.lsp_session_manager.clone(),
        repo_root,
        file_path: file_path.to_string(),
        reference: prepared_file.reference.clone(),
        document_text: prepared_file.text.clone(),
    })
}

#[derive(Clone)]
struct DiffLineLspContext {
    file: DiffFileLspContext,
    line_number: usize,
}

#[derive(Clone)]
struct DiffLineLspQuery {
    state: Entity<AppState>,
    detail_key: String,
    lsp_session_manager: Arc<lsp::LspSessionManager>,
    repo_root: PathBuf,
    query_key: String,
    token_label: String,
    request: lsp::LspTextDocumentRequest,
}

fn build_diff_line_lsp_context(
    file_context: Option<&DiffFileLspContext>,
    line: &ParsedDiffLine,
) -> Option<DiffLineLspContext> {
    let line_number = usize::try_from(line.right_line_number?).ok()?;
    if line_number == 0 {
        return None;
    }

    Some(DiffLineLspContext {
        file: file_context?.clone(),
        line_number,
    })
}

impl DiffLineLspContext {
    fn query_for_index(
        &self,
        index: usize,
        tokens: &[InteractiveCodeToken],
    ) -> Option<DiffLineLspQuery> {
        let token = tokens
            .iter()
            .find(|token| token.byte_range.contains(&index))?;

        Some(DiffLineLspQuery {
            state: self.file.state.clone(),
            detail_key: self.file.detail_key.clone(),
            lsp_session_manager: self.file.lsp_session_manager.clone(),
            repo_root: self.file.repo_root.clone(),
            query_key: format!(
                "{}:{}:{}:{}",
                self.file.file_path, self.file.reference, self.line_number, token.column_start
            ),
            token_label: display_lsp_token_label(&token.text),
            request: lsp::LspTextDocumentRequest {
                file_path: self.file.file_path.clone(),
                document_text: self.file.document_text.clone(),
                line: self.line_number,
                column: token.column_start,
            },
        })
    }
}

fn display_lsp_token_label(text: &str) -> String {
    let trimmed = text.trim();
    let mut label = trimmed.chars().take(48).collect::<String>();
    if trimmed.chars().count() > 48 {
        label.push('…');
    }
    label
}

fn should_request_diff_line_lsp_details(query: &DiffLineLspQuery, cx: &App) -> bool {
    query
        .state
        .read(cx)
        .detail_states
        .get(&query.detail_key)
        .and_then(|detail_state| detail_state.lsp_symbol_states.get(&query.query_key))
        .map(|state| !state.loading && state.details.is_none() && state.error.is_none())
        .unwrap_or(true)
}

fn request_diff_line_lsp_details(query: DiffLineLspQuery, window: &mut Window, cx: &mut App) {
    if !should_request_diff_line_lsp_details(&query, cx) {
        return;
    }

    let query_key = query.query_key.clone();
    let detail_key = query.detail_key.clone();
    let state = query.state.clone();

    state.update(cx, |state, cx| {
        let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
            return;
        };
        let symbol_state = detail_state
            .lsp_symbol_states
            .entry(query_key.clone())
            .or_default();
        if symbol_state.loading || symbol_state.details.is_some() || symbol_state.error.is_some() {
            return;
        }
        symbol_state.loading = true;
        symbol_state.details = None;
        symbol_state.error = None;
        cx.notify();
    });

    window
        .spawn(cx, {
            let state = state.clone();
            let detail_key = detail_key.clone();
            let query_key = query_key.clone();
            let lsp_session_manager = query.lsp_session_manager.clone();
            let repo_root = query.repo_root.clone();
            let request = query.request.clone();
            async move |cx: &mut AsyncWindowContext| {
                let result = cx
                    .background_executor()
                    .spawn(async move { lsp_session_manager.symbol_details(&repo_root, &request) })
                    .await;

                state
                    .update(cx, |state, cx| {
                        let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                            return;
                        };
                        let symbol_state = detail_state
                            .lsp_symbol_states
                            .entry(query_key.clone())
                            .or_default();
                        symbol_state.loading = false;
                        match result {
                            Ok(details) => {
                                symbol_state.details = Some(details);
                                symbol_state.error = None;
                            }
                            Err(error) => {
                                symbol_state.details = None;
                                symbol_state.error = Some(error);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
            }
        })
        .detach();
}

fn navigate_to_diff_lsp_definition(query: DiffLineLspQuery, window: &mut Window, cx: &mut App) {
    // Try to read cached definition targets
    let targets = query
        .state
        .read(cx)
        .detail_states
        .get(&query.detail_key)
        .and_then(|detail_state| detail_state.lsp_symbol_states.get(&query.query_key))
        .and_then(|symbol_state| symbol_state.details.as_ref())
        .map(|details| details.definition_targets.clone());

    if let Some(targets) = targets.filter(|t| !t.is_empty()) {
        navigate_to_definition_target(&query.state, &targets[0], window, cx);
        return;
    }

    // Not cached — fetch definition asynchronously, then navigate
    let state = query.state.clone();
    window
        .spawn(cx, {
            let lsp_session_manager = query.lsp_session_manager.clone();
            let repo_root = query.repo_root.clone();
            let request = query.request.clone();
            async move |cx: &mut AsyncWindowContext| {
                let result = cx
                    .background_executor()
                    .spawn(async move { lsp_session_manager.definition(&repo_root, &request) })
                    .await;

                if let Ok(targets) = result {
                    if let Some(target) = targets.first() {
                        let target = target.clone();
                        state
                            .update(cx, |state, cx| {
                                state.navigate_to_review_location(
                                    ReviewLocation::from_source(
                                        target.path.clone(),
                                        Some(target.line),
                                        Some("Jumped to definition".to_string()),
                                    ),
                                    true,
                                );
                                state.persist_active_review_session();
                                cx.notify();
                            })
                            .ok();

                        load_local_source_file_content_flow(state, target.path.clone(), cx).await;
                    }
                }
            }
        })
        .detach();
}

fn navigate_to_definition_target(
    state: &Entity<AppState>,
    target: &lsp::LspDefinitionTarget,
    window: &mut Window,
    cx: &mut App,
) {
    open_review_source_location(
        state,
        target.path.clone(),
        Some(target.line),
        Some("Jumped to definition".to_string()),
        window,
        cx,
    );
}

fn build_diff_view_items(
    state: &Entity<AppState>,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    semantic_file: Option<&SemanticDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    rows: &[DiffRenderRow],
    stack_visibility: Option<&StackFileVisibility>,
    cx: &App,
) -> Vec<DiffViewItem> {
    let mut items = Vec::with_capacity(rows.len() + 4);
    let mut last_hunk_index = None;
    let mut current_section_ix = None;
    let last_hunk_row_index = rows.iter().rposition(|row| {
        matches!(
            row,
            DiffRenderRow::HunkHeader { .. } | DiffRenderRow::Line { .. }
        )
    });
    let collapsed_sections = state
        .read(cx)
        .active_review_session()
        .map(|session| session.collapsed_sections.clone())
        .unwrap_or_default();
    let mut current_hunk_visible = true;
    let mut emitted_visible_stack_hunk = false;

    for (row_index, row) in rows.iter().enumerate() {
        if let DiffRenderRow::HunkHeader { hunk_index } = row {
            current_hunk_visible = stack_visibility
                .map(|visibility| {
                    visibility
                        .visible_hunk_indices
                        .as_ref()
                        .map(|visible| visible.contains(hunk_index))
                        .unwrap_or(visibility.file_has_visible_atoms)
                })
                .unwrap_or(true);
            if let Some(section_ix) = semantic_file
                .and_then(|semantic_file| semantic_file.section_index_for_hunk(*hunk_index))
            {
                if current_hunk_visible && current_section_ix != Some(section_ix) {
                    items.push(DiffViewItem::SemanticSection(section_ix));
                    current_section_ix = Some(section_ix);
                }
            } else {
                current_section_ix = None;
            }

            if current_hunk_visible {
                if let Some(gap) =
                    diff_gap_before_hunk(file, parsed, prepared_file, last_hunk_index, *hunk_index)
                {
                    items.push(DiffViewItem::Gap(gap));
                }
                last_hunk_index = Some(*hunk_index);
                emitted_visible_stack_hunk = true;
            }
        }

        let is_collapsed = current_section_ix
            .and_then(|section_ix| {
                semantic_file.and_then(|semantic_file| semantic_file.sections.get(section_ix))
            })
            .map(|section| collapsed_sections.contains(&section.id))
            .unwrap_or(false);
        let should_skip = is_collapsed
            && matches!(
                row,
                DiffRenderRow::HunkHeader { .. }
                    | DiffRenderRow::Line { .. }
                    | DiffRenderRow::InlineThread { .. }
            );
        let should_skip = should_skip
            || (!current_hunk_visible
                && matches!(
                    row,
                    DiffRenderRow::HunkHeader { .. }
                        | DiffRenderRow::Line { .. }
                        | DiffRenderRow::InlineThread { .. }
                ));

        if !should_skip {
            items.push(DiffViewItem::Row(row_index));
        }

        if Some(row_index) == last_hunk_row_index {
            if let Some(last_hunk_index) = last_hunk_index {
                if let Some(gap) =
                    diff_gap_after_last_hunk(file, parsed, prepared_file, last_hunk_index)
                {
                    items.push(DiffViewItem::Gap(gap));
                }
            }
        }
    }

    if stack_visibility
        .map(|visibility| !visibility.file_has_visible_atoms || !emitted_visible_stack_hunk)
        .unwrap_or(false)
    {
        items.push(DiffViewItem::StackLayerEmpty);
    }

    items
}

fn diff_gap_before_hunk(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    previous_hunk_index: Option<usize>,
    current_hunk_index: usize,
) -> Option<DiffGapSummary> {
    let parsed = parsed?;
    let current_hunk = parsed.hunks.get(current_hunk_index)?;
    let current_first = first_visible_line_number(file, current_hunk)?;

    match previous_hunk_index {
        Some(previous_hunk_index) => {
            let previous_hunk = parsed.hunks.get(previous_hunk_index)?;
            let previous_last = last_visible_line_number(file, previous_hunk)?;
            if current_first <= previous_last.saturating_add(1) {
                return None;
            }

            let start_line = previous_last.saturating_add(1);
            let end_line = current_first.saturating_sub(1);
            let hidden_count = end_line.saturating_sub(start_line).saturating_add(1);

            Some(DiffGapSummary {
                position: DiffGapPosition::Between,
                hidden_count,
                start_line: Some(start_line),
                end_line: Some(end_line),
            })
        }
        None => {
            if current_first <= 1 {
                return None;
            }

            let total_lines = prepared_file
                .and_then(|prepared| prepared.lines.last().map(|line| line.line_number))
                .unwrap_or(0);
            let end_line = current_first.saturating_sub(1);
            let hidden_count = if total_lines > 0 {
                end_line.min(total_lines)
            } else {
                end_line
            };

            Some(DiffGapSummary {
                position: DiffGapPosition::Start,
                hidden_count,
                start_line: Some(1),
                end_line: Some(end_line),
            })
        }
    }
}

fn diff_gap_after_last_hunk(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    last_hunk_index: usize,
) -> Option<DiffGapSummary> {
    let prepared_file = prepared_file?;
    let parsed = parsed?;
    let last_hunk = parsed.hunks.get(last_hunk_index)?;
    let last_visible = last_visible_line_number(file, last_hunk)?;
    let total_lines = prepared_file
        .lines
        .last()
        .map(|line| line.line_number)
        .unwrap_or(0);

    if total_lines <= last_visible {
        return None;
    }

    Some(DiffGapSummary {
        position: DiffGapPosition::End,
        hidden_count: total_lines.saturating_sub(last_visible),
        start_line: Some(last_visible.saturating_add(1)),
        end_line: Some(total_lines),
    })
}

fn first_visible_line_number(file: &PullRequestFile, hunk: &ParsedDiffHunk) -> Option<usize> {
    hunk.lines
        .iter()
        .find_map(|line| primary_diff_line_number(file, line))
}

fn last_visible_line_number(file: &PullRequestFile, hunk: &ParsedDiffHunk) -> Option<usize> {
    hunk.lines
        .iter()
        .rev()
        .find_map(|line| primary_diff_line_number(file, line))
}

fn primary_diff_line_number(file: &PullRequestFile, line: &ParsedDiffLine) -> Option<usize> {
    let number = if file.change_type == "DELETED" {
        line.left_line_number.or(line.right_line_number)
    } else {
        line.right_line_number.or(line.left_line_number)
    }?;

    if number > 0 {
        Some(number as usize)
    } else {
        None
    }
}

fn render_diff_gap_row(
    summary: DiffGapSummary,
    gutter_layout: DiffGutterLayout,
) -> impl IntoElement {
    let markers = match summary.position {
        DiffGapPosition::Start => vec!["...", "\u{2193}"],
        DiffGapPosition::Between => vec!["\u{2191}", "...", "\u{2193}"],
        DiffGapPosition::End => vec!["\u{2191}", "..."],
    };

    div()
        .flex()
        .items_center()
        .w_full()
        .min_h(px(26.0))
        .bg(diff_annotation_bg())
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .font_family(mono_font_family())
        .text_size(px(11.0))
        .child(
            div()
                .w(px(gutter_layout.gutter_width()))
                .flex_shrink_0()
                .h_full()
                .bg(diff_context_gutter_bg())
                .border_r(px(1.0))
                .border_color(diff_gutter_separator()),
        )
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .px(px(12.0))
                .py(px(4.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .children(markers.into_iter().map(|marker| {
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(999.0))
                                .bg(diff_editor_chrome())
                                .border_1()
                                .border_color(diff_annotation_border())
                                .text_color(accent())
                                .child(marker)
                        })),
                )
                .child(
                    div()
                        .text_color(fg_muted())
                        .child(render_diff_gap_label(summary)),
                ),
        )
}

fn render_stack_layer_diff_notice(
    stack: &ReviewStack,
    visibility: &StackFileVisibility,
) -> impl IntoElement {
    let stack_kind = match stack.kind {
        crate::stacks::model::StackKind::Real => "Real stack",
        crate::stacks::model::StackKind::Virtual => "Virtual stack",
    };
    let hidden = visibility.hidden_hunk_count;
    let visible = visibility
        .total_hunk_count
        .saturating_sub(visibility.hidden_hunk_count);
    let mode_label = visibility.mode.label();

    div()
        .px(px(14.0))
        .py(px(8.0))
        .bg(diff_annotation_bg())
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .min_w_0()
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(accent())
                        .child(format!("{stack_kind} / {mode_label}").to_uppercase()),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(visibility.layer_title.clone()),
                )
                .when(!visibility.layer_rationale.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(fg_muted())
                            .line_clamp(2)
                            .child(if visibility.ai_assisted {
                                format!("Why this layer: {}", visibility.layer_rationale)
                            } else {
                                visibility.layer_rationale.clone()
                            }),
                    )
                })
                .when(visibility.ai_assisted, |el| {
                    el.child(
                        div()
                            .text_size(px(11.0))
                            .font_family(mono_font_family())
                            .text_color(fg_subtle())
                            .child(format!(
                                "Confidence: {}",
                                match visibility.confidence {
                                    Confidence::High => "High",
                                    Confidence::Medium => "Medium",
                                    Confidence::Low => "Low",
                                }
                            )),
                    )
                })
                .when_some(
                    visibility
                        .layer_warnings
                        .first()
                        .map(|warning| warning.message.clone()),
                    |el, warning_message| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(warning())
                                .line_clamp(2)
                                .child(warning_message),
                        )
                    },
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(6.0))
                .flex_wrap()
                .flex_shrink_0()
                .child(badge(visibility.confidence.label()))
                .child(badge(&format!(
                    "{} atom{}",
                    visibility.visible_atom_count,
                    if visibility.visible_atom_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                )))
                .child(badge(&format!(
                    "{} of {} hunk{}",
                    visible,
                    visibility.total_hunk_count,
                    if visibility.total_hunk_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                )))
                .when(hidden > 0, |el| {
                    el.child(badge(&format!(
                        "{hidden} hidden from other layer{}",
                        if hidden == 1 { "" } else { "s" }
                    )))
                }),
        )
}

fn render_diff_gap_label(summary: DiffGapSummary) -> String {
    let line_label = if summary.hidden_count == 1 {
        "1 unchanged line".to_string()
    } else {
        format!("{} unchanged lines", summary.hidden_count)
    };

    match (summary.start_line, summary.end_line) {
        (Some(start), Some(end)) if start == end => {
            format!("{line_label} hidden at line {start}")
        }
        (Some(start), Some(end)) => format!("{line_label} hidden ({start}-{end})"),
        _ => format!("{line_label} hidden"),
    }
}

fn render_semantic_section_header(
    state: &Entity<AppState>,
    section: &SemanticDiffSection,
    selected_anchor: Option<&DiffAnchor>,
    cx: &App,
) -> impl IntoElement {
    let state_for_open = state.clone();
    let state_for_toggle = state.clone();
    let path = section
        .anchor
        .as_ref()
        .map(|anchor| anchor.file_path.clone())
        .unwrap_or_default();
    let anchor = section.anchor.clone();
    let section_id = section.id.clone();
    let is_selected = selected_anchor
        .and_then(|selected_anchor| selected_anchor.hunk_header.as_deref())
        .zip(
            section
                .anchor
                .as_ref()
                .and_then(|anchor| anchor.hunk_header.as_deref()),
        )
        .map(|(left, right)| left == right)
        .unwrap_or(false)
        || selected_anchor
            .and_then(|selected_anchor| selected_anchor.line)
            .zip(section.anchor.as_ref().and_then(|anchor| anchor.line))
            .map(|(left, right)| left == right)
            .unwrap_or(false);
    let collapsed = state.read(cx).is_review_section_collapsed(&section.id);

    div()
        .px(px(14.0))
        .py(px(5.0))
        .border_b(px(1.0))
        .border_color(if is_selected {
            diff_selected_edge()
        } else {
            diff_annotation_border()
        })
        .bg(if is_selected {
            diff_line_hover_bg()
        } else {
            diff_hunk_bg()
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .min_w_0()
                        .cursor_pointer()
                        .hover(|style| style.text_color(fg_emphasis()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            open_review_diff_location(
                                &state_for_open,
                                path.clone(),
                                anchor.clone(),
                                window,
                                cx,
                            );
                        })
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_family(mono_font_family())
                                .text_color(diff_hunk_fg())
                                .flex_shrink_0()
                                .child(section.kind.label().to_ascii_uppercase()),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_family(mono_font_family())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .flex_grow()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(section.title.clone()),
                        ),
                )
                .child(workspace_mode_button(
                    if collapsed { "Expand" } else { "Fold" },
                    false,
                    move |_, _, cx| {
                        state_for_toggle.update(cx, |state, cx| {
                            state.toggle_review_section_collapse(&section_id);
                            state.persist_active_review_session();
                            cx.notify();
                        });
                    },
                )),
        )
}

fn prepare_diff_view_state(
    app_state: &AppState,
    detail: &PullRequestDetail,
    file_path: &str,
) -> DiffFileViewState {
    prepare_diff_view_state_with_key(
        app_state,
        detail,
        build_diff_view_state_key(app_state.active_pr_key.as_deref(), "files", file_path),
        file_path,
    )
}

fn prepare_tour_diff_view_state(
    app_state: &AppState,
    detail: &PullRequestDetail,
    preview_key: &str,
    file_path: &str,
) -> DiffFileViewState {
    prepare_diff_view_state_with_key(
        app_state,
        detail,
        build_diff_view_state_key(app_state.active_pr_key.as_deref(), "tour", preview_key),
        file_path,
    )
}

fn build_diff_view_state_key(active_pr_key: Option<&str>, surface: &str, item_key: &str) -> String {
    format!(
        "{surface}:{}:{item_key}",
        active_pr_key.unwrap_or("detached")
    )
}

fn prepare_diff_view_state_with_key(
    app_state: &AppState,
    detail: &PullRequestDetail,
    state_key: String,
    file_path: &str,
) -> DiffFileViewState {
    let revision = detail.updated_at.clone();
    let mut diff_view_states = app_state.diff_view_states.borrow_mut();
    let entry = diff_view_states.entry(state_key).or_insert_with(|| {
        let (parsed_file_index, highlighted_hunks) =
            find_parsed_diff_file_with_index(&detail.parsed_diff, file_path)
                .map(|(ix, file)| (Some(ix), Some(build_diff_highlights(file))))
                .unwrap_or((None, None));
        DiffFileViewState::new(
            Arc::new(build_diff_render_rows(detail, file_path)),
            revision.clone(),
            parsed_file_index,
            highlighted_hunks,
        )
    });

    let needs_highlight_refresh =
        entry.highlighted_hunks.is_none() && entry.parsed_file_index.is_some();

    if entry.revision != revision || needs_highlight_refresh {
        let (parsed_file_index, highlighted_hunks) =
            find_parsed_diff_file_with_index(&detail.parsed_diff, file_path)
                .map(|(ix, file)| (Some(ix), Some(build_diff_highlights(file))))
                .unwrap_or((None, None));
        if entry.revision != revision {
            entry.rows = Arc::new(build_diff_render_rows(detail, file_path));
            entry.revision = revision.clone();
            entry.list_state.reset(0);
        }
        entry.revision = revision;
        entry.parsed_file_index = parsed_file_index;
        entry.highlighted_hunks = highlighted_hunks;
    }

    entry.clone()
}

fn render_virtualized_diff_row(
    state: &Entity<AppState>,
    gutter_layout: DiffGutterLayout,
    parsed_file_index: Option<usize>,
    highlighted_hunks: Option<&Vec<Vec<DiffLineHighlight>>>,
    file_lsp_context: Option<&DiffFileLspContext>,
    row: &DiffRenderRow,
    selected_anchor: Option<&DiffAnchor>,
    cx: &App,
) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();
    let parsed_file =
        parsed_file_index.and_then(|ix| detail.and_then(|detail| detail.parsed_diff.get(ix)));

    match row {
        DiffRenderRow::FileCommentsHeader { count } => {
            render_diff_section_header("File comments", *count).into_any_element()
        }
        DiffRenderRow::OutdatedCommentsHeader { count } => {
            render_diff_section_header("Outdated comments", *count).into_any_element()
        }
        DiffRenderRow::FileCommentThread { thread_index } => detail
            .and_then(|detail| detail.review_threads.get(*thread_index))
            .map(|thread| {
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b(px(1.0))
                    .border_color(diff_annotation_border())
                    .child(render_review_thread(
                        thread,
                        selected_anchor,
                        &s.unread_review_comment_ids,
                        state,
                    ))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::InlineThread { thread_index } => detail
            .and_then(|detail| detail.review_threads.get(*thread_index))
            .map(|thread| {
                div()
                    .pl(px(gutter_layout.inline_thread_inset()))
                    .pr(px(16.0))
                    .py(px(10.0))
                    .border_b(px(1.0))
                    .border_color(diff_annotation_border())
                    .bg(diff_annotation_bg())
                    .child(render_review_thread(
                        thread,
                        selected_anchor,
                        &s.unread_review_comment_ids,
                        state,
                    ))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::OutdatedThread { thread_index } => detail
            .and_then(|detail| detail.review_threads.get(*thread_index))
            .map(|thread| {
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b(px(1.0))
                    .border_color(diff_annotation_border())
                    .bg(diff_annotation_bg())
                    .child(render_review_thread(
                        thread,
                        selected_anchor,
                        &s.unread_review_comment_ids,
                        state,
                    ))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::HunkHeader { hunk_index } => parsed_file
            .and_then(|parsed| parsed.hunks.get(*hunk_index))
            .map(|hunk| render_hunk_header(hunk, selected_anchor).into_any_element())
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::Line {
            hunk_index,
            line_index,
        } => parsed_file
            .and_then(|parsed| {
                let path = parsed.path.as_str();
                parsed.hunks.get(*hunk_index).and_then(|hunk| {
                    hunk.lines.get(*line_index).map(|line| {
                        let hunk_header = hunk.header.as_str();
                        let highlight = highlighted_hunks
                            .and_then(|hunks| hunks.get(*hunk_index))
                            .and_then(|lines| lines.get(*line_index))
                            .cloned()
                            .unwrap_or_default();
                        let line_lsp_context = build_diff_line_lsp_context(file_lsp_context, line);
                        render_reviewable_diff_line(
                            state,
                            gutter_layout,
                            path,
                            Some(hunk_header),
                            line,
                            Some(highlight.syntax_spans.as_slice()),
                            Some(highlight.emphasis_ranges.as_slice()),
                            selected_anchor,
                            line_lsp_context.as_ref(),
                            cx,
                        )
                        .into_any_element()
                    })
                })
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::NoTextHunks => render_diff_state_row(
            if parsed_file.map(|parsed| parsed.is_binary).unwrap_or(false) {
                "Binary file not displayed in the unified diff."
            } else {
                "No textual hunks available for this file."
            },
        )
        .into_any_element(),
        DiffRenderRow::RawDiffFallback => {
            render_raw_diff_fallback(detail.map(|detail| detail.raw_diff.as_str()).unwrap_or(""))
                .into_any_element()
        }
        DiffRenderRow::NoParsedDiff => {
            render_diff_state_row("No parsed diff is available for this file.").into_any_element()
        }
    }
}

fn render_diff_section_header(label: &str, count: usize) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(6.0))
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .bg(diff_annotation_bg())
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(11.0))
                .font_family(mono_font_family())
                .text_color(fg_muted())
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .text_size(px(11.0))
                .font_family(mono_font_family())
                .text_color(fg_subtle())
                .child(count.to_string()),
        )
}

fn render_diff_state_row(message: &str) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(18.0))
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .bg(diff_editor_bg())
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(message.to_string()),
        )
}

fn render_raw_diff_fallback(raw_diff: &str) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(16.0))
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .bg(diff_editor_bg())
        .child(if raw_diff.is_empty() {
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child("No diff returned.".to_string())
                .into_any_element()
        } else {
            render_highlighted_code_content("diff.patch", raw_diff).into_any_element()
        })
}

fn render_change_type_chip(change_type: &str) -> impl IntoElement {
    let (bg, fg, _border) = match change_type {
        "ADDED" => (success_muted(), success(), diff_add_border()),
        "DELETED" => (danger_muted(), danger(), diff_remove_border()),
        "RENAMED" | "COPIED" => (accent_muted(), accent(), accent()),
        _ => (bg_subtle(), fg_muted(), border_muted()),
    };

    metric_pill(label_for_change_type(change_type).to_string(), fg, bg)
}

fn render_file_stat_bar(additions: i64, deletions: i64) -> impl IntoElement {
    let total = additions + deletions;
    let segments = 8usize;
    let additions = additions.max(0) as usize;
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
        .gap(px(2.0))
        .children((0..segments).map(move |ix| {
            let bg = if ix < add_segments {
                success()
            } else if ix < add_segments + delete_segments {
                danger()
            } else {
                border_muted()
            };

            div().w(px(8.0)).h(px(4.0)).rounded(px(999.0)).bg(bg)
        }))
}

fn build_review_line_action_target(
    file_path: &str,
    hunk_header: Option<&str>,
    line: &ParsedDiffLine,
) -> Option<ReviewLineActionTarget> {
    let side = if matches!(line.kind, DiffLineKind::Deletion) {
        Some("LEFT")
    } else if matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Context) {
        Some("RIGHT")
    } else {
        None
    }?;

    let line_number = match side {
        "LEFT" => line.left_line_number,
        _ => line.right_line_number,
    }?;
    let display_line = usize::try_from(line_number).ok().filter(|line| *line > 0)?;

    Some(ReviewLineActionTarget {
        anchor: DiffAnchor {
            file_path: file_path.to_string(),
            hunk_header: hunk_header.map(str::to_string),
            line: Some(line_number),
            side: Some(side.to_string()),
            thread_id: None,
        },
        label: format!("{file_path}:{display_line}"),
    })
}

fn render_reviewable_diff_line(
    state: &Entity<AppState>,
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    hunk_header: Option<&str>,
    line: &ParsedDiffLine,
    syntax_spans: Option<&[SyntaxSpan]>,
    emphasis_ranges: Option<&[DiffInlineRange]>,
    selected_anchor: Option<&DiffAnchor>,
    lsp_context: Option<&DiffLineLspContext>,
    cx: &App,
) -> impl IntoElement {
    let line_action_target = build_review_line_action_target(file_path, hunk_header, line);
    let (active_line_action, waypoint) = {
        let app_state = state.read(cx);
        let active_line_action = app_state.active_review_line_action.clone();
        let waypoint = line_action_target
            .as_ref()
            .and_then(|target| {
                app_state
                    .active_review_session()
                    .and_then(|session| session.waymark_for_location(&target.review_location()))
            })
            .cloned();
        (active_line_action, waypoint)
    };

    let popup_open = line_action_target
        .as_ref()
        .zip(active_line_action.as_ref())
        .map(|(line_target, active_target)| line_target.stable_key() == active_target.stable_key())
        .unwrap_or(false);
    let has_waypoint = !popup_open && waypoint.is_some();

    render_diff_line(
        gutter_layout,
        file_path,
        line,
        syntax_spans,
        emphasis_ranges,
        selected_anchor,
        lsp_context,
        line_action_target.map(|target| (state.clone(), target)),
        has_waypoint,
        popup_open,
    )
}

fn render_diff_waypoint_icon() -> impl IntoElement {
    div()
        .relative()
        .w(px(12.0))
        .h(px(12.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(waypoint_icon_border())
        .bg(waypoint_icon_bg())
        .child(
            div()
                .absolute()
                .left(px(3.0))
                .top(px(3.0))
                .w(px(4.0))
                .h(px(4.0))
                .rounded(px(999.0))
                .bg(waypoint_icon_core()),
        )
}

fn build_static_tooltip(text: &'static str, cx: &mut App) -> AnyView {
    build_text_tooltip(SharedString::from(text), cx)
}

fn build_text_tooltip(text: SharedString, cx: &mut App) -> AnyView {
    AnyView::from(cx.new(|_| StaticTooltipView { text }))
}

struct StaticTooltipView {
    text: SharedString,
}

impl Render for StaticTooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(radius_sm())
            .border_1()
            .border_color(border_default())
            .bg(bg_overlay())
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg_emphasis())
            .child(self.text.clone())
    }
}

fn render_waypoint_pill(label: &str, active: bool) -> impl IntoElement {
    div()
        .px(px(9.0))
        .py(px(4.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(if active { warning() } else { waypoint_border() })
        .bg(if active {
            waypoint_active_bg()
        } else {
            waypoint_bg()
        })
        .shadow_sm()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(div().w(px(8.0)).h(px(8.0)).rounded(px(999.0)).bg(warning()))
                .child(
                    div()
                        .max_w(px(220.0))
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(waypoint_fg())
                        .child(label.to_string()),
                ),
        )
}

fn render_review_line_action_overlay(
    state: &Entity<AppState>,
    target: &ReviewLineActionTarget,
    position: Point<Pixels>,
    mode: ReviewLineActionMode,
    cx: &App,
) -> impl IntoElement {
    let has_waypoint = state
        .read(cx)
        .active_review_session()
        .and_then(|session| session.waymark_for_location(&target.review_location()))
        .is_some();

    anchored()
        .position(position)
        .anchor(Corner::TopLeft)
        .offset(point(px(12.0), px(10.0)))
        .snap_to_window_with_margin(px(12.0))
        .child(render_review_line_action_popup(
            state,
            Some(target),
            mode,
            has_waypoint,
            cx,
        ))
}

fn render_review_line_action_popup(
    state: &Entity<AppState>,
    target: Option<&ReviewLineActionTarget>,
    mode: ReviewLineActionMode,
    has_waypoint: bool,
    cx: &App,
) -> impl IntoElement {
    let inline_comment_draft = state.read(cx).inline_comment_draft.clone();
    let inline_comment_loading = state.read(cx).inline_comment_loading;
    let inline_comment_error = state.read(cx).inline_comment_error.clone();
    let popup_key = target
        .map(|target| target.stable_key())
        .unwrap_or_else(|| "line-action-popup".to_string());
    let popup_animation_key = popup_key.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(33).wrapping_add(byte as usize)
    });

    div()
        .min_w(px(248.0))
        .max_w(px(320.0))
        .rounded(radius())
        .border_1()
        .border_color(border_default())
        .bg(bg_overlay())
        // Prevent diff rows behind the popup from receiving mouse interactions.
        .occlude()
        .shadow_sm()
        .on_any_mouse_down(|_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .border_b(px(1.0))
                .border_color(border_muted())
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(waypoint_fg())
                        .child(
                            target
                                .map(|target| target.label.to_uppercase())
                                .unwrap_or_else(|| "LINE ACTION".to_string()),
                        ),
                ),
        )
        .child(match mode {
            ReviewLineActionMode::Menu => div()
                .p(px(10.0))
                .flex()
                .gap(px(8.0))
                .child(line_action_button("Comment", false, {
                    let state = state.clone();
                    move |_, _, cx| {
                        state.update(cx, |state, cx| {
                            state.review_line_action_mode = ReviewLineActionMode::Comment;
                            state.inline_comment_error = None;
                            cx.notify();
                        });
                    }
                }))
                .child(line_action_button("Add waypoint", has_waypoint, {
                    let state = state.clone();
                    move |_, _, cx| {
                        let default_name = {
                            let app_state = state.read(cx);
                            default_waymark_name(
                                app_state.selected_file_path.as_deref(),
                                None,
                                app_state.selected_diff_anchor.as_ref(),
                            )
                        };
                        state.update(cx, |state, cx| {
                            state.add_waymark_for_current_review_location(default_name.clone());
                            state.persist_active_review_session();
                            cx.notify();
                        });
                    }
                }))
                .into_any_element(),
            ReviewLineActionMode::Comment => div()
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(9.0))
                        .rounded(radius_sm())
                        .border_1()
                        .border_color(border_default())
                        .bg(bg_surface())
                        .text_color(if inline_comment_draft.is_empty() {
                            fg_subtle()
                        } else {
                            fg_emphasis()
                        })
                        .child(
                            AppTextInput::new(
                                format!(
                                    "inline-comment-{}",
                                    target.map(|target| target.stable_key()).unwrap_or_default()
                                ),
                                state.clone(),
                                AppTextFieldKind::InlineCommentDraft,
                                "Comment on this line…",
                            )
                            .autofocus(true),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family(mono_font_family())
                                .text_color(fg_subtle())
                                .child("cmd-enter submit • esc close"),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .child(ghost_button("Back", {
                                    let state = state.clone();
                                    move |_, _, cx| {
                                        state.update(cx, |state, cx| {
                                            state.review_line_action_mode =
                                                ReviewLineActionMode::Menu;
                                            state.inline_comment_error = None;
                                            cx.notify();
                                        });
                                    }
                                }))
                                .child(review_button(
                                    if inline_comment_loading {
                                        "Submitting..."
                                    } else {
                                        "Submit"
                                    },
                                    {
                                        let state = state.clone();
                                        move |_, window, cx| {
                                            trigger_submit_inline_comment(&state, window, cx);
                                        }
                                    },
                                )),
                        ),
                )
                .when_some(inline_comment_error, |el, error| {
                    el.child(error_text(&error))
                })
                .into_any_element(),
        })
        .with_animation(
            ("review-line-action-popup", popup_animation_key),
            Animation::new(Duration::from_millis(140)).with_easing(ease_in_out),
            move |el, delta| {
                el.mt(lerp_px(8.0, 0.0, delta))
                    .opacity(delta.clamp(0.0, 1.0))
                    .border_color(lerp_rgba(transparent(), border_default(), delta))
                    .bg(lerp_rgba(bg_surface(), bg_overlay(), delta))
            },
        )
}

fn line_action_button(
    label: &str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "line-action-button-{label}-{}",
        usize::from(active)
    ));

    div()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(if active { warning() } else { border_default() })
        .bg(if active { waypoint_bg() } else { bg_surface() })
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { waypoint_fg() } else { fg_emphasis() })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(label.to_string())
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(bg_surface(), waypoint_bg(), progress))
                    .border_color(mix_rgba(border_default(), warning(), progress))
                    .text_color(mix_rgba(fg_emphasis(), waypoint_fg(), progress))
            },
        )
}

fn lerp_px(from: f32, to: f32, progress: f32) -> Pixels {
    px(from + (to - from) * progress)
}

fn lerp_rgba(from: Rgba, to: Rgba, progress: f32) -> Rgba {
    Rgba {
        r: from.r + (to.r - from.r) * progress,
        g: from.g + (to.g - from.g) * progress,
        b: from.b + (to.b - from.b) * progress,
        a: from.a + (to.a - from.a) * progress,
    }
}

fn render_hunk(
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    hunk: &ParsedDiffHunk,
    line_threads: &[&PullRequestReviewThread],
    selected_anchor: Option<&DiffAnchor>,
    unread_comment_ids: &BTreeSet<String>,
    state: &Entity<AppState>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(render_hunk_header(hunk, selected_anchor))
        .child(
            div()
                .flex()
                .flex_col()
                .children(hunk.lines.iter().map(|line| {
                    let threads_for_line = find_threads_for_line(file_path, line, line_threads);
                    render_diff_line_with_threads(
                        gutter_layout,
                        file_path,
                        line,
                        &threads_for_line,
                        selected_anchor,
                        unread_comment_ids,
                        state,
                    )
                })),
        )
}

fn render_diff_line_with_threads(
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    line: &ParsedDiffLine,
    threads: &[&PullRequestReviewThread],
    selected_anchor: Option<&DiffAnchor>,
    unread_comment_ids: &BTreeSet<String>,
    state: &Entity<AppState>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(render_diff_line(
            gutter_layout,
            file_path,
            line,
            None,
            None,
            selected_anchor,
            None,
            None,
            false,
            false,
        ))
        .when(!threads.is_empty(), |el| {
            el.child(
                div()
                    .pl(px(gutter_layout.inline_thread_inset()))
                    .pr(px(16.0))
                    .py(px(8.0))
                    .border_b(px(1.0))
                    .border_color(diff_annotation_border())
                    .bg(diff_annotation_bg())
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(threads.iter().map(|thread| {
                        render_review_thread(thread, selected_anchor, unread_comment_ids, state)
                    })),
            )
        })
}

fn render_diff_line(
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    line: &ParsedDiffLine,
    syntax_spans: Option<&[SyntaxSpan]>,
    emphasis_ranges: Option<&[DiffInlineRange]>,
    selected_anchor: Option<&DiffAnchor>,
    lsp_context: Option<&DiffLineLspContext>,
    line_action: Option<(Entity<AppState>, ReviewLineActionTarget)>,
    has_waypoint: bool,
    force_marker_visible: bool,
) -> impl IntoElement {
    let is_selected = line_matches_diff_anchor(line, selected_anchor);
    let row_action = line_action.clone();

    let left_num = line
        .left_line_number
        .map(|n| n.to_string())
        .unwrap_or_default();
    let right_num = line
        .right_line_number
        .map(|n| n.to_string())
        .unwrap_or_default();

    let marker = if line.prefix.is_empty() {
        " ".to_string()
    } else {
        line.prefix.clone()
    };

    let (row_bg, gutter_bg, marker_color, fallback_text_color) = match line.kind {
        DiffLineKind::Addition => (diff_add_bg(), diff_add_gutter_bg(), success(), fg_default()),
        DiffLineKind::Deletion => (
            diff_remove_bg(),
            diff_remove_gutter_bg(),
            danger(),
            fg_default(),
        ),
        DiffLineKind::Meta => (
            diff_meta_bg(),
            diff_context_gutter_bg(),
            fg_subtle(),
            fg_muted(),
        ),
        DiffLineKind::Context => (
            diff_context_bg(),
            diff_context_gutter_bg(),
            fg_subtle(),
            fg_default(),
        ),
    };
    let row_border = if is_selected {
        diff_selected_edge()
    } else {
        transparent()
    };
    let marker_visible = is_selected || force_marker_visible;
    let number_color = if is_selected {
        fg_default()
    } else {
        fg_subtle()
    };

    div()
        .flex()
        .items_start()
        .w_full()
        .min_w_0()
        .min_h(px(DIFF_ROW_HEIGHT))
        .bg(row_bg)
        .border_b(px(1.0))
        .border_color(row_border)
        .font_family(mono_font_family())
        .text_size(px(12.0))
        .text_color(if marker_visible {
            marker_color
        } else {
            transparent()
        })
        .hover(move |style| style.bg(diff_line_hover_bg()).text_color(marker_color))
        .when(is_selected, |el| {
            el.border_l(px(2.0)).border_color(diff_selected_edge())
        })
        .when_some(row_action, |el, (state, target)| {
            el.cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                    open_review_line_action(&state, target.clone(), event.position, cx);
                })
        })
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .w(px(gutter_layout.gutter_width()))
                .bg(gutter_bg)
                .border_r(px(1.0))
                .border_color(diff_gutter_separator())
                .when(gutter_layout.reserve_waypoint_slot, |el| {
                    el.child(
                        div()
                            .w(px(DIFF_WAYPOINT_SLOT_WIDTH))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(has_waypoint, |slot| {
                                slot.child(
                                    div()
                                        .id((
                                            ElementId::named_usize(
                                                "diff-waypoint",
                                                line.right_line_number
                                                    .or(line.left_line_number)
                                                    .unwrap_or_default()
                                                    as usize,
                                            ),
                                            SharedString::from(file_path.to_string()),
                                        ))
                                        .tooltip(|_, cx| build_static_tooltip("waypoint", cx))
                                        .child(render_diff_waypoint_icon()),
                                )
                            }),
                    )
                })
                .when(gutter_layout.show_left_numbers, |el| {
                    el.child(
                        div()
                            .w(px(DIFF_LINE_NUMBER_COLUMN_WIDTH))
                            .px(px(DIFF_LINE_NUMBER_CELL_PADDING_X))
                            .flex()
                            .justify_end()
                            .text_size(px(11.0))
                            .text_color(number_color)
                            .child(left_num),
                    )
                })
                .when(gutter_layout.show_right_numbers, |el| {
                    el.child(
                        div()
                            .w(px(DIFF_LINE_NUMBER_COLUMN_WIDTH))
                            .px(px(DIFF_LINE_NUMBER_CELL_PADDING_X))
                            .flex()
                            .justify_end()
                            .text_size(px(11.0))
                            .text_color(number_color)
                            .child(right_num),
                    )
                }),
        )
        .child(
            div()
                .w(px(DIFF_MARKER_COLUMN_WIDTH))
                .flex_shrink_0()
                .py(px(1.0))
                .child(marker),
        )
        .child(render_syntax_content(
            file_path,
            line,
            syntax_spans,
            emphasis_ranges,
            fallback_text_color,
            lsp_context,
            line_action,
        ))
}

fn render_syntax_content(
    file_path: &str,
    line: &ParsedDiffLine,
    syntax_spans: Option<&[SyntaxSpan]>,
    emphasis_ranges: Option<&[DiffInlineRange]>,
    fallback_color: Rgba,
    lsp_context: Option<&DiffLineLspContext>,
    line_action: Option<(Entity<AppState>, ReviewLineActionTarget)>,
) -> Div {
    let content = line.content.as_str();
    let content_div = div()
        .flex_grow()
        .min_w_0()
        .px(px(8.0))
        .py(px(1.0))
        .whitespace_nowrap()
        .text_size(px(12.0))
        .font_family(mono_font_family());

    if content.is_empty() {
        return content_div
            .text_color(fallback_color)
            .child("\u{00a0}".to_string());
    }

    let owned_spans;
    let spans = if let Some(spans) = syntax_spans {
        spans
    } else {
        owned_spans = syntax::highlight_line(file_path, content);
        owned_spans.as_slice()
    };

    let rendered_runs = decorated_diff_text_runs(
        content,
        spans,
        emphasis_ranges.unwrap_or(&[]),
        line.kind.clone(),
        fallback_color,
    )
    .or_else(|| code_text_runs(spans));

    if spans.is_empty() && rendered_runs.is_none() {
        let mut selectable = SelectableText::new(
            format!(
                "diff-line:{}:{}:{}",
                file_path,
                line.left_line_number.unwrap_or_default(),
                line.right_line_number.unwrap_or_default()
            ),
            content.to_string(),
        );
        if let Some((state, target)) = line_action {
            selectable = selectable.on_click_unmatched(move |window, cx| {
                open_review_line_action(&state, target.clone(), window.mouse_position(), cx);
            });
        }
        return content_div.text_color(fallback_color).child(selectable);
    }

    let selection_id = format!(
        "diff-line:{}:{}:{}",
        file_path,
        line.left_line_number.unwrap_or_default(),
        line.right_line_number.unwrap_or_default()
    );
    let token_ranges = Arc::new(build_interactive_code_tokens(content));

    if let Some(lsp_context) = lsp_context.filter(|_| !token_ranges.is_empty()) {
        let hover_context = lsp_context.clone();
        let hover_tokens = token_ranges.clone();
        let tooltip_context = lsp_context.clone();
        let tooltip_tokens = token_ranges.clone();
        let click_context = lsp_context.clone();
        let click_tokens = token_ranges.clone();
        let unmatched_click = line_action.clone();
        let click_ranges: Vec<std::ops::Range<usize>> =
            token_ranges.iter().map(|t| t.byte_range.clone()).collect();
        let interactive = if let Some(runs) = rendered_runs.clone() {
            SelectableText::new(
                format!(
                    "diff-lsp:{}:{}:{}",
                    lsp_context.file.file_path,
                    lsp_context.line_number,
                    line.right_line_number.unwrap_or_default()
                ),
                content.to_string(),
            )
            .with_runs(runs)
        } else {
            SelectableText::new(selection_id.clone(), content.to_string())
        }
        .on_click(click_ranges, move |range_ix, window, cx| {
            let token = &click_tokens[range_ix];
            let Some(query) =
                click_context.query_for_index(token.byte_range.start, click_tokens.as_ref())
            else {
                return;
            };
            navigate_to_diff_lsp_definition(query, window, cx);
        })
        .on_hover(move |index, _event, window, cx| {
            let Some(index) = index else {
                return;
            };
            let Some(query) = hover_context.query_for_index(index, hover_tokens.as_ref()) else {
                return;
            };
            request_diff_line_lsp_details(query, window, cx);
        })
        .tooltip(move |index, _window, cx| {
            let query = tooltip_context.query_for_index(index, tooltip_tokens.as_ref())?;
            Some(build_lsp_hover_tooltip_view(
                query.state.clone(),
                query.detail_key.clone(),
                query.query_key.clone(),
                query.token_label.clone(),
                cx,
            ))
        });

        let interactive = if let Some((state, target)) = unmatched_click {
            interactive.on_click_unmatched(move |window, cx| {
                open_review_line_action(&state, target.clone(), window.mouse_position(), cx);
            })
        } else {
            interactive
        };

        return content_div.text_color(fallback_color).child(interactive);
    }

    let mut selectable = if let Some(runs) = rendered_runs {
        SelectableText::new(selection_id, content.to_string()).with_runs(runs)
    } else {
        SelectableText::new(selection_id, content.to_string())
    };

    if let Some((state, target)) = line_action {
        selectable = selectable.on_click_unmatched(move |window, cx| {
            open_review_line_action(&state, target.clone(), window.mouse_position(), cx);
        });
    }

    content_div.text_color(fallback_color).child(selectable)
}

const DIFF_ROW_HEIGHT: f32 = 22.0;
const DIFF_LINE_NUMBER_COLUMN_WIDTH: f32 = 36.0;
const DIFF_LINE_NUMBER_CELL_PADDING_X: f32 = 6.0;
const DIFF_MARKER_COLUMN_WIDTH: f32 = 16.0;
const DIFF_WAYPOINT_SLOT_WIDTH: f32 = DIFF_ROW_HEIGHT;

#[derive(Clone, Copy)]
struct DiffGutterLayout {
    show_left_numbers: bool,
    show_right_numbers: bool,
    reserve_waypoint_slot: bool,
}

impl DiffGutterLayout {
    fn gutter_width(self) -> f32 {
        let column_count = self.show_left_numbers as u8 + self.show_right_numbers as u8;
        DIFF_LINE_NUMBER_COLUMN_WIDTH * f32::from(column_count.max(1))
            + if self.reserve_waypoint_slot {
                DIFF_WAYPOINT_SLOT_WIDTH
            } else {
                0.0
            }
    }

    fn inline_thread_inset(self) -> f32 {
        self.gutter_width() + 12.0
    }
}

fn diff_gutter_layout(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    reserve_waypoint_slot: bool,
) -> DiffGutterLayout {
    if let Some(parsed) = parsed {
        let show_left_numbers = parsed
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .any(|line| line.left_line_number.unwrap_or_default() > 0);
        let show_right_numbers = parsed
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .any(|line| line.right_line_number.unwrap_or_default() > 0);

        if show_left_numbers || show_right_numbers {
            return DiffGutterLayout {
                show_left_numbers,
                show_right_numbers,
                reserve_waypoint_slot,
            };
        }
    }

    match file.change_type.as_str() {
        "ADDED" => DiffGutterLayout {
            show_left_numbers: false,
            show_right_numbers: true,
            reserve_waypoint_slot,
        },
        "DELETED" => DiffGutterLayout {
            show_left_numbers: true,
            show_right_numbers: false,
            reserve_waypoint_slot,
        },
        _ => DiffGutterLayout {
            show_left_numbers: true,
            show_right_numbers: true,
            reserve_waypoint_slot,
        },
    }
}

fn diff_gutter_layout_from_parsed(parsed_file: &ParsedDiffFile) -> DiffGutterLayout {
    let show_left_numbers = parsed_file
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .any(|line| line.left_line_number.unwrap_or_default() > 0);
    let show_right_numbers = parsed_file
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .any(|line| line.right_line_number.unwrap_or_default() > 0);

    DiffGutterLayout {
        show_left_numbers: show_left_numbers || !show_right_numbers,
        show_right_numbers,
        reserve_waypoint_slot: false,
    }
}

const MAX_INLINE_DIFF_LINE_CHARS: usize = 512;
const MAX_INLINE_DIFF_TOKEN_CHARS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InlineTokenKind {
    Whitespace,
    Word,
    Punctuation,
}

#[derive(Clone, Debug)]
struct InlineToken {
    text: String,
    column_start: usize,
    column_end: usize,
    kind: InlineTokenKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InlineDiffOp {
    Equal(usize, usize),
    Delete(usize),
    Add(usize),
}

fn classify_inline_diff_char(ch: char) -> InlineTokenKind {
    if ch.is_whitespace() {
        InlineTokenKind::Whitespace
    } else if ch == '_' || ch.is_alphanumeric() {
        InlineTokenKind::Word
    } else {
        InlineTokenKind::Punctuation
    }
}

fn tokenize_inline_diff_line(content: &str) -> Vec<InlineToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_kind = None;
    let mut token_start = 1usize;
    let mut next_column = 1usize;

    for ch in content.chars() {
        let kind = classify_inline_diff_char(ch);
        if current_kind != Some(kind) && !current.is_empty() {
            tokens.push(InlineToken {
                text: std::mem::take(&mut current),
                column_start: token_start,
                column_end: next_column,
                kind: current_kind.expect("non-empty token should have a kind"),
            });
            token_start = next_column;
        }

        if current.is_empty() {
            token_start = next_column;
        }

        current_kind = Some(kind);
        current.push(ch);
        next_column += 1;
    }

    if !current.is_empty() {
        tokens.push(InlineToken {
            text: current,
            column_start: token_start,
            column_end: next_column,
            kind: current_kind.expect("non-empty token should have a kind"),
        });
    }

    tokens
}

fn diff_sequence_by<T, F>(left: &[T], right: &[T], eq: F) -> Vec<InlineDiffOp>
where
    F: Fn(&T, &T) -> bool + Copy,
{
    let mut lcs = vec![vec![0usize; right.len() + 1]; left.len() + 1];

    for left_ix in 0..left.len() {
        for right_ix in 0..right.len() {
            lcs[left_ix + 1][right_ix + 1] = if eq(&left[left_ix], &right[right_ix]) {
                lcs[left_ix][right_ix] + 1
            } else {
                lcs[left_ix + 1][right_ix].max(lcs[left_ix][right_ix + 1])
            };
        }
    }

    let mut left_ix = left.len();
    let mut right_ix = right.len();
    let mut ops = Vec::new();

    while left_ix > 0 || right_ix > 0 {
        if left_ix > 0 && right_ix > 0 && eq(&left[left_ix - 1], &right[right_ix - 1]) {
            ops.push(InlineDiffOp::Equal(left_ix - 1, right_ix - 1));
            left_ix -= 1;
            right_ix -= 1;
        } else if right_ix > 0
            && (left_ix == 0 || lcs[left_ix][right_ix - 1] >= lcs[left_ix - 1][right_ix])
        {
            ops.push(InlineDiffOp::Add(right_ix - 1));
            right_ix -= 1;
        } else {
            ops.push(InlineDiffOp::Delete(left_ix - 1));
            left_ix -= 1;
        }
    }

    ops.reverse();
    ops
}

fn token_range(token: &InlineToken) -> DiffInlineRange {
    DiffInlineRange {
        column_start: token.column_start,
        column_end: token.column_end,
    }
}

fn diff_single_token_chars(
    left: &InlineToken,
    right: &InlineToken,
) -> (Vec<DiffInlineRange>, Vec<DiffInlineRange>) {
    if left.text == right.text
        || left.text.chars().count() > MAX_INLINE_DIFF_TOKEN_CHARS
        || right.text.chars().count() > MAX_INLINE_DIFF_TOKEN_CHARS
    {
        return (vec![token_range(left)], vec![token_range(right)]);
    }

    let left_chars = left.text.chars().collect::<Vec<_>>();
    let right_chars = right.text.chars().collect::<Vec<_>>();
    let ops = diff_sequence_by(&left_chars, &right_chars, |left, right| left == right);

    let mut left_ranges = Vec::new();
    let mut right_ranges = Vec::new();

    for op in ops {
        match op {
            InlineDiffOp::Equal(_, _) => {}
            InlineDiffOp::Delete(left_ix) => left_ranges.push(DiffInlineRange {
                column_start: left.column_start + left_ix,
                column_end: left.column_start + left_ix + 1,
            }),
            InlineDiffOp::Add(right_ix) => right_ranges.push(DiffInlineRange {
                column_start: right.column_start + right_ix,
                column_end: right.column_start + right_ix + 1,
            }),
        }
    }

    if left_ranges.is_empty() || right_ranges.is_empty() {
        return (vec![token_range(left)], vec![token_range(right)]);
    }

    (
        merge_inline_ranges(left_ranges),
        merge_inline_ranges(right_ranges),
    )
}

fn apply_inline_diff_group(
    left_tokens: &[InlineToken],
    right_tokens: &[InlineToken],
    deleted_indices: &[usize],
    added_indices: &[usize],
    left_ranges: &mut Vec<DiffInlineRange>,
    right_ranges: &mut Vec<DiffInlineRange>,
) {
    let deleted = deleted_indices
        .iter()
        .filter_map(|ix| left_tokens.get(*ix))
        .filter(|token| token.kind != InlineTokenKind::Whitespace)
        .collect::<Vec<_>>();
    let added = added_indices
        .iter()
        .filter_map(|ix| right_tokens.get(*ix))
        .filter(|token| token.kind != InlineTokenKind::Whitespace)
        .collect::<Vec<_>>();

    if deleted.is_empty() && added.is_empty() {
        return;
    }

    if deleted.len() == 1 && added.len() == 1 {
        let (deleted_chars, added_chars) = diff_single_token_chars(deleted[0], added[0]);
        left_ranges.extend(deleted_chars);
        right_ranges.extend(added_chars);
        return;
    }

    left_ranges.extend(deleted.into_iter().map(token_range));
    right_ranges.extend(added.into_iter().map(token_range));
}

fn merge_inline_ranges(mut ranges: Vec<DiffInlineRange>) -> Vec<DiffInlineRange> {
    if ranges.len() <= 1 {
        return ranges;
    }

    ranges.sort_by_key(|range| (range.column_start, range.column_end));
    let mut merged: Vec<DiffInlineRange> = Vec::with_capacity(ranges.len());

    for range in ranges {
        match merged.last_mut() {
            Some(previous) if previous.column_end >= range.column_start => {
                previous.column_end = previous.column_end.max(range.column_end);
            }
            _ => merged.push(range),
        }
    }

    merged
}

fn compute_inline_emphasis(
    left: &str,
    right: &str,
) -> (Vec<DiffInlineRange>, Vec<DiffInlineRange>) {
    if left == right
        || left.chars().count() > MAX_INLINE_DIFF_LINE_CHARS
        || right.chars().count() > MAX_INLINE_DIFF_LINE_CHARS
    {
        return (Vec::new(), Vec::new());
    }

    let left_tokens = tokenize_inline_diff_line(left);
    let right_tokens = tokenize_inline_diff_line(right);
    let ops = diff_sequence_by(&left_tokens, &right_tokens, |left, right| {
        left.kind == right.kind && left.text == right.text
    });

    let mut left_ranges = Vec::new();
    let mut right_ranges = Vec::new();
    let mut deleted_indices = Vec::new();
    let mut added_indices = Vec::new();

    for op in ops {
        match op {
            InlineDiffOp::Equal(_, _) => {
                apply_inline_diff_group(
                    &left_tokens,
                    &right_tokens,
                    &deleted_indices,
                    &added_indices,
                    &mut left_ranges,
                    &mut right_ranges,
                );
                deleted_indices.clear();
                added_indices.clear();
            }
            InlineDiffOp::Delete(left_ix) => deleted_indices.push(left_ix),
            InlineDiffOp::Add(right_ix) => added_indices.push(right_ix),
        }
    }

    apply_inline_diff_group(
        &left_tokens,
        &right_tokens,
        &deleted_indices,
        &added_indices,
        &mut left_ranges,
        &mut right_ranges,
    );

    (
        merge_inline_ranges(left_ranges),
        merge_inline_ranges(right_ranges),
    )
}

fn build_hunk_inline_emphasis(hunk: &ParsedDiffHunk) -> Vec<Vec<DiffInlineRange>> {
    let mut emphasis = vec![Vec::new(); hunk.lines.len()];
    let mut line_ix = 0usize;

    while line_ix < hunk.lines.len() {
        if !matches!(
            hunk.lines[line_ix].kind,
            DiffLineKind::Addition | DiffLineKind::Deletion
        ) {
            line_ix += 1;
            continue;
        }

        let mut deletions = Vec::new();
        let mut additions = Vec::new();
        while line_ix < hunk.lines.len()
            && matches!(
                hunk.lines[line_ix].kind,
                DiffLineKind::Addition | DiffLineKind::Deletion
            )
        {
            match hunk.lines[line_ix].kind {
                DiffLineKind::Deletion => deletions.push(line_ix),
                DiffLineKind::Addition => additions.push(line_ix),
                _ => {}
            }
            line_ix += 1;
        }

        for (deleted_ix, added_ix) in deletions.into_iter().zip(additions.into_iter()) {
            let (deleted_ranges, added_ranges) = compute_inline_emphasis(
                hunk.lines[deleted_ix].content.as_str(),
                hunk.lines[added_ix].content.as_str(),
            );
            emphasis[deleted_ix].extend(deleted_ranges);
            emphasis[added_ix].extend(added_ranges);
        }
    }

    emphasis
        .into_iter()
        .map(merge_inline_ranges)
        .collect::<Vec<_>>()
}

fn inline_emphasis_background(kind: DiffLineKind) -> Option<Hsla> {
    match kind {
        DiffLineKind::Addition => Some(diff_add_emphasis_bg().into()),
        DiffLineKind::Deletion => Some(diff_remove_emphasis_bg().into()),
        DiffLineKind::Context | DiffLineKind::Meta => None,
    }
}

fn decorated_diff_text_runs(
    content: &str,
    spans: &[SyntaxSpan],
    emphasis_ranges: &[DiffInlineRange],
    kind: DiffLineKind,
    fallback_color: Rgba,
) -> Option<Vec<TextRun>> {
    if emphasis_ranges.is_empty() {
        return None;
    }

    let emphasis_background = inline_emphasis_background(kind)?;
    let chars = content.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return None;
    }

    let mut colors = vec![Hsla::from(fallback_color); chars.len()];
    for span in spans {
        let start = span.column_start.saturating_sub(1).min(chars.len());
        let end = span.column_end.saturating_sub(1).min(chars.len());
        for color in colors.iter_mut().take(end).skip(start) {
            *color = span.color;
        }
    }

    let mut emphasized = vec![false; chars.len()];
    for range in emphasis_ranges {
        let start = range.column_start.saturating_sub(1).min(chars.len());
        let end = range.column_end.saturating_sub(1).min(chars.len());
        for flag in emphasized.iter_mut().take(end).skip(start) {
            *flag = true;
        }
    }

    let mut runs = Vec::new();
    let mut segment = String::new();
    let mut current_color = colors[0];
    let mut current_emphasis = emphasized[0];

    for (index, ch) in chars.into_iter().enumerate() {
        if index > 0 && (colors[index] != current_color || emphasized[index] != current_emphasis) {
            runs.push(TextRun {
                len: segment.len(),
                font: font(mono_font_family()),
                color: current_color,
                background_color: current_emphasis.then_some(emphasis_background),
                underline: None,
                strikethrough: None,
            });
            segment.clear();
            current_color = colors[index];
            current_emphasis = emphasized[index];
        }

        segment.push(ch);
    }

    if !segment.is_empty() {
        runs.push(TextRun {
            len: segment.len(),
            font: font(mono_font_family()),
            color: current_color,
            background_color: current_emphasis.then_some(emphasis_background),
            underline: None,
            strikethrough: None,
        });
    }

    (!runs.is_empty()).then_some(runs)
}

fn build_diff_highlights(parsed_file: &ParsedDiffFile) -> Arc<Vec<Vec<DiffLineHighlight>>> {
    Arc::new(
        parsed_file
            .hunks
            .iter()
            .map(|hunk| {
                let syntax_lines = syntax::highlight_lines(
                    parsed_file.path.as_str(),
                    hunk.lines.iter().map(|line| line.content.as_str()),
                );
                let emphasis_lines = build_hunk_inline_emphasis(hunk);

                hunk.lines
                    .iter()
                    .enumerate()
                    .map(|(line_ix, _)| DiffLineHighlight {
                        syntax_spans: syntax_lines.get(line_ix).cloned().unwrap_or_default(),
                        emphasis_ranges: emphasis_lines.get(line_ix).cloned().unwrap_or_default(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

fn render_review_thread(
    thread: &PullRequestReviewThread,
    selected_anchor: Option<&DiffAnchor>,
    unread_comment_ids: &BTreeSet<String>,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let is_selected = thread_matches_diff_anchor(thread, selected_anchor);
    let thread_unread_comment_ids = thread
        .comments
        .iter()
        .filter(|comment| unread_comment_ids.contains(&comment.id))
        .map(|comment| comment.id.clone())
        .collect::<Vec<_>>();
    let unread_count = thread_unread_comment_ids.len();
    let state_for_mark_read = state.clone();
    let thread_border = transparent();
    let header_bg = if is_selected {
        diff_line_hover_bg()
    } else if thread.is_resolved {
        success_muted()
    } else {
        diff_annotation_bg()
    };

    div()
        .rounded(radius_sm())
        .border_1()
        .border_color(thread_border)
        .bg(diff_editor_chrome())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .border_b(px(1.0))
                .border_color(diff_annotation_border())
                .bg(header_bg)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(badge(&thread.subject_type.to_lowercase()))
                        .when(thread.is_resolved, |el| el.child(badge_success("resolved")))
                        .when(thread.is_outdated, |el| el.child(badge("outdated"))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .when(unread_count > 0, |el| {
                            el.child(badge(&format!("{unread_count} new")))
                                .child(ghost_button("Mark read", move |_, _, cx| {
                                    state_for_mark_read.update(cx, |state, cx| {
                                        state.mark_review_comments_read(
                                            thread_unread_comment_ids.clone(),
                                        );
                                        cx.notify();
                                    });
                                }))
                        }),
                ),
        )
        .child(div().p(px(12.0)).flex().flex_col().gap(px(8.0)).children(
            thread.comments.iter().map(|comment| {
                render_thread_comment(comment, unread_comment_ids.contains(&comment.id))
            }),
        ))
}

fn render_thread_comment(comment: &PullRequestReviewComment, is_unread: bool) -> impl IntoElement {
    div()
        .p(px(12.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if is_unread {
            diff_selected_edge()
        } else {
            diff_annotation_border()
        })
        .bg(if is_unread {
            diff_line_hover_bg()
        } else {
            diff_editor_surface()
        })
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .child(user_avatar(
                    &comment.author_login,
                    comment.author_avatar_url.as_deref(),
                    20.0,
                    false,
                ))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(comment.author_login.clone()),
                )
                .child(
                    div().text_color(fg_subtle()).child(
                        comment
                            .published_at
                            .as_deref()
                            .unwrap_or(&comment.created_at)
                            .to_string(),
                    ),
                )
                .when(is_unread, |el| el.child(badge("new"))),
        )
        .child(if comment.body.is_empty() {
            div()
                .text_size(px(13.0))
                .text_color(fg_muted())
                .child("No comment body.")
                .into_any_element()
        } else {
            render_markdown(&format!("thread-comment-{}", comment.id), &comment.body)
                .into_any_element()
        })
}

fn render_hunk_header(
    hunk: &ParsedDiffHunk,
    selected_anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    let hunk_is_selected = selected_anchor
        .and_then(|anchor| anchor.hunk_header.as_deref())
        .map(|header| header == hunk.header)
        .unwrap_or(false)
        && selected_anchor.and_then(|anchor| anchor.line).is_none();

    div()
        .px(px(14.0))
        .py(px(5.0))
        .border_b(px(1.0))
        .border_color(if hunk_is_selected {
            diff_selected_edge()
        } else {
            diff_annotation_border()
        })
        .bg(if hunk_is_selected {
            diff_line_hover_bg()
        } else {
            diff_hunk_bg()
        })
        .text_size(px(11.0))
        .font_family(mono_font_family())
        .text_color(if hunk_is_selected {
            fg_emphasis()
        } else {
            diff_hunk_fg()
        })
        .child(hunk.header.clone())
}

// Helpers

pub fn render_tour_diff_file(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    preview_key: &str,
    file_path: Option<&str>,
    snippet: Option<&str>,
    anchor: Option<&DiffAnchor>,
    cx: &App,
) -> AnyElement {
    render_tour_diff_file_with_options(
        state,
        detail,
        preview_key,
        file_path,
        snippet,
        anchor,
        true,
        cx,
    )
}

fn render_tour_diff_file_compact(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    preview_key: &str,
    file_path: Option<&str>,
    snippet: Option<&str>,
    anchor: Option<&DiffAnchor>,
    cx: &App,
) -> AnyElement {
    render_tour_diff_file_with_options(
        state,
        detail,
        preview_key,
        file_path,
        snippet,
        anchor,
        false,
        cx,
    )
}

fn render_tour_diff_file_with_options(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    preview_key: &str,
    file_path: Option<&str>,
    snippet: Option<&str>,
    anchor: Option<&DiffAnchor>,
    show_header: bool,
    cx: &App,
) -> AnyElement {
    let Some(file_path) = file_path else {
        return div().into_any_element();
    };

    let file = detail
        .files
        .iter()
        .find(|candidate| candidate.path == file_path);
    let parsed_file = find_parsed_diff_file(&detail.parsed_diff, file_path);

    if let Some(parsed_file) = parsed_file {
        let prepared_file = state
            .read(cx)
            .active_detail_state()
            .and_then(|detail_state| detail_state.file_content_states.get(file_path))
            .and_then(|file_state| file_state.prepared.as_ref())
            .cloned();
        let diff_view_state = {
            let app_state = state.read(cx);
            file.map(|file| {
                prepare_tour_diff_view_state(&app_state, detail, preview_key, &file.path)
            })
        };
        let file_lsp_context = show_header
            .then(|| {
                build_diff_file_lsp_context(
                    state,
                    parsed_file.path.as_str(),
                    prepared_file.as_ref(),
                    cx,
                )
            })
            .flatten();

        let diff_body = if parsed_file.hunks.is_empty() {
            panel_state_text("No textual hunks available for this file.").into_any_element()
        } else if let (Some(file), Some(diff_view_state)) = (file, diff_view_state) {
            render_tour_diff_preview(
                state,
                file,
                parsed_file,
                prepared_file.as_ref(),
                anchor,
                diff_view_state,
                file_lsp_context,
                cx,
            )
            .into_any_element()
        } else {
            render_full_tour_diff_preview(parsed_file, anchor, file_lsp_context.as_ref())
                .into_any_element()
        };

        if !show_header {
            return diff_body;
        }

        return nested_panel()
            .child(render_tour_diff_file_header(file, parsed_file))
            .child(diff_body)
            .into_any_element();
    }

    if let Some(snippet) = snippet {
        let snippet_body = div()
            .child(render_highlighted_code_block("diff.patch", snippet))
            .into_any_element();
        if !show_header {
            return snippet_body;
        }

        return nested_panel()
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_subtle())
                    .font_family(mono_font_family())
                    .mb(px(8.0))
                    .child("CHANGESET"),
            )
            .child(snippet_body)
            .into_any_element();
    }

    panel_state_text("No parsed diff is available for this file.").into_any_element()
}

fn render_tour_diff_file_header(
    file: Option<&PullRequestFile>,
    parsed_file: &ParsedDiffFile,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .mb(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_subtle())
                        .font_family(mono_font_family())
                        .child("CHANGESET"),
                )
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(
                            if parsed_file.previous_path.is_some()
                                && parsed_file.previous_path.as_deref() != Some(&parsed_file.path)
                            {
                                format!(
                                    "{} -> {}",
                                    parsed_file.previous_path.as_deref().unwrap_or(""),
                                    parsed_file.path
                                )
                            } else {
                                parsed_file.path.clone()
                            },
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .when_some(file, |el, file| {
                    el.child(render_change_type_chip(&file.change_type))
                        .child(badge(&format!("+{} / -{}", file.additions, file.deletions)))
                })
                .when(parsed_file.is_binary, |el| el.child(badge("binary"))),
        )
}

fn render_tour_diff_preview(
    state: &Entity<AppState>,
    file: &PullRequestFile,
    parsed_file: &ParsedDiffFile,
    prepared_file: Option<&PreparedFileContent>,
    selected_anchor: Option<&DiffAnchor>,
    diff_view_state: DiffFileViewState,
    file_lsp_context: Option<DiffFileLspContext>,
    cx: &App,
) -> impl IntoElement {
    let rows = diff_view_state.rows;
    let parsed_file_index = diff_view_state.parsed_file_index;
    let highlighted_hunks = diff_view_state.highlighted_hunks;
    let gutter_layout = diff_gutter_layout(file, Some(parsed_file), false);
    let preview_items = {
        let app_state = state.read(cx);
        build_tour_diff_preview_items(
            state,
            app_state.active_detail(),
            file,
            parsed_file,
            prepared_file,
            &rows,
            selected_anchor,
            cx,
        )
    };

    let elements: Vec<AnyElement> = preview_items
        .items
        .iter()
        .map(|item| match item {
            DiffViewItem::SemanticSection(_) => div().into_any_element(),
            DiffViewItem::Gap(gap) => render_diff_gap_row(*gap, gutter_layout).into_any_element(),
            DiffViewItem::StackLayerEmpty => div().into_any_element(),
            DiffViewItem::Row(row_ix) => render_virtualized_diff_row(
                state,
                gutter_layout,
                parsed_file_index,
                highlighted_hunks.as_deref(),
                file_lsp_context.as_ref(),
                &rows[*row_ix],
                selected_anchor,
                cx,
            )
            .into_any_element(),
        })
        .collect();

    div()
        .flex()
        .flex_col()
        .rounded(radius())
        .border_1()
        .border_color(diff_annotation_border())
        .bg(diff_editor_bg())
        .overflow_hidden()
        .when(preview_items.focused_excerpt, |el| {
            el.child(
                div()
                    .px(px(14.0))
                    .py(px(10.0))
                    .border_b(px(1.0))
                    .border_color(diff_annotation_border())
                    .bg(diff_annotation_bg())
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_wrap()
                    .child(badge("focused excerpt"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .child(
                                "Showing the diff slice relevant to this guide step. Open in Files for the full changeset.",
                            ),
                    ),
            )
        })
        .child(div().flex().flex_col().bg(diff_editor_bg()).children(elements))
}

fn render_full_tour_diff_preview(
    parsed_file: &ParsedDiffFile,
    anchor: Option<&DiffAnchor>,
    file_lsp_context: Option<&DiffFileLspContext>,
) -> impl IntoElement {
    let highlighted_hunks = build_diff_highlights(parsed_file);
    let gutter_layout = diff_gutter_layout_from_parsed(parsed_file);
    let mut elements: Vec<AnyElement> = Vec::new();
    let file_path = parsed_file.path.as_str();

    for hunk_idx in 0..parsed_file.hunks.len() {
        let hunk = &parsed_file.hunks[hunk_idx];
        elements.push(render_hunk_header(hunk, anchor).into_any_element());

        for (line_idx, line) in hunk.lines.iter().enumerate() {
            let highlight = highlighted_hunks
                .get(hunk_idx)
                .and_then(|lines| lines.get(line_idx))
                .cloned()
                .unwrap_or_default();
            let line_lsp_context = build_diff_line_lsp_context(file_lsp_context, line);
            elements.push(
                render_diff_line(
                    gutter_layout,
                    file_path,
                    line,
                    Some(highlight.syntax_spans.as_slice()),
                    Some(highlight.emphasis_ranges.as_slice()),
                    anchor,
                    line_lsp_context.as_ref(),
                    None,
                    false,
                    false,
                )
                .into_any_element(),
            );
        }
    }

    div().flex().flex_col().children(elements)
}

const TOUR_PREVIEW_MAX_ITEMS: usize = 96;
const TOUR_PREVIEW_CONTEXT_ITEMS: usize = 24;

struct TourDiffPreviewItems {
    items: Vec<DiffViewItem>,
    focused_excerpt: bool,
}

fn build_tour_diff_preview_items(
    state: &Entity<AppState>,
    detail: Option<&PullRequestDetail>,
    file: &PullRequestFile,
    parsed_file: &ParsedDiffFile,
    prepared_file: Option<&PreparedFileContent>,
    rows: &[DiffRenderRow],
    selected_anchor: Option<&DiffAnchor>,
    cx: &App,
) -> TourDiffPreviewItems {
    let full_items = build_diff_view_items(
        state,
        file,
        Some(parsed_file),
        None,
        prepared_file,
        rows,
        None,
        cx,
    );
    if full_items.len() <= TOUR_PREVIEW_MAX_ITEMS {
        return TourDiffPreviewItems {
            items: full_items,
            focused_excerpt: false,
        };
    }

    let focused_rows = selected_anchor
        .and_then(|anchor| find_tour_preview_focus_rows(detail, parsed_file, rows, anchor))
        .unwrap_or_else(|| (0..rows.len().min(TOUR_PREVIEW_MAX_ITEMS)).collect());

    let items = focused_rows
        .into_iter()
        .map(DiffViewItem::Row)
        .collect::<Vec<_>>();
    let focused_excerpt = items.len() < full_items.len();

    TourDiffPreviewItems {
        items,
        focused_excerpt,
    }
}

fn find_tour_preview_focus_rows(
    detail: Option<&PullRequestDetail>,
    parsed_file: &ParsedDiffFile,
    rows: &[DiffRenderRow],
    anchor: &DiffAnchor,
) -> Option<Vec<usize>> {
    if let Some(detail) = detail.filter(|_| anchor.thread_id.is_some()) {
        if let Some((row_ix, row)) = rows.iter().enumerate().find(|(_, row)| match row {
            DiffRenderRow::FileCommentThread { thread_index }
            | DiffRenderRow::InlineThread { thread_index }
            | DiffRenderRow::OutdatedThread { thread_index } => detail
                .review_threads
                .get(*thread_index)
                .map(|thread| thread_matches_diff_anchor(thread, Some(anchor)))
                .unwrap_or(false),
            _ => false,
        }) {
            return Some(match row {
                DiffRenderRow::InlineThread { .. } => preview_rows_for_hunk(rows, row_ix)
                    .unwrap_or_else(|| preview_rows_for_window(rows, row_ix)),
                DiffRenderRow::FileCommentThread { .. } => preview_rows_for_header_and_row(
                    rows,
                    row_ix,
                    matches!(row, DiffRenderRow::FileCommentThread { .. }),
                ),
                DiffRenderRow::OutdatedThread { .. } => {
                    preview_rows_for_header_and_row(rows, row_ix, false)
                }
                _ => preview_rows_for_window(rows, row_ix),
            });
        }
    }

    if let Some((row_ix, _)) = rows.iter().enumerate().find(|(_, row)| match row {
        DiffRenderRow::HunkHeader { hunk_index } => {
            anchor.line.is_none()
                && anchor
                    .hunk_header
                    .as_deref()
                    .map(|header| {
                        parsed_file
                            .hunks
                            .get(*hunk_index)
                            .map(|hunk| hunk.header == header)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
        }
        DiffRenderRow::Line {
            hunk_index,
            line_index,
        } => parsed_file
            .hunks
            .get(*hunk_index)
            .and_then(|hunk| hunk.lines.get(*line_index))
            .map(|line| line_matches_diff_anchor(line, Some(anchor)))
            .unwrap_or(false),
        _ => false,
    }) {
        return preview_rows_for_hunk(rows, row_ix)
            .or_else(|| Some(preview_rows_for_window(rows, row_ix)));
    }

    None
}

fn preview_rows_for_hunk(rows: &[DiffRenderRow], focus_row_ix: usize) -> Option<Vec<usize>> {
    let hunk_start = (0..=focus_row_ix)
        .rev()
        .find(|ix| matches!(rows[*ix], DiffRenderRow::HunkHeader { .. }))?;
    let hunk_end = rows
        .iter()
        .enumerate()
        .skip(focus_row_ix + 1)
        .find_map(|(ix, row)| {
            matches!(
                row,
                DiffRenderRow::HunkHeader { .. } | DiffRenderRow::OutdatedCommentsHeader { .. }
            )
            .then_some(ix.saturating_sub(1))
        })
        .unwrap_or_else(|| rows.len().saturating_sub(1));

    let hunk_len = hunk_end.saturating_sub(hunk_start).saturating_add(1);
    if hunk_len <= TOUR_PREVIEW_MAX_ITEMS {
        return Some((hunk_start..=hunk_end).collect());
    }

    let excerpt_start = focus_row_ix
        .saturating_sub(TOUR_PREVIEW_CONTEXT_ITEMS)
        .max(hunk_start.saturating_add(1));
    let excerpt_end = (focus_row_ix + TOUR_PREVIEW_CONTEXT_ITEMS).min(hunk_end);
    let mut rows_to_render = Vec::with_capacity(excerpt_end.saturating_sub(excerpt_start) + 2);
    rows_to_render.push(hunk_start);
    rows_to_render.extend(excerpt_start..=excerpt_end);
    Some(rows_to_render)
}

fn preview_rows_for_header_and_row(
    rows: &[DiffRenderRow],
    row_ix: usize,
    file_comment_thread: bool,
) -> Vec<usize> {
    let header = (0..row_ix).rev().find(|ix| {
        if file_comment_thread {
            matches!(rows[*ix], DiffRenderRow::FileCommentsHeader { .. })
        } else {
            matches!(rows[*ix], DiffRenderRow::OutdatedCommentsHeader { .. })
        }
    });

    let mut rows_to_render = Vec::with_capacity(2);
    if let Some(header) = header {
        rows_to_render.push(header);
    }
    rows_to_render.push(row_ix);
    rows_to_render
}

fn preview_rows_for_window(rows: &[DiffRenderRow], focus_row_ix: usize) -> Vec<usize> {
    let start = focus_row_ix.saturating_sub(TOUR_PREVIEW_CONTEXT_ITEMS);
    let end = (focus_row_ix + TOUR_PREVIEW_CONTEXT_ITEMS).min(rows.len().saturating_sub(1));
    (start..=end).collect()
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
