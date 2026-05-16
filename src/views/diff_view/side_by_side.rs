use super::*;

#[derive(Clone)]
pub(super) struct NormalSideBySideDiffFile {
    pub(super) hunks: Vec<NormalSideBySideHunk>,
    pub(super) line_map: Vec<Vec<Option<NormalSideBySideLineMap>>>,
}

#[derive(Clone)]
pub(super) struct NormalSideBySideHunk {
    pub(super) rows: Vec<NormalSideBySideRow>,
}

#[derive(Clone, Copy)]
pub(super) struct NormalSideBySideRow {
    pub(super) left_line_index: Option<usize>,
    pub(super) right_line_index: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct NormalSideBySideLineMap {
    pub(super) row_index: usize,
    pub(super) primary: bool,
}

#[derive(Clone)]
pub(super) struct DiffScrollFocus {
    pub(super) file_path: String,
    pub(super) line: Option<usize>,
    pub(super) side: Option<String>,
    pub(super) hunk_header: Option<String>,
    pub(super) anchor: Option<DiffAnchor>,
}

pub(super) fn build_normal_side_by_side_diff_file(
    parsed: &ParsedDiffFile,
) -> NormalSideBySideDiffFile {
    let mut hunks = Vec::with_capacity(parsed.hunks.len());
    let mut line_map = Vec::with_capacity(parsed.hunks.len());

    for hunk in &parsed.hunks {
        let mut rows = Vec::new();
        let mut hunk_line_map = vec![None; hunk.lines.len()];
        let mut line_ix = 0usize;

        while line_ix < hunk.lines.len() {
            match hunk.lines[line_ix].kind {
                DiffLineKind::Addition | DiffLineKind::Deletion => {
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

                    let row_count = deletions.len().max(additions.len());
                    for row_offset in 0..row_count {
                        let left_line_index = deletions.get(row_offset).copied();
                        let right_line_index = additions.get(row_offset).copied();
                        let row_index = rows.len();
                        let primary_line_index = match (left_line_index, right_line_index) {
                            (Some(left), Some(right)) => left.min(right),
                            (Some(left), None) => left,
                            (None, Some(right)) => right,
                            (None, None) => continue,
                        };

                        if let Some(left) = left_line_index {
                            hunk_line_map[left] = Some(NormalSideBySideLineMap {
                                row_index,
                                primary: left == primary_line_index,
                            });
                        }
                        if let Some(right) = right_line_index {
                            hunk_line_map[right] = Some(NormalSideBySideLineMap {
                                row_index,
                                primary: right == primary_line_index,
                            });
                        }

                        rows.push(NormalSideBySideRow {
                            left_line_index,
                            right_line_index,
                        });
                    }
                }
                DiffLineKind::Context | DiffLineKind::Meta => {
                    let row_index = rows.len();
                    hunk_line_map[line_ix] = Some(NormalSideBySideLineMap {
                        row_index,
                        primary: true,
                    });
                    rows.push(NormalSideBySideRow {
                        left_line_index: Some(line_ix),
                        right_line_index: Some(line_ix),
                    });
                    line_ix += 1;
                }
            }
        }

        hunks.push(NormalSideBySideHunk { rows });
        line_map.push(hunk_line_map);
    }

    NormalSideBySideDiffFile { hunks, line_map }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SideBySideDiffSide {
    Left,
    Right,
}

impl SideBySideDiffSide {
    pub(super) fn id_label(self) -> &'static str {
        match self {
            SideBySideDiffSide::Left => "left",
            SideBySideDiffSide::Right => "right",
        }
    }
}

#[derive(Clone)]
pub(super) struct SideBySideScrollHandles {
    pub(super) left: ScrollHandle,
    pub(super) right: ScrollHandle,
}

impl SideBySideScrollHandles {
    pub(super) fn new() -> Self {
        Self {
            left: ScrollHandle::new(),
            right: ScrollHandle::new(),
        }
    }

    pub(super) fn handle_for(&self, side: SideBySideDiffSide) -> &ScrollHandle {
        match side {
            SideBySideDiffSide::Left => &self.left,
            SideBySideDiffSide::Right => &self.right,
        }
    }

    pub(super) fn left_has_horizontal_scroll(&self) -> bool {
        self.left.max_offset().width > px(0.0)
    }

    pub(super) fn right_has_horizontal_scroll(&self) -> bool {
        self.right.max_offset().width > px(0.0)
    }
}

#[derive(Clone, Copy)]
pub(super) struct SideBySideColumnWidths {
    pub(super) left: f32,
    pub(super) right: f32,
}

impl SideBySideColumnWidths {
    pub(super) fn width_for(self, side: SideBySideDiffSide) -> f32 {
        match side {
            SideBySideDiffSide::Left => self.left,
            SideBySideDiffSide::Right => self.right,
        }
    }

    pub(super) fn max(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            right: self.right.max(other.right),
        }
    }
}

pub(super) fn combined_side_by_side_column_widths(
    contexts: &[CombinedDiffFileContext],
) -> Option<SideBySideColumnWidths> {
    max_side_by_side_column_widths(
        contexts
            .iter()
            .filter(|context| !context.collapsed)
            .filter_map(|context| context.side_by_side_column_widths),
    )
}

pub(super) fn max_side_by_side_column_widths(
    widths: impl Iterator<Item = SideBySideColumnWidths>,
) -> Option<SideBySideColumnWidths> {
    widths.reduce(SideBySideColumnWidths::max)
}

pub(super) fn side_by_side_column_widths_for_file(
    parsed: Option<&ParsedDiffFile>,
    structural_side_by_side: Option<&crate::difftastic::AdaptedDifftasticDiffFile>,
    normal_side_by_side: Option<&NormalSideBySideDiffFile>,
    reserve_waypoint_slot: bool,
    wrap_diff_lines: bool,
) -> Option<SideBySideColumnWidths> {
    structural_side_by_side
        .map(|side_by_side| {
            structural_side_by_side_column_widths(
                side_by_side,
                reserve_waypoint_slot,
                wrap_diff_lines,
            )
        })
        .or_else(|| {
            parsed.and_then(|parsed| {
                normal_side_by_side.map(|side_by_side| {
                    normal_side_by_side_column_widths(
                        parsed,
                        side_by_side,
                        reserve_waypoint_slot,
                        wrap_diff_lines,
                    )
                })
            })
        })
}

pub(super) fn normal_side_by_side_column_widths(
    parsed: &ParsedDiffFile,
    side_by_side: &NormalSideBySideDiffFile,
    reserve_waypoint_slot: bool,
    wrap_diff_lines: bool,
) -> SideBySideColumnWidths {
    let left_layout = side_by_side_gutter_layout(SideBySideDiffSide::Left, reserve_waypoint_slot);
    let right_layout = side_by_side_gutter_layout(SideBySideDiffSide::Right, reserve_waypoint_slot);
    let mut widths = SideBySideColumnWidths {
        left: side_by_side_cell_min_width(left_layout, None, wrap_diff_lines),
        right: side_by_side_cell_min_width(right_layout, None, wrap_diff_lines),
    };

    for (hunk_ix, hunk) in parsed.hunks.iter().enumerate() {
        let Some(side_by_side_hunk) = side_by_side.hunks.get(hunk_ix) else {
            continue;
        };
        for row in &side_by_side_hunk.rows {
            if let Some(line) = row
                .left_line_index
                .and_then(|line_ix| hunk.lines.get(line_ix))
            {
                widths.left = widths.left.max(side_by_side_cell_min_width(
                    left_layout,
                    Some(line.content.as_str()),
                    wrap_diff_lines,
                ));
            }
            if let Some(line) = row
                .right_line_index
                .and_then(|line_ix| hunk.lines.get(line_ix))
            {
                widths.right = widths.right.max(side_by_side_cell_min_width(
                    right_layout,
                    Some(line.content.as_str()),
                    wrap_diff_lines,
                ));
            }
        }
    }

    widths
}

pub(super) fn structural_side_by_side_column_widths(
    side_by_side: &crate::difftastic::AdaptedDifftasticDiffFile,
    reserve_waypoint_slot: bool,
    wrap_diff_lines: bool,
) -> SideBySideColumnWidths {
    let left_layout = side_by_side_gutter_layout(SideBySideDiffSide::Left, reserve_waypoint_slot);
    let right_layout = side_by_side_gutter_layout(SideBySideDiffSide::Right, reserve_waypoint_slot);
    let mut widths = SideBySideColumnWidths {
        left: side_by_side_cell_min_width(left_layout, None, wrap_diff_lines),
        right: side_by_side_cell_min_width(right_layout, None, wrap_diff_lines),
    };

    for hunk in &side_by_side.side_by_side_hunks {
        for row in &hunk.rows {
            if let Some(cell) = row.left.as_ref() {
                widths.left = widths.left.max(side_by_side_cell_min_width(
                    left_layout,
                    Some(cell.line.content.as_str()),
                    wrap_diff_lines,
                ));
            }
            if let Some(cell) = row.right.as_ref() {
                widths.right = widths.right.max(side_by_side_cell_min_width(
                    right_layout,
                    Some(cell.line.content.as_str()),
                    wrap_diff_lines,
                ));
            }
        }
    }

    widths
}

pub(super) fn render_normal_side_by_side_diff_row(
    state: &Entity<AppState>,
    reserve_waypoint_slot: bool,
    detail: Option<&PullRequestDetail>,
    parsed_file: &ParsedDiffFile,
    file_path: &str,
    hunk_index: usize,
    hunk_header: &str,
    row_index: usize,
    hunk: &ParsedDiffHunk,
    highlighted_hunk: Option<&Vec<DiffLineHighlight>>,
    row: &NormalSideBySideRow,
    selected_anchor: Option<&DiffAnchor>,
    file_lsp_context: Option<&DiffFileLspContext>,
    side_by_side_scroll_handles: &SideBySideScrollHandles,
    column_widths: SideBySideColumnWidths,
    cx: &App,
) -> impl IntoElement {
    let row_scroll_key = format!("normal-side-by-side-scroll:{file_path}:{hunk_index}:{row_index}");
    div()
        .flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .bg(diff_editor_bg())
        .child(render_normal_side_by_side_cell(
            state,
            SideBySideDiffSide::Left,
            reserve_waypoint_slot,
            row_scroll_key.as_str(),
            side_by_side_scroll_handles.handle_for(SideBySideDiffSide::Left),
            column_widths.width_for(SideBySideDiffSide::Left),
            file_path,
            hunk_header,
            hunk,
            highlighted_hunk,
            row.left_line_index,
            selected_anchor,
            None,
            detail,
            parsed_file,
            cx,
        ))
        .child(render_normal_side_by_side_cell(
            state,
            SideBySideDiffSide::Right,
            reserve_waypoint_slot,
            row_scroll_key.as_str(),
            side_by_side_scroll_handles.handle_for(SideBySideDiffSide::Right),
            column_widths.width_for(SideBySideDiffSide::Right),
            file_path,
            hunk_header,
            hunk,
            highlighted_hunk,
            row.right_line_index,
            selected_anchor,
            file_lsp_context,
            detail,
            parsed_file,
            cx,
        ))
}

fn render_normal_side_by_side_cell(
    state: &Entity<AppState>,
    side: SideBySideDiffSide,
    reserve_waypoint_slot: bool,
    row_scroll_key: &str,
    scroll_handle: &ScrollHandle,
    column_width: f32,
    file_path: &str,
    hunk_header: &str,
    hunk: &ParsedDiffHunk,
    highlighted_hunk: Option<&Vec<DiffLineHighlight>>,
    line_index: Option<usize>,
    selected_anchor: Option<&DiffAnchor>,
    file_lsp_context: Option<&DiffFileLspContext>,
    detail: Option<&PullRequestDetail>,
    parsed_file: &ParsedDiffFile,
    cx: &App,
) -> AnyElement {
    let gutter_layout = side_by_side_gutter_layout(side, reserve_waypoint_slot);
    let wrap_diff_lines = state
        .read(cx)
        .active_review_session()
        .map(|session| session.wrap_diff_lines)
        .unwrap_or(false);
    let line_entry =
        line_index.and_then(|line_index| hunk.lines.get(line_index).map(|line| (line_index, line)));
    let cell_min_width = column_width.max(side_by_side_cell_min_width(
        gutter_layout,
        line_entry.map(|(_, line)| line.content.as_str()),
        wrap_diff_lines,
    ));
    let content = line_entry
        .map(|(line_index, line)| {
            let highlight = highlighted_hunk
                .and_then(|lines| lines.get(line_index))
                .cloned()
                .unwrap_or_default();
            let line_lsp_context = (side == SideBySideDiffSide::Right)
                .then(|| build_diff_line_lsp_context(file_lsp_context, line))
                .flatten();
            let target_side = match side {
                SideBySideDiffSide::Left => TempSourceSide::Base,
                SideBySideDiffSide::Right => TempSourceSide::Head,
            };
            let temp_source_target = detail.and_then(|detail| {
                temp_source_target_for_diff_side(detail, parsed_file, line, target_side)
            });

            render_reviewable_diff_line(
                state,
                gutter_layout,
                file_path,
                Some(hunk_header),
                line,
                Some(highlight.syntax_spans.as_slice()),
                Some(highlight.emphasis_ranges.as_slice()),
                selected_anchor,
                line_lsp_context.as_ref(),
                temp_source_target,
                cx,
            )
            .into_any_element()
        })
        .unwrap_or_else(|| {
            render_empty_side_by_side_cell(
                gutter_layout,
                side == SideBySideDiffSide::Left
                    && should_stripe_missing_previous_side(parsed_file),
            )
            .into_any_element()
        });

    render_side_by_side_cell_container(
        side,
        row_scroll_key,
        scroll_handle,
        content,
        cell_min_width,
        wrap_diff_lines,
    )
}

pub(super) fn render_structural_side_by_side_diff_row(
    state: &Entity<AppState>,
    gutter_layout: DiffGutterLayout,
    detail: Option<&PullRequestDetail>,
    parsed_file: &ParsedDiffFile,
    file_path: &str,
    hunk_index: usize,
    hunk_header: &str,
    row_index: usize,
    row: &crate::difftastic::AdaptedDifftasticSideBySideRow,
    selected_anchor: Option<&DiffAnchor>,
    file_lsp_context: Option<&DiffFileLspContext>,
    side_by_side_scroll_handles: &SideBySideScrollHandles,
    column_widths: SideBySideColumnWidths,
    cx: &App,
) -> impl IntoElement {
    let row_scroll_key =
        format!("structural-side-by-side-scroll:{file_path}:{hunk_index}:{row_index}");
    div()
        .flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .bg(diff_editor_bg())
        .child(render_structural_side_by_side_cell(
            state,
            SideBySideDiffSide::Left,
            gutter_layout.reserve_waypoint_slot,
            row_scroll_key.as_str(),
            side_by_side_scroll_handles.handle_for(SideBySideDiffSide::Left),
            column_widths.width_for(SideBySideDiffSide::Left),
            file_path,
            hunk_header,
            row.left.as_ref(),
            selected_anchor,
            None,
            detail,
            parsed_file,
            cx,
        ))
        .child(render_structural_side_by_side_cell(
            state,
            SideBySideDiffSide::Right,
            gutter_layout.reserve_waypoint_slot,
            row_scroll_key.as_str(),
            side_by_side_scroll_handles.handle_for(SideBySideDiffSide::Right),
            column_widths.width_for(SideBySideDiffSide::Right),
            file_path,
            hunk_header,
            row.right.as_ref(),
            selected_anchor,
            file_lsp_context,
            detail,
            parsed_file,
            cx,
        ))
}

fn render_structural_side_by_side_cell(
    state: &Entity<AppState>,
    side: SideBySideDiffSide,
    reserve_waypoint_slot: bool,
    row_scroll_key: &str,
    scroll_handle: &ScrollHandle,
    column_width: f32,
    file_path: &str,
    hunk_header: &str,
    cell: Option<&crate::difftastic::AdaptedDifftasticSideBySideCell>,
    selected_anchor: Option<&DiffAnchor>,
    file_lsp_context: Option<&DiffFileLspContext>,
    detail: Option<&PullRequestDetail>,
    parsed_file: &ParsedDiffFile,
    cx: &App,
) -> AnyElement {
    let gutter_layout = side_by_side_gutter_layout(side, reserve_waypoint_slot);
    let wrap_diff_lines = state
        .read(cx)
        .active_review_session()
        .map(|session| session.wrap_diff_lines)
        .unwrap_or(false);
    let cell_min_width = column_width.max(side_by_side_cell_min_width(
        gutter_layout,
        cell.map(|cell| cell.line.content.as_str()),
        wrap_diff_lines,
    ));
    let content = cell
        .map(|cell| {
            let emphasis_ranges = if DIFF_INLINE_EMPHASIS_ENABLED {
                normalize_inline_emphasis_ranges(
                    cell.line.content.as_str(),
                    cell.emphasis_ranges.as_slice(),
                )
            } else {
                Vec::new()
            };
            let emphasis_ranges =
                (!emphasis_ranges.is_empty()).then_some(emphasis_ranges.as_slice());
            let line_lsp_context = (side == SideBySideDiffSide::Right)
                .then(|| build_diff_line_lsp_context(file_lsp_context, &cell.line))
                .flatten();
            let target_side = match side {
                SideBySideDiffSide::Left => TempSourceSide::Base,
                SideBySideDiffSide::Right => TempSourceSide::Head,
            };
            let temp_source_target = detail.and_then(|detail| {
                temp_source_target_for_diff_side(detail, parsed_file, &cell.line, target_side)
            });

            render_reviewable_diff_line(
                state,
                gutter_layout,
                file_path,
                Some(hunk_header),
                &cell.line,
                None,
                emphasis_ranges,
                selected_anchor,
                line_lsp_context.as_ref(),
                temp_source_target,
                cx,
            )
            .into_any_element()
        })
        .unwrap_or_else(|| {
            render_empty_side_by_side_cell(
                gutter_layout,
                side == SideBySideDiffSide::Left
                    && should_stripe_missing_previous_side(parsed_file),
            )
            .into_any_element()
        });

    render_side_by_side_cell_container(
        side,
        row_scroll_key,
        scroll_handle,
        content,
        cell_min_width,
        wrap_diff_lines,
    )
}

fn render_side_by_side_cell_container(
    side: SideBySideDiffSide,
    row_scroll_key: &str,
    scroll_handle: &ScrollHandle,
    content: AnyElement,
    cell_min_width: f32,
    wrap_diff_lines: bool,
) -> AnyElement {
    let content = if wrap_diff_lines {
        content
    } else {
        restrict_diff_scroll_to_axis(div().w_full().min_w_0())
            .id(ElementId::Name(
                format!("{row_scroll_key}:{}", side.id_label()).into(),
            ))
            .overflow_x_scroll()
            .track_scroll(scroll_handle)
            .child(div().min_w(px(cell_min_width)).child(content))
            .into_any_element()
    };

    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .when(side == SideBySideDiffSide::Left, |el| {
            el.border_r(px(1.0)).border_color(diff_gutter_separator())
        })
        .child(content)
        .into_any_element()
}

fn side_by_side_gutter_layout(
    side: SideBySideDiffSide,
    reserve_waypoint_slot: bool,
) -> DiffGutterLayout {
    DiffGutterLayout {
        show_left_numbers: side == SideBySideDiffSide::Left,
        show_right_numbers: side == SideBySideDiffSide::Right,
        reserve_waypoint_slot,
        reserve_source_slot: true,
    }
}

fn render_empty_side_by_side_cell(
    gutter_layout: DiffGutterLayout,
    striped: bool,
) -> impl IntoElement {
    div()
        .relative()
        .flex()
        .w_full()
        .min_w_0()
        .min_h(diff_row_height_px())
        .overflow_hidden()
        .bg(diff_context_bg())
        .font_family(mono_font_family())
        .text_size(diff_code_font_size_px())
        .line_height(diff_code_line_height_px())
        .font_weight(FontWeight::MEDIUM)
        .text_color(transparent())
        .when(striped, |el| {
            let stripe_color: Hsla = with_alpha(fg_subtle(), 0.16).into();
            el.child(
                div()
                    .absolute()
                    .size_full()
                    .bg(pattern_slash(stripe_color, 1.0, 7.0)),
            )
        })
        .when(!striped, |el| {
            el.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .w(px(gutter_layout.gutter_width()))
                    .min_h(diff_row_height_px())
                    .bg(diff_context_gutter_bg())
                    .border_r(px(1.0))
                    .border_color(diff_gutter_separator())
                    .when(gutter_layout.reserve_source_slot, |el| {
                        el.child(div().w(diff_source_slot_width_px()).h_full())
                    })
                    .when(gutter_layout.reserve_waypoint_slot, |el| {
                        el.child(div().w(diff_waypoint_slot_width_px()).h_full())
                    })
                    .child(
                        div()
                            .w(px(diff_line_number_column_width()))
                            .px(px(DIFF_LINE_NUMBER_CELL_PADDING_X))
                            .flex()
                            .justify_end()
                            .text_size(diff_line_number_font_size_px())
                            .line_height(diff_code_line_height_px())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(" "),
                    ),
            )
            .child(
                div()
                    .w(px(diff_marker_column_width()))
                    .flex_shrink_0()
                    .min_h(diff_row_height_px())
                    .py(px(1.0))
                    .child(" "),
            )
            .child(
                div()
                    .flex_grow()
                    .min_w_0()
                    .px(px(8.0))
                    .py(px(1.0))
                    .child("\u{00a0}".to_string()),
            )
        })
}

pub(super) fn should_stripe_missing_previous_side(parsed_file: &ParsedDiffFile) -> bool {
    let mut has_additions = false;
    let mut has_removals = false;

    for line in parsed_file.hunks.iter().flat_map(|hunk| hunk.lines.iter()) {
        match line.kind {
            DiffLineKind::Addition => has_additions = true,
            DiffLineKind::Deletion => has_removals = true,
            DiffLineKind::Context | DiffLineKind::Meta => {}
        }
    }

    has_additions && !has_removals
}
