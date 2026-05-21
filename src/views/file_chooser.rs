use std::{sync::Arc, time::Duration};

use gpui::prelude::*;
use gpui::*;

use crate::{
    icons::{lucide_icon, LucideIcon},
    review_session::ReviewSourceTarget,
    selectable_text::{AppTextFieldKind, AppTextInput},
    shortcuts,
    state::{AppState, PullRequestSurface, ReviewFileTreeRow, SourceFileTreeState},
    theme::*,
};

use super::{
    diff_view::{ensure_active_review_focus_loaded, ensure_source_file_tree_loaded},
    motion::{lerp_px, lerp_rgba},
    palette::{close_palette, fuzzy_match_score, fuzzy_query_chars},
    sections::{badge, panel_state_text},
};

const FILE_CHOOSER_ROW_HEIGHT: f32 = 48.0;
const FILE_CHOOSER_RESULT_CONTEXT_ROWS: usize = 6;

pub fn open_file_chooser(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    if state.read(cx).palette_open {
        close_palette(state, cx);
    }

    let mut should_load_source_tree = false;
    state.update(cx, |state, cx| {
        if state.active_surface != PullRequestSurface::Files || state.active_detail().is_none() {
            return;
        }

        state.file_chooser_open = true;
        state.file_chooser_query.clear();
        state.file_chooser_selected_index = 0;
        state.file_chooser_list_state.reset(0);
        state.palette_open = false;
        state.palette_closing = false;
        state.palette_query.clear();
        state.palette_selected_index = 0;
        state.waypoint_spotlight_open = false;
        state.waypoint_spotlight_query.clear();
        state.waypoint_spotlight_selected_index = 0;
        state.active_review_line_action = None;
        state.active_review_line_action_position = None;
        state.review_line_action_mode = crate::state::ReviewLineActionMode::Menu;
        state.active_review_line_drag_origin = None;
        state.active_review_line_drag_current = None;
        state.inline_comment_error = None;
        should_load_source_tree = true;
        cx.notify();
    });

    if should_load_source_tree {
        ensure_source_file_tree_loaded(state, window, cx);
    }
}

pub fn toggle_file_chooser(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let is_open = state.read(cx).file_chooser_open;
    if is_open {
        close_file_chooser(state, cx);
    } else {
        open_file_chooser(state, window, cx);
    }
}

pub fn close_file_chooser(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |state, cx| {
        state.file_chooser_open = false;
        state.file_chooser_query.clear();
        state.file_chooser_selected_index = 0;
        state.file_chooser_list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
}

pub fn move_file_chooser_selection(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    state.update(cx, |state, cx| {
        if !state.file_chooser_open {
            return;
        }

        let item_count = filtered_file_chooser_items(state).len();
        if item_count == 0 {
            state.file_chooser_selected_index = 0;
            cx.notify();
            return;
        }

        let max_index = item_count.saturating_sub(1) as isize;
        let next =
            (state.file_chooser_selected_index as isize + delta).clamp(0, max_index) as usize;
        if next != state.file_chooser_selected_index {
            state.file_chooser_selected_index = next;
            state.file_chooser_list_state.scroll_to(ListOffset {
                item_ix: next.saturating_sub(FILE_CHOOSER_RESULT_CONTEXT_ROWS),
                offset_in_item: px(0.0),
            });
            cx.notify();
        }
    });
}

pub fn execute_file_chooser_selection(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let item = {
        let app_state = state.read(cx);
        let items = filtered_file_chooser_items(&app_state);
        let selected_index = app_state
            .file_chooser_selected_index
            .min(items.len().saturating_sub(1));
        items.get(selected_index).cloned()
    };

    let Some(item) = item else {
        return;
    };

    open_file_chooser_item(item, state, window, cx);
}

pub fn render_file_chooser(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let app_state = state.read(cx);
    let query = app_state.file_chooser_query.clone();
    let source_tree = app_state
        .active_detail_state()
        .map(|detail_state| detail_state.source_file_tree.clone())
        .unwrap_or_default();
    let rows_loaded = source_tree.rows.is_some();
    let filtered = filtered_file_chooser_items(&app_state);
    let filtered_count = filtered.len();
    let selected_index = app_state
        .file_chooser_selected_index
        .min(filtered_count.saturating_sub(1));
    let list_state = app_state.file_chooser_list_state.clone();
    let count_label = file_chooser_count_label(&source_tree, filtered_count, query.as_str());
    let status_message = file_chooser_status_message(&source_tree, rows_loaded, filtered_count);
    let state_for_backdrop = state.clone();

    if list_state.item_count() != filtered_count {
        list_state.reset(filtered_count);
    }

    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .justify_center()
        .pt(px(72.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, {
                    let state = state_for_backdrop.clone();
                    move |_, _, cx| {
                        close_file_chooser(&state, cx);
                    }
                }),
        )
        .child(
            div()
                .w(px(720.0))
                .max_h(px(640.0))
                .bg(bg_overlay())
                .rounded(radius_lg())
                .border_1()
                .border_color(transparent())
                .occlude()
                .shadow(dialog_shadow())
                .overflow_hidden()
                .flex()
                .flex_col()
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
                                        .child(lucide_icon(
                                            LucideIcon::FileCode2,
                                            18.0,
                                            fg_emphasis(),
                                        ))
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child("Open repository file"),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .child(badge(&shortcuts::secondary_key_label("p")))
                                        .child(badge("esc")),
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
                                        "file-chooser-query-input",
                                        state.clone(),
                                        AppTextFieldKind::FileChooserQuery,
                                        "Type a file name or path",
                                    )
                                    .autofocus(app_state.file_chooser_open),
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
                                        .child(count_label),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child("up/down move")
                                        .child("/")
                                        .child("enter open"),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("file-chooser-results")
                        .flex_grow()
                        .min_h_0()
                        .min_h(px(92.0))
                        .max_h(px(452.0))
                        .flex()
                        .flex_col()
                        .px(px(8.0))
                        .pb(px(8.0))
                        .child(if let Some(message) = status_message {
                            div()
                                .px(px(16.0))
                                .py(px(18.0))
                                .child(panel_state_text(&message))
                                .into_any_element()
                        } else {
                            render_file_chooser_results(
                                state.clone(),
                                Arc::new(filtered),
                                selected_index,
                                list_state,
                            )
                            .into_any_element()
                        }),
                )
                .with_animation(
                    "file-chooser",
                    Animation::new(Duration::from_millis(160)).with_easing(ease_in_out),
                    move |el, delta| {
                        el.mt(lerp_px(10.0, 0.0, delta)).bg(lerp_rgba(
                            bg_canvas(),
                            bg_overlay(),
                            delta,
                        ))
                    },
                ),
        )
}

fn render_file_chooser_results(
    state: Entity<AppState>,
    items: Arc<Vec<FileChooserItem>>,
    selected_index: usize,
    list_state: ListState,
) -> impl IntoElement {
    list(list_state, move |ix, _window, _cx| {
        let item = items[ix].clone();
        render_file_chooser_row(item, ix == selected_index, state.clone()).into_any_element()
    })
    .with_sizing_behavior(ListSizingBehavior::Auto)
    .flex_grow()
    .min_h_0()
}

fn render_file_chooser_row(
    item: FileChooserItem,
    selected: bool,
    state: Entity<AppState>,
) -> impl IntoElement {
    let name = item.name.clone();
    let directory = item.directory.clone();
    let additions = item.additions;
    let deletions = item.deletions;

    div()
        .w_full()
        .h(px(FILE_CHOOSER_ROW_HEIGHT))
        .mx(px(0.0))
        .mb(px(1.0))
        .px(px(16.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if selected {
            bg_emphasis()
        } else {
            bg_overlay()
        })
        .text_color(if selected {
            fg_emphasis()
        } else {
            fg_default()
        })
        .hover(move |style| {
            style.bg(if selected {
                bg_emphasis()
            } else {
                bg_selected()
            })
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_file_chooser_item(item.clone(), &state, window, cx);
        })
        .child(
            div()
                .h_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .min_w_0()
                        .child(lucide_icon(LucideIcon::FileCode2, 15.0, fg_subtle()))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .child(directory),
                                ),
                        ),
                )
                .when(additions != 0 || deletions != 0, |el| {
                    el.child(render_file_chooser_diff_summary(additions, deletions))
                }),
        )
}

fn render_file_chooser_diff_summary(additions: i64, deletions: i64) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(10.0))
                .font_family(mono_font_family())
                .text_color(success())
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_size(px(10.0))
                .font_family(mono_font_family())
                .text_color(fg_subtle())
                .child("/"),
        )
        .child(
            div()
                .text_size(px(10.0))
                .font_family(mono_font_family())
                .text_color(danger())
                .child(format!("-{deletions}")),
        )
}

fn open_file_chooser_item(
    item: FileChooserItem,
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let path = item.path;
    state.update(cx, |state, cx| {
        if state.active_detail().is_none() {
            return;
        }

        state.file_chooser_open = false;
        state.file_chooser_query.clear();
        state.file_chooser_selected_index = 0;
        state.file_chooser_list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        state.active_surface = PullRequestSurface::Files;
        state.pr_header_compact = false;
        state.selected_file_path = Some(path.clone());
        state.selected_diff_anchor = None;
        state.set_review_source_target(ReviewSourceTarget {
            path: path.clone(),
            line: None,
            reason: Some("Selected from file chooser".to_string()),
        });
        state.reset_review_focus_scroll();
        state.persist_active_review_session();
        cx.notify();
    });
    ensure_active_review_focus_loaded(state, window, cx);
}

fn filtered_file_chooser_items(state: &AppState) -> Vec<FileChooserItem> {
    let Some(rows) = state
        .active_detail_state()
        .and_then(|detail_state| detail_state.source_file_tree.rows.as_deref())
    else {
        return Vec::new();
    };
    let items = rows
        .iter()
        .filter_map(FileChooserItem::from_row)
        .collect::<Vec<_>>();
    ranked_file_chooser_items(items, state.file_chooser_query.trim())
}

fn ranked_file_chooser_items(items: Vec<FileChooserItem>, query: &str) -> Vec<FileChooserItem> {
    let query_chars = fuzzy_query_chars(query);
    if query_chars.is_empty() {
        return items;
    }

    let mut ranked = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            fuzzy_match_score(&item.search_text, &query_chars).map(|score| (score, index, item))
        })
        .collect::<Vec<_>>();

    ranked.sort_by(
        |(left_score, left_index, left_item), (right_score, right_index, right_item)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_item.path.len().cmp(&right_item.path.len()))
                .then_with(|| left_index.cmp(right_index))
        },
    );

    ranked.into_iter().map(|(_, _, item)| item).collect()
}

fn file_chooser_count_label(
    source_tree: &SourceFileTreeState,
    filtered_count: usize,
    query: &str,
) -> String {
    if source_tree.error.is_some() {
        return "unavailable".to_string();
    }
    if source_tree.loading || source_tree.rows.is_none() {
        return "loading".to_string();
    }
    if query.trim().is_empty() {
        format!("{} files", source_tree.file_count)
    } else {
        format!("{filtered_count} matches")
    }
}

fn file_chooser_status_message(
    source_tree: &SourceFileTreeState,
    rows_loaded: bool,
    filtered_count: usize,
) -> Option<String> {
    if let Some(error) = source_tree.error.as_ref() {
        return Some(error.clone());
    }
    if source_tree.loading {
        return Some("Loading repository files from the local checkout...".to_string());
    }
    if !rows_loaded {
        return Some("Repository files will appear after the local checkout is ready.".to_string());
    }
    (filtered_count == 0).then(|| "No files matched the current query.".to_string())
}

#[derive(Clone, Debug)]
struct FileChooserItem {
    path: String,
    name: String,
    directory: String,
    additions: i64,
    deletions: i64,
    search_text: String,
}

impl FileChooserItem {
    fn from_row(row: &ReviewFileTreeRow) -> Option<Self> {
        let ReviewFileTreeRow::File {
            path,
            name,
            additions,
            deletions,
            ..
        } = row
        else {
            return None;
        };
        let directory = path
            .rsplit_once('/')
            .map(|(directory, _)| directory.to_string())
            .unwrap_or_else(|| "repository root".to_string());
        let search_text = format!("{} {}", lower_for_search(name), lower_for_search(path));
        Some(Self {
            path: path.clone(),
            name: name.clone(),
            directory,
            additions: *additions,
            deletions: *deletions,
            search_text,
        })
    }
}

fn lower_for_search(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::{ranked_file_chooser_items, FileChooserItem};

    #[test]
    fn file_chooser_ranks_filename_matches_before_directory_matches() {
        let ranked = ranked_file_chooser_items(
            vec![
                item("src/source_browser.rs"),
                item("src/browser/root.rs"),
                item("src/views/root.rs"),
            ],
            "browser",
        );

        assert_eq!(ranked[0].path, "src/source_browser.rs");
    }

    #[test]
    fn file_chooser_fuzzy_matches_abbreviated_paths() {
        let ranked = ranked_file_chooser_items(
            vec![item("src/views/root.rs"), item("src/views/file_chooser.rs")],
            "vfch",
        );

        assert_eq!(ranked[0].path, "src/views/file_chooser.rs");
    }

    fn item(path: &str) -> FileChooserItem {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        FileChooserItem {
            path: path.to_string(),
            name: name.clone(),
            directory: path
                .rsplit_once('/')
                .map(|(directory, _)| directory.to_string())
                .unwrap_or_else(|| "repository root".to_string()),
            additions: 0,
            deletions: 0,
            search_text: format!("{} {}", name.to_lowercase(), path.to_lowercase()),
        }
    }
}
