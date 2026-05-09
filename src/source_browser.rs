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
    review_session::ReviewSourceTarget,
    state::AppState,
    theme::*,
};

pub fn render_source_browser(
    state: &Entity<AppState>,
    target: &ReviewSourceTarget,
    parsed: Option<&ParsedDiffFile>,
    cx: &App,
) -> AnyElement {
    let prepared_file = {
        let app_state = state.read(cx);
        app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.file_content_states.get(&target.path))
            .and_then(|file_state| file_state.prepared.as_ref())
            .cloned()
    };

    let shell = div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .rounded(radius())
        .border_1()
        .border_color(border_default())
        .bg(bg_surface())
        .overflow_hidden();

    let Some(prepared_file) = prepared_file else {
        return shell
            .child(
                div()
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
    let list_state = prepare_source_browser_list_state(state, target.path.as_str(), cx);
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

    shell
        .child(
            div()
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

fn prepare_source_browser_list_state(
    state: &Entity<AppState>,
    file_path: &str,
    cx: &App,
) -> ListState {
    let app_state = state.read(cx);
    let state_key = format!(
        "source:{}:{file_path}",
        app_state.active_pr_key.as_deref().unwrap_or("detached")
    );
    app_state
        .source_browser_list_states
        .borrow_mut()
        .entry(state_key)
        .or_insert_with(|| ListState::new(0, ListAlignment::Top, px(400.0)))
        .clone()
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
