use std::{collections::BTreeMap, sync::Arc};

use gpui::prelude::*;
use gpui::*;

use crate::{
    code_display::{
        build_prepared_file_lsp_context,
        render_virtualized_prepared_file_with_line_numbers_and_focus,
        render_virtualized_prepared_file_with_line_numbers_diffs_and_focus,
        PreparedFileLineDiffKind, PreparedFileLineDiffs,
    },
    diff::{DiffLineKind, ParsedDiffFile},
    review_file_header::{render_review_file_header, ReviewFileHeaderProps},
    review_session::{ReviewCenterMode, ReviewSourceTarget},
    state::{AppState, PullRequestSurface, SourceBrowserViewState},
    theme::*,
};

const SOURCE_DIFF_CONTENT_LEFT_GUTTER: f32 = 36.0;
const SOURCE_DIFF_CONTENT_RIGHT_GUTTER: f32 = 16.0;
const SOURCE_DIFF_SECTION_LEFT_MARGIN: f32 = 0.0;
const SOURCE_DIFF_SECTION_RIGHT_MARGIN: f32 = 0.0;
const SOURCE_DIFF_SECTION_BODY_INSET: f32 = 12.0;
const SOURCE_DIFF_SECTION_BODY_LEFT_MARGIN: f32 =
    SOURCE_DIFF_SECTION_LEFT_MARGIN + SOURCE_DIFF_SECTION_BODY_INSET;
const SOURCE_DIFF_SECTION_BODY_RIGHT_MARGIN: f32 =
    SOURCE_DIFF_SECTION_RIGHT_MARGIN + SOURCE_DIFF_SECTION_BODY_INSET;
const SOURCE_DIFF_FILE_HEADER_TOP_MARGIN: f32 = 14.0;
const SOURCE_DIFF_FILE_HEADER_BOTTOM_MARGIN: f32 = 0.0;

pub fn render_source_browser(
    state: &Entity<AppState>,
    target: &ReviewSourceTarget,
    parsed: Option<&ParsedDiffFile>,
    cx: &App,
) -> AnyElement {
    let (prepared_file, changed_file) = {
        let app_state = state.read(cx);
        let prepared_file = app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.file_content_states.get(&target.path))
            .and_then(|file_state| file_state.prepared.as_ref())
            .cloned();
        let changed_file = app_state.active_detail().and_then(|detail| {
            detail
                .files
                .iter()
                .find(|file| file.path == target.path)
                .cloned()
        });
        (prepared_file, changed_file)
    };
    let mut header = changed_file
        .as_ref()
        .map(ReviewFileHeaderProps::from_pull_request_file)
        .unwrap_or_else(|| ReviewFileHeaderProps::from_path(target.path.clone()));
    header.previous_path = parsed.and_then(|parsed| parsed.previous_path.clone());
    header.binary = parsed
        .map(|parsed| parsed.is_binary)
        .or_else(|| prepared_file.as_ref().map(|prepared| prepared.is_binary))
        .unwrap_or(false);
    header.active = true;

    let shell = div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .bg(diff_editor_bg())
        .overflow_hidden()
        .pl(px(SOURCE_DIFF_CONTENT_LEFT_GUTTER))
        .pr(px(SOURCE_DIFF_CONTENT_RIGHT_GUTTER))
        .child(
            div()
                .ml(px(SOURCE_DIFF_SECTION_LEFT_MARGIN))
                .mr(px(SOURCE_DIFF_SECTION_RIGHT_MARGIN))
                .mt(px(SOURCE_DIFF_FILE_HEADER_TOP_MARGIN))
                .mb(px(SOURCE_DIFF_FILE_HEADER_BOTTOM_MARGIN))
                .child(render_review_file_header(header)),
        );

    let Some(prepared_file) = prepared_file else {
        return shell
            .child(
                div()
                    .ml(px(SOURCE_DIFF_SECTION_BODY_LEFT_MARGIN))
                    .mr(px(SOURCE_DIFF_SECTION_BODY_RIGHT_MARGIN))
                    .border_l(px(1.0))
                    .border_r(px(1.0))
                    .border_color(diff_annotation_border())
                    .flex_grow()
                    .min_h_0()
                    .p(px(18.0))
                    .child(source_state_text(
                        "Loading source context from the local checkout...",
                    )),
            )
            .into_any_element();
    };

    let lsp_context =
        build_prepared_file_lsp_context(state, target.path.as_str(), Some(&prepared_file), cx);
    let view_state = prepare_source_browser_view_state(state, target.path.as_str(), cx);
    let list_state = view_state.list_state.clone();
    update_source_browser_scroll_focus(state, target.path.clone(), list_state.clone());
    let full_file = if let Some(parsed) = parsed {
        render_virtualized_prepared_file_with_line_numbers_diffs_and_focus(
            &prepared_file,
            lsp_context.as_ref(),
            build_full_file_diff_lines(parsed),
            list_state,
            target.line,
        )
    } else {
        render_virtualized_prepared_file_with_line_numbers_and_focus(
            &prepared_file,
            lsp_context.as_ref(),
            list_state,
            target.line,
        )
    };
    scroll_source_browser_to_focus(&view_state, target, prepared_file.lines.len());

    shell
        .child(
            div()
                .ml(px(SOURCE_DIFF_SECTION_BODY_LEFT_MARGIN))
                .mr(px(SOURCE_DIFF_SECTION_BODY_RIGHT_MARGIN))
                .border_l(px(1.0))
                .border_r(px(1.0))
                .border_color(diff_annotation_border())
                .flex_grow()
                .min_h_0()
                .id("source-browser-scroll")
                .p(px(10.0))
                .flex()
                .flex_col()
                .child(full_file),
        )
        .into_any_element()
}

fn update_source_browser_scroll_focus(
    state: &Entity<AppState>,
    file_path: String,
    list_state: ListState,
) {
    let state_for_scroll = state.clone();
    let list_state_for_handler = list_state.clone();
    list_state.set_scroll_handler(move |_, window, _| {
        let state = state_for_scroll.clone();
        let list_state = list_state_for_handler.clone();
        let file_path = file_path.clone();
        window.on_next_frame(move |_, cx| {
            let line = list_state.logical_scroll_top().item_ix.saturating_add(1);
            state.update(cx, |state, _| {
                if state.active_surface != PullRequestSurface::Files
                    || state
                        .active_review_session()
                        .map(|session| session.center_mode != ReviewCenterMode::SourceBrowser)
                        .unwrap_or(true)
                {
                    return;
                }

                state.set_review_scroll_focus(
                    ReviewCenterMode::SourceBrowser,
                    file_path,
                    Some(line),
                    Some("RIGHT".to_string()),
                    None,
                );
            });
        });
    });
}

fn prepare_source_browser_view_state(
    state: &Entity<AppState>,
    file_path: &str,
    cx: &App,
) -> SourceBrowserViewState {
    let app_state = state.read(cx);
    let state_key = format!(
        "source:{}:{file_path}",
        app_state.active_pr_key.as_deref().unwrap_or("detached")
    );
    app_state
        .source_browser_list_states
        .borrow_mut()
        .entry(state_key)
        .or_insert_with(SourceBrowserViewState::new)
        .clone()
}

fn scroll_source_browser_to_focus(
    view_state: &SourceBrowserViewState,
    target: &ReviewSourceTarget,
    line_count: usize,
) {
    let Some(line) = target.line.filter(|line| *line > 0 && *line <= line_count) else {
        return;
    };
    let focus_key = format!("{}:{line}", target.path);
    let mut last_focus_key = view_state.last_focus_key.borrow_mut();
    if last_focus_key.as_deref() == Some(focus_key.as_str()) {
        return;
    }

    view_state.list_state.scroll_to(ListOffset {
        item_ix: line.saturating_sub(6),
        offset_in_item: px(0.0),
    });
    *last_focus_key = Some(focus_key);
}

pub(crate) fn build_full_file_diff_lines(parsed: &ParsedDiffFile) -> PreparedFileLineDiffs {
    let mut lines = BTreeMap::new();

    for line in parsed.hunks.iter().flat_map(|hunk| hunk.lines.iter()) {
        if line.kind != DiffLineKind::Addition {
            continue;
        }

        if let Some(line_number) = line
            .right_line_number
            .and_then(|line_number| usize::try_from(line_number).ok())
            .filter(|line_number| *line_number > 0)
        {
            lines.insert(line_number, PreparedFileLineDiffKind::Addition);
        }
    }

    Arc::new(lines)
}

#[cfg(test)]
mod tests {
    use super::build_full_file_diff_lines;
    use crate::{
        code_display::PreparedFileLineDiffKind,
        diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine},
    };

    #[test]
    fn full_file_diff_lines_highlight_added_right_side_lines() {
        let parsed = ParsedDiffFile {
            path: "src/lib.rs".to_string(),
            previous_path: Some("src/lib.rs".to_string()),
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,2 +1,3 @@".to_string(),
                lines: vec![
                    ParsedDiffLine {
                        kind: DiffLineKind::Context,
                        prefix: " ".to_string(),
                        left_line_number: Some(1),
                        right_line_number: Some(1),
                        content: "fn main() {".to_string(),
                    },
                    ParsedDiffLine {
                        kind: DiffLineKind::Deletion,
                        prefix: "-".to_string(),
                        left_line_number: Some(2),
                        right_line_number: None,
                        content: "    old();".to_string(),
                    },
                    ParsedDiffLine {
                        kind: DiffLineKind::Addition,
                        prefix: "+".to_string(),
                        left_line_number: None,
                        right_line_number: Some(2),
                        content: "    new();".to_string(),
                    },
                    ParsedDiffLine {
                        kind: DiffLineKind::Addition,
                        prefix: "+".to_string(),
                        left_line_number: None,
                        right_line_number: Some(3),
                        content: "}".to_string(),
                    },
                ],
            }],
        };

        let highlighted = build_full_file_diff_lines(&parsed);

        assert_eq!(highlighted.len(), 2);
        assert_eq!(
            highlighted.get(&2),
            Some(&PreparedFileLineDiffKind::Addition)
        );
        assert_eq!(
            highlighted.get(&3),
            Some(&PreparedFileLineDiffKind::Addition)
        );
        assert!(!highlighted.contains_key(&1));
    }
}

fn source_state_text(message: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(fg_muted())
        .child(message.to_string())
}
