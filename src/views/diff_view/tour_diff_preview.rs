use super::*;

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

pub(super) fn render_tour_diff_file_compact(
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
        let wrap_diff_lines = state
            .read(cx)
            .active_review_session()
            .map(|session| session.wrap_diff_lines)
            .unwrap_or(false);

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
                wrap_diff_lines,
                cx,
            )
            .into_any_element()
        } else {
            render_full_tour_diff_preview(
                parsed_file,
                anchor,
                file_lsp_context.as_ref(),
                wrap_diff_lines,
            )
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
    let mut props = file
        .map(ReviewFileHeaderProps::from_pull_request_file)
        .unwrap_or_else(|| ReviewFileHeaderProps::from_path(parsed_file.path.clone()));
    props.previous_path = parsed_file.previous_path.clone();
    props.binary = parsed_file.is_binary;
    div().mb(px(12.0)).child(render_review_file_header(props))
}

fn render_tour_diff_preview(
    state: &Entity<AppState>,
    file: &PullRequestFile,
    parsed_file: &ParsedDiffFile,
    prepared_file: Option<&PreparedFileContent>,
    selected_anchor: Option<&DiffAnchor>,
    diff_view_state: DiffFileViewState,
    file_lsp_context: Option<DiffFileLspContext>,
    wrap_diff_lines: bool,
    cx: &App,
) -> impl IntoElement {
    let rows = diff_view_state.rows;
    let parsed_file_index = diff_view_state.parsed_file_index;
    let highlighted_hunks = diff_view_state.highlighted_hunks;
    let gutter_layout = diff_gutter_layout(file, Some(parsed_file), false);
    let preview_items = {
        let app_state = state.read(cx);
        build_tour_diff_preview_items(
            app_state.active_detail(),
            file,
            parsed_file,
            prepared_file,
            &rows,
            selected_anchor,
        )
    };
    let side_by_side_scroll_handles = SideBySideScrollHandles::new();

    let elements: Vec<AnyElement> = preview_items
        .items
        .iter()
        .map(|item| match item {
            DiffViewItem::Gap(gap) => render_diff_gap_row(*gap, gutter_layout).into_any_element(),
            DiffViewItem::StackLayerEmpty => div().into_any_element(),
            DiffViewItem::Row(row_ix) => render_virtualized_diff_row(
                state,
                gutter_layout,
                parsed_file_index,
                None,
                None,
                None,
                highlighted_hunks.as_deref(),
                file_lsp_context.as_ref(),
                &rows[*row_ix],
                selected_anchor,
                &side_by_side_scroll_handles,
                None,
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
        .border_color(transparent())
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
        .child(render_tour_diff_rows_container(
            file.path.as_str(),
            elements,
            wrap_diff_lines,
        ))
}

fn render_full_tour_diff_preview(
    parsed_file: &ParsedDiffFile,
    anchor: Option<&DiffAnchor>,
    file_lsp_context: Option<&DiffFileLspContext>,
    wrap_diff_lines: bool,
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
                    None,
                    false,
                    false,
                    false,
                    wrap_diff_lines,
                )
                .into_any_element(),
            );
        }
    }

    render_tour_diff_rows_container(parsed_file.path.as_str(), elements, wrap_diff_lines)
}

fn render_tour_diff_rows_container(
    id_key: &str,
    elements: Vec<AnyElement>,
    wrap_diff_lines: bool,
) -> AnyElement {
    let rows = div()
        .flex()
        .flex_col()
        .min_w(px(if wrap_diff_lines {
            0.0
        } else {
            DIFF_UNIFIED_MIN_WIDTH
        }))
        .children(elements);

    if wrap_diff_lines {
        div()
            .flex()
            .flex_col()
            .bg(diff_editor_bg())
            .child(rows)
            .into_any_element()
    } else {
        restrict_diff_scroll_to_axis(div().flex().flex_col().bg(diff_editor_bg()))
            .id(ElementId::Name(
                format!("tour-diff-horizontal-scroll-{id_key}").into(),
            ))
            .overflow_x_scroll()
            .scrollbar_width(px(DIFF_SCROLLBAR_WIDTH))
            .child(rows)
            .into_any_element()
    }
}

const TOUR_PREVIEW_MAX_ITEMS: usize = 96;
const TOUR_PREVIEW_CONTEXT_ITEMS: usize = 24;

struct TourDiffPreviewItems {
    items: Vec<DiffViewItem>,
    focused_excerpt: bool,
}

fn build_tour_diff_preview_items(
    detail: Option<&PullRequestDetail>,
    file: &PullRequestFile,
    parsed_file: &ParsedDiffFile,
    prepared_file: Option<&PreparedFileContent>,
    rows: &[DiffRenderRow],
    selected_anchor: Option<&DiffAnchor>,
) -> TourDiffPreviewItems {
    let full_items = build_diff_view_items(
        file,
        Some(parsed_file),
        prepared_file,
        rows,
        None,
        None,
        None,
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
