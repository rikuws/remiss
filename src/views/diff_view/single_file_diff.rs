use super::*;

pub(super) fn render_structural_file_diff(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    structural_state: Option<&StructuralDiffFileState>,
    prepared_file: Option<&PreparedFileContent>,
    selected_anchor: Option<&DiffAnchor>,
    review_stack: Arc<ReviewStack>,
    cx: &App,
) -> AnyElement {
    let Some(structural_state) = structural_state else {
        return render_diff_state_row("Preparing structural diff with difftastic...")
            .into_any_element();
    };

    if structural_state.loading {
        return render_diff_state_row("Building structural diff with difftastic...")
            .into_any_element();
    }

    if let Some(error) = structural_state.error.as_deref() {
        return render_diff_state_row(format!("Structural diff unavailable: {error}"))
            .into_any_element();
    }

    let Some(structural) = structural_state.diff.as_ref() else {
        return render_diff_state_row("Preparing structural diff with difftastic...")
            .into_any_element();
    };

    let request_key = structural_state
        .request_key
        .as_deref()
        .unwrap_or("structural");
    let diff_view_state =
        prepare_structural_diff_view_state(app_state, detail, &file.path, request_key, structural);
    let parsed_override = Arc::new(structural.parsed_file.clone());
    let structural_diff_layout = app_state
        .active_review_session()
        .map(|session| session.structural_diff_layout)
        .unwrap_or(DiffLayout::SideBySide);
    let structural_side_by_side =
        (structural_diff_layout == DiffLayout::SideBySide).then(|| structural.clone());

    render_file_diff(
        state,
        file,
        Some(&structural.parsed_file),
        Some(parsed_override),
        structural_side_by_side,
        prepared_file,
        selected_anchor,
        diff_view_state,
        review_stack,
        None,
        structural_diff_layout,
        cx,
    )
    .into_any_element()
}

pub(super) fn render_file_diff(
    state: &Entity<AppState>,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    parsed_override: Option<Arc<ParsedDiffFile>>,
    structural_side_by_side: Option<Arc<crate::difftastic::AdaptedDifftasticDiffFile>>,
    prepared_file: Option<&PreparedFileContent>,
    selected_anchor: Option<&DiffAnchor>,
    diff_view_state: DiffFileViewState,
    review_stack: Arc<ReviewStack>,
    stack_filter: Option<LayerDiffFilter>,
    diff_layout: DiffLayout,
    cx: &App,
) -> impl IntoElement {
    let rows = diff_view_state.rows.clone();
    let parsed_file_index = diff_view_state.parsed_file_index;
    let highlighted_hunks = diff_view_state.highlighted_hunks.clone();
    let (reserve_waypoint_slot, wrap_diff_lines) = {
        let app_state = state.read(cx);
        let has_review_submission = app_state
            .active_detail()
            .map(|detail| !crate::local_review::is_local_review_detail(detail))
            .unwrap_or(false);
        let reserve_waypoint_slot = has_review_submission
            || app_state
                .active_review_session()
                .map(|session| {
                    session.waymarks.iter().any(|waymark| {
                        matches!(
                            waymark.location.mode,
                            ReviewCenterMode::SemanticDiff | ReviewCenterMode::StructuralDiff
                        ) && waymark.location.file_path == file.path
                    })
                })
                .unwrap_or(false);
        let wrap_diff_lines = app_state
            .active_review_session()
            .map(|session| session.wrap_diff_lines)
            .unwrap_or(false);
        (reserve_waypoint_slot, wrap_diff_lines)
    };
    let gutter_layout = diff_gutter_layout(file, parsed, reserve_waypoint_slot);
    let selected_anchor = selected_anchor.cloned();
    let list_state = diff_view_state.list_state.clone();
    let side_by_side_scroll_handles = SideBySideScrollHandles {
        left: diff_view_state.side_by_side_left_scroll.clone(),
        right: diff_view_state.side_by_side_right_scroll.clone(),
    };
    let prepared_file = prepared_file.cloned();
    let file_lsp_context =
        build_diff_file_lsp_context(state, file.path.as_str(), prepared_file.as_ref(), cx);
    let stack_visibility = stack_filter
        .as_ref()
        .map(|filter| stack_file_visibility(review_stack.as_ref(), filter, &file.path));
    let normal_side_by_side = (structural_side_by_side.is_none()
        && diff_layout == DiffLayout::SideBySide)
        .then(|| parsed.filter(|parsed| !parsed.hunks.is_empty() && !parsed.is_binary))
        .flatten()
        .map(|parsed| Arc::new(build_normal_side_by_side_diff_file(parsed)));
    let side_by_side_column_widths = side_by_side_column_widths_for_file(
        parsed,
        structural_side_by_side.as_deref(),
        normal_side_by_side.as_deref(),
        gutter_layout.reserve_waypoint_slot,
        wrap_diff_lines,
    );

    let items = build_diff_view_items(
        file,
        parsed,
        prepared_file.as_ref(),
        &rows,
        structural_side_by_side.as_deref(),
        normal_side_by_side.as_deref(),
        stack_visibility.as_ref(),
    );

    let review_threads = state
        .read(cx)
        .active_detail()
        .map(|detail| detail.review_threads.clone())
        .unwrap_or_default();
    reset_list_state_preserving_scroll(&list_state, items.len());
    scroll_diff_list_to_focus(
        &diff_view_state,
        &items,
        &rows,
        parsed,
        structural_side_by_side.as_deref(),
        normal_side_by_side.as_deref(),
        &review_threads,
        selected_anchor.as_ref(),
        file.path.as_str(),
    );

    if let Some(active_pr_key) = state.read(cx).active_pr_key.clone() {
        let state_for_scroll = state.clone();
        let list_state_for_scroll = list_state.clone();
        let items_for_scroll_focus = Arc::new(items.clone());
        let rows_for_scroll_focus = rows.clone();
        let parsed_for_scroll_focus = parsed.cloned();
        let file_path_for_scroll_focus = file.path.clone();
        list_state.set_scroll_handler(move |_, window, _| {
            let state = state_for_scroll.clone();
            let list_state = list_state_for_scroll.clone();
            let active_pr_key = active_pr_key.clone();
            let items_for_scroll_focus = items_for_scroll_focus.clone();
            let rows_for_scroll_focus = rows_for_scroll_focus.clone();
            let parsed_for_scroll_focus = parsed_for_scroll_focus.clone();
            let file_path_for_scroll_focus = file_path_for_scroll_focus.clone();
            window.on_next_frame(move |_, cx| {
                let scroll_top = list_state.logical_scroll_top();
                let compact = scroll_top.item_ix > 0 || scroll_top.offset_in_item > px(0.0);
                let focus = parsed_for_scroll_focus.as_ref().and_then(|parsed| {
                    diff_scroll_focus_for_item_index(
                        items_for_scroll_focus.as_ref(),
                        rows_for_scroll_focus.as_ref(),
                        parsed,
                        scroll_top.item_ix,
                        file_path_for_scroll_focus.as_str(),
                    )
                });
                state.update(cx, |state, cx| {
                    if state.active_surface != PullRequestSurface::Files
                        || state.active_pr_key.as_deref() != Some(active_pr_key.as_str())
                    {
                        return;
                    }

                    if let Some(focus) = focus {
                        if let Some(mode) = state.active_review_session().and_then(|session| {
                            matches!(
                                session.center_mode,
                                ReviewCenterMode::SemanticDiff | ReviewCenterMode::StructuralDiff
                            )
                            .then_some(session.center_mode)
                        }) {
                            state.set_review_scroll_focus(
                                mode,
                                focus.file_path,
                                focus.line,
                                focus.side,
                                focus.anchor,
                            );
                        }
                    }

                    if state.pr_header_compact != compact {
                        state.pr_header_compact = compact;
                        cx.notify();
                    }
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
                    el.child(render_stack_layer_diff_notice(&visibility))
                })
                .child(
                    render_virtualized_diff_rows(
                        &state,
                        rows,
                        gutter_layout,
                        parsed_file_index,
                        parsed_override,
                        structural_side_by_side,
                        normal_side_by_side,
                        highlighted_hunks,
                        file_lsp_context,
                        selected_anchor,
                        list_state,
                        items,
                        wrap_diff_lines,
                        side_by_side_scroll_handles,
                        side_by_side_column_widths,
                    )
                    .into_any_element(),
                ),
        )
}

pub(super) fn render_virtualized_diff_rows(
    state: &Entity<AppState>,
    rows: Arc<Vec<DiffRenderRow>>,
    gutter_layout: DiffGutterLayout,
    parsed_file_index: Option<usize>,
    parsed_file_override: Option<Arc<ParsedDiffFile>>,
    structural_side_by_side: Option<Arc<crate::difftastic::AdaptedDifftasticDiffFile>>,
    normal_side_by_side: Option<Arc<NormalSideBySideDiffFile>>,
    highlighted_hunks: Option<Arc<Vec<Vec<DiffLineHighlight>>>>,
    file_lsp_context: Option<DiffFileLspContext>,
    selected_anchor: Option<DiffAnchor>,
    list_state: ListState,
    items: Arc<Vec<DiffViewItem>>,
    wrap_diff_lines: bool,
    side_by_side_scroll_handles: SideBySideScrollHandles,
    side_by_side_column_widths: Option<SideBySideColumnWidths>,
) -> AnyElement {
    let state = state.clone();
    let has_side_by_side_rows = structural_side_by_side.is_some() || normal_side_by_side.is_some();
    let scrollbar_list_state = list_state.clone();
    let render_side_by_side_scroll_handles = side_by_side_scroll_handles.clone();
    let item_count = items.len();

    let rows = list(list_state, move |ix, _window, cx| match items[ix] {
        DiffViewItem::Gap(gap) => render_diff_gap_row(gap, gutter_layout).into_any_element(),
        DiffViewItem::StackLayerEmpty => render_diff_state_row(
            "No changed hunks in this file belong to the selected stack layer.",
        )
        .into_any_element(),
        DiffViewItem::Row(row_ix) => render_virtualized_diff_row(
            &state,
            gutter_layout,
            parsed_file_index,
            parsed_file_override.as_deref(),
            structural_side_by_side.as_deref(),
            normal_side_by_side.as_deref(),
            highlighted_hunks.as_deref(),
            file_lsp_context.as_ref(),
            &rows[row_ix],
            selected_anchor.as_ref(),
            &render_side_by_side_scroll_handles,
            side_by_side_column_widths,
            cx,
        )
        .into_any_element(),
    })
    .with_sizing_behavior(ListSizingBehavior::Auto)
    .flex_grow()
    .min_h_0();

    let use_whole_diff_horizontal_scroll = !wrap_diff_lines && !has_side_by_side_rows;
    let body = if use_whole_diff_horizontal_scroll {
        restrict_diff_scroll_to_axis(div().flex().flex_col().flex_grow().min_h_0().min_w_0())
            .id("diff-horizontal-scroll")
            .overflow_x_scroll()
            .scrollbar_width(px(DIFF_SCROLLBAR_WIDTH))
            .child(rows.min_w(px(DIFF_UNIFIED_MIN_WIDTH)))
            .into_any_element()
    } else {
        rows.into_any_element()
    };

    render_diff_scroll_body(
        body,
        &scrollbar_list_state,
        item_count,
        (!wrap_diff_lines && has_side_by_side_rows).then_some(&side_by_side_scroll_handles),
        DiffScrollbarInsets::none(),
        false,
    )
}

#[derive(Clone, Copy)]
pub(super) enum DiffViewItem {
    Row(usize),
    Gap(DiffGapSummary),
    StackLayerEmpty,
}

pub(super) fn diff_scroll_focus_for_item_index(
    items: &[DiffViewItem],
    rows: &[DiffRenderRow],
    parsed: &ParsedDiffFile,
    item_ix: usize,
    fallback_file_path: &str,
) -> Option<DiffScrollFocus> {
    let item_ix = focus_item_index_around(items.len(), item_ix, |ix| {
        diff_scroll_focus_for_item(items[ix], rows, parsed, fallback_file_path).is_some()
    })?;
    diff_scroll_focus_for_item(items[item_ix], rows, parsed, fallback_file_path)
}

pub(super) fn diff_scroll_focus_for_item(
    item: DiffViewItem,
    rows: &[DiffRenderRow],
    parsed: &ParsedDiffFile,
    fallback_file_path: &str,
) -> Option<DiffScrollFocus> {
    let DiffViewItem::Row(row_ix) = item else {
        return None;
    };
    let DiffRenderRow::Line {
        hunk_index,
        line_index,
    } = rows.get(row_ix)?
    else {
        return None;
    };
    let hunk = parsed.hunks.get(*hunk_index)?;
    let line = hunk.lines.get(*line_index)?;
    let file_path = if parsed.path.is_empty() {
        fallback_file_path
    } else {
        parsed.path.as_str()
    };
    let target = build_review_line_action_target(file_path, Some(hunk.header.as_str()), line)?;
    let line = target
        .anchor
        .line
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0);

    Some(DiffScrollFocus {
        file_path: target.anchor.file_path.clone(),
        line,
        side: target.anchor.side.clone(),
        hunk_header: Some(hunk.header.clone()),
        anchor: Some(target.anchor),
    })
}

fn scroll_diff_list_to_focus(
    view_state: &DiffFileViewState,
    items: &[DiffViewItem],
    rows: &[DiffRenderRow],
    parsed: Option<&ParsedDiffFile>,
    structural_side_by_side: Option<&crate::difftastic::AdaptedDifftasticDiffFile>,
    normal_side_by_side: Option<&NormalSideBySideDiffFile>,
    review_threads: &[PullRequestReviewThread],
    selected_anchor: Option<&DiffAnchor>,
    fallback_file_path: &str,
) {
    let Some(anchor) = selected_anchor else {
        return;
    };
    let Some(focus_key) = diff_focus_key(fallback_file_path, anchor) else {
        return;
    };
    let mut last_focus_key = view_state.last_focus_key.borrow_mut();
    if last_focus_key.as_deref() == Some(focus_key.as_str()) {
        return;
    }

    if let Some(item_ix) = find_diff_focus_item_index(
        items,
        rows,
        parsed,
        structural_side_by_side,
        normal_side_by_side,
        review_threads,
        anchor,
    ) {
        view_state.list_state.scroll_to(ListOffset {
            item_ix: item_ix.saturating_sub(6),
            offset_in_item: px(0.0),
        });
        *last_focus_key = Some(focus_key);
    }
}

pub(super) fn diff_focus_key(fallback_file_path: &str, anchor: &DiffAnchor) -> Option<String> {
    if anchor.line.is_none() && anchor.hunk_header.is_none() && anchor.thread_id.is_none() {
        return None;
    }
    Some(format!(
        "{}:{}:{}:{}:{}",
        if anchor.file_path.is_empty() {
            fallback_file_path
        } else {
            anchor.file_path.as_str()
        },
        anchor.side.as_deref().unwrap_or(""),
        anchor.line.unwrap_or_default(),
        anchor.hunk_header.as_deref().unwrap_or(""),
        anchor.thread_id.as_deref().unwrap_or("")
    ))
}

pub(super) fn find_diff_focus_item_index(
    items: &[DiffViewItem],
    rows: &[DiffRenderRow],
    parsed: Option<&ParsedDiffFile>,
    structural_side_by_side: Option<&crate::difftastic::AdaptedDifftasticDiffFile>,
    normal_side_by_side: Option<&NormalSideBySideDiffFile>,
    review_threads: &[PullRequestReviewThread],
    anchor: &DiffAnchor,
) -> Option<usize> {
    if let Some(thread_id) = anchor.thread_id.as_deref() {
        if let Some(row_ix) = rows.iter().position(|row| match row {
            DiffRenderRow::FileCommentThread { thread_index }
            | DiffRenderRow::InlineThread { thread_index }
            | DiffRenderRow::OutdatedThread { thread_index } => review_threads
                .get(*thread_index)
                .map(|thread| thread.id == thread_id)
                .unwrap_or(false),
            _ => false,
        }) {
            if let Some(item_ix) = items.iter().position(
                |item| matches!(item, DiffViewItem::Row(item_row_ix) if *item_row_ix == row_ix),
            ) {
                return Some(item_ix);
            }
        }
    }

    let parsed = parsed?;
    let direct_row_ix = rows.iter().position(|row| match row {
        DiffRenderRow::HunkHeader { hunk_index } => {
            anchor.line.is_none()
                && anchor.hunk_header.as_deref().is_some_and(|header| {
                    parsed
                        .hunks
                        .get(*hunk_index)
                        .map(|hunk| hunk.header == header)
                        .unwrap_or(false)
                })
        }
        DiffRenderRow::Line {
            hunk_index,
            line_index,
        } => parsed
            .hunks
            .get(*hunk_index)
            .and_then(|hunk| hunk.lines.get(*line_index))
            .map(|line| line_matches_diff_anchor(line, Some(anchor)))
            .unwrap_or(false),
        _ => false,
    });
    if let Some(row_ix) = direct_row_ix {
        if let Some(item_ix) = items.iter().position(
            |item| matches!(item, DiffViewItem::Row(item_row_ix) if *item_row_ix == row_ix),
        ) {
            return Some(item_ix);
        }
    }

    let row_ix = {
        let (hunk_index, line_index) = find_parsed_line_index_for_anchor(parsed, anchor)?;
        side_by_side_primary_row_index(
            rows,
            hunk_index,
            line_index,
            structural_side_by_side,
            normal_side_by_side,
        )
    }?;

    items
        .iter()
        .position(|item| matches!(item, DiffViewItem::Row(item_row_ix) if *item_row_ix == row_ix))
}

fn find_parsed_line_index_for_anchor(
    parsed: &ParsedDiffFile,
    anchor: &DiffAnchor,
) -> Option<(usize, usize)> {
    for (hunk_index, hunk) in parsed.hunks.iter().enumerate() {
        for (line_index, line) in hunk.lines.iter().enumerate() {
            if line_matches_diff_anchor(line, Some(anchor)) {
                return Some((hunk_index, line_index));
            }
        }
    }

    None
}

fn side_by_side_primary_row_index(
    rows: &[DiffRenderRow],
    hunk_index: usize,
    line_index: usize,
    structural_side_by_side: Option<&crate::difftastic::AdaptedDifftasticDiffFile>,
    normal_side_by_side: Option<&NormalSideBySideDiffFile>,
) -> Option<usize> {
    let target_row_index = structural_side_by_side
        .and_then(|side_by_side| {
            side_by_side
                .side_by_side_line_map
                .get(hunk_index)
                .and_then(|lines| lines.get(line_index))
                .and_then(|entry| *entry)
        })
        .map(|entry| entry.row_index)
        .or_else(|| {
            normal_side_by_side
                .and_then(|side_by_side| {
                    side_by_side
                        .line_map
                        .get(hunk_index)
                        .and_then(|lines| lines.get(line_index))
                        .and_then(|entry| *entry)
                })
                .map(|entry| entry.row_index)
        })?;

    rows.iter().position(|row| {
        let DiffRenderRow::Line {
            hunk_index: row_hunk_index,
            line_index: row_line_index,
        } = row
        else {
            return false;
        };

        if *row_hunk_index != hunk_index {
            return false;
        }

        structural_side_by_side
            .and_then(|side_by_side| {
                side_by_side
                    .side_by_side_line_map
                    .get(hunk_index)
                    .and_then(|lines| lines.get(*row_line_index))
                    .and_then(|entry| *entry)
            })
            .map(|entry| entry.primary && entry.row_index == target_row_index)
            .or_else(|| {
                normal_side_by_side
                    .and_then(|side_by_side| {
                        side_by_side
                            .line_map
                            .get(hunk_index)
                            .and_then(|lines| lines.get(*row_line_index))
                            .and_then(|entry| *entry)
                    })
                    .map(|entry| entry.primary && entry.row_index == target_row_index)
            })
            .unwrap_or(false)
    })
}

#[derive(Clone)]
pub(super) struct StackFileVisibility {
    layer_id: Option<String>,
    layer_title: String,
    layer_rationale: String,
    layer_warnings: Vec<crate::stacks::model::StackWarning>,
    ai_assisted: bool,
    visible_hunk_indices: Option<BTreeSet<usize>>,
    file_has_visible_atoms: bool,
}

pub(super) fn build_layer_diff_filter(
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

pub(super) fn stack_file_visibility(
    stack: &ReviewStack,
    filter: &LayerDiffFilter,
    file_path: &str,
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

    let file_has_visible_atoms = visible_atom_count > 0;
    let visible_hunk_indices = if show_whole_file {
        None
    } else {
        Some(visible_hunks)
    };

    StackFileVisibility {
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
        ai_assisted: stack.source == crate::stacks::model::StackSource::VirtualAi,
        visible_hunk_indices,
        file_has_visible_atoms,
    }
}

#[derive(Clone, Copy)]
pub(super) enum DiffGapPosition {
    Start,
    Between,
    End,
}

#[derive(Clone, Copy)]
pub(super) struct DiffGapSummary {
    pub(super) position: DiffGapPosition,
    pub(super) hidden_count: usize,
    pub(super) start_line: Option<usize>,
    pub(super) end_line: Option<usize>,
}

#[derive(Clone)]
pub(super) struct DiffFileLspContext {
    pub(super) state: Entity<AppState>,
    pub(super) detail_key: String,
    pub(super) lsp_session_manager: Arc<lsp::LspSessionManager>,
    pub(super) repo_root: PathBuf,
    pub(super) file_path: String,
    pub(super) reference: String,
    pub(super) document_text: Arc<str>,
}

pub(super) fn build_diff_file_lsp_context(
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
pub(super) struct DiffLineLspContext {
    pub(super) file: DiffFileLspContext,
    pub(super) line_number: usize,
}

#[derive(Clone)]
pub(super) struct DiffLineLspQuery {
    pub(super) state: Entity<AppState>,
    pub(super) detail_key: String,
    pub(super) lsp_session_manager: Arc<lsp::LspSessionManager>,
    pub(super) repo_root: PathBuf,
    pub(super) query_key: String,
    pub(super) token_label: String,
    pub(super) request: lsp::LspTextDocumentRequest,
}

pub(super) fn build_diff_line_lsp_context(
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
    pub(super) fn query_for_index(
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

pub(super) fn request_diff_line_lsp_details(
    query: DiffLineLspQuery,
    window: &mut Window,
    cx: &mut App,
) {
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

pub(super) fn navigate_to_diff_lsp_definition(
    query: DiffLineLspQuery,
    window: &mut Window,
    cx: &mut App,
) {
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

pub(super) fn build_diff_view_items(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    rows: &[DiffRenderRow],
    structural_side_by_side: Option<&crate::difftastic::AdaptedDifftasticDiffFile>,
    normal_side_by_side: Option<&NormalSideBySideDiffFile>,
    stack_visibility: Option<&StackFileVisibility>,
) -> Vec<DiffViewItem> {
    let mut items = Vec::with_capacity(rows.len() + 4);
    let mut last_hunk_index = None;
    let last_hunk_row_index = rows.iter().rposition(|row| {
        matches!(
            row,
            DiffRenderRow::HunkHeader { .. } | DiffRenderRow::Line { .. }
        )
    });
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

        let non_primary_side_by_side_line = match row {
            DiffRenderRow::Line {
                hunk_index,
                line_index,
            } => {
                let structural_non_primary = structural_side_by_side
                    .and_then(|side_by_side| {
                        side_by_side
                            .side_by_side_line_map
                            .get(*hunk_index)
                            .and_then(|lines| lines.get(*line_index))
                            .and_then(|entry| *entry)
                    })
                    .map(|entry| !entry.primary);
                let normal_non_primary = normal_side_by_side
                    .and_then(|side_by_side| {
                        side_by_side
                            .line_map
                            .get(*hunk_index)
                            .and_then(|lines| lines.get(*line_index))
                            .and_then(|entry| *entry)
                    })
                    .map(|entry| !entry.primary);

                structural_non_primary
                    .or(normal_non_primary)
                    .unwrap_or(false)
            }
            _ => false,
        };

        let should_skip = non_primary_side_by_side_line
            || !current_hunk_visible
                && matches!(
                    row,
                    DiffRenderRow::HunkHeader { .. }
                        | DiffRenderRow::Line { .. }
                        | DiffRenderRow::InlineThread { .. }
                );

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

pub(super) fn render_diff_gap_row(
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
                                .border_color(transparent())
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

pub(super) fn render_stack_layer_diff_notice(visibility: &StackFileVisibility) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(8.0))
        .bg(diff_annotation_bg())
        .border_b(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .items_start()
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
}

pub(super) fn render_diff_gap_label(summary: DiffGapSummary) -> String {
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

pub(super) fn render_semantic_section_header(
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

pub(super) fn prepare_diff_view_state(
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

pub(super) fn prepare_tour_diff_view_state(
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

pub(super) fn prepare_structural_diff_view_state(
    app_state: &AppState,
    detail: &PullRequestDetail,
    file_path: &str,
    request_key: &str,
    structural: &crate::difftastic::AdaptedDifftasticDiffFile,
) -> DiffFileViewState {
    let revision = request_key.to_string();
    let state_key =
        build_diff_view_state_key(app_state.active_pr_key.as_deref(), "structural", file_path);
    let mut diff_view_states = app_state.diff_view_states.borrow_mut();
    let entry = diff_view_states.entry(state_key).or_insert_with(|| {
        DiffFileViewState::new(
            Arc::new(build_diff_render_rows_for_parsed_file(
                detail,
                file_path,
                Some(&structural.parsed_file),
            )),
            revision.clone(),
            None,
            Some(build_adapted_diff_highlights(structural)),
        )
    });

    if entry.revision != revision || entry.highlighted_hunks.is_none() {
        entry.rows = Arc::new(build_diff_render_rows_for_parsed_file(
            detail,
            file_path,
            Some(&structural.parsed_file),
        ));
        entry.revision = revision;
        entry.parsed_file_index = None;
        entry.highlighted_hunks = Some(build_adapted_diff_highlights(structural));
        entry.list_state.reset(0);
    }

    entry.clone()
}

pub(super) fn render_virtualized_diff_row(
    state: &Entity<AppState>,
    gutter_layout: DiffGutterLayout,
    parsed_file_index: Option<usize>,
    parsed_file_override: Option<&ParsedDiffFile>,
    structural_side_by_side: Option<&crate::difftastic::AdaptedDifftasticDiffFile>,
    normal_side_by_side: Option<&NormalSideBySideDiffFile>,
    highlighted_hunks: Option<&Vec<Vec<DiffLineHighlight>>>,
    file_lsp_context: Option<&DiffFileLspContext>,
    row: &DiffRenderRow,
    selected_anchor: Option<&DiffAnchor>,
    side_by_side_scroll_handles: &SideBySideScrollHandles,
    side_by_side_column_widths: Option<SideBySideColumnWidths>,
    cx: &App,
) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();
    let thread_ui = review_thread_ui_state(&s);
    let parsed_file = parsed_file_override.or_else(|| {
        parsed_file_index.and_then(|ix| detail.and_then(|detail| detail.parsed_diff.get(ix)))
    });

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
                        cx,
                        thread_ui.clone(),
                    ))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::InlineThread { thread_index } => detail
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
                        cx,
                        thread_ui.clone(),
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
                        cx,
                        thread_ui.clone(),
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
                    if let Some(side_by_side) = structural_side_by_side {
                        let line_map = side_by_side
                            .side_by_side_line_map
                            .get(*hunk_index)
                            .and_then(|lines| lines.get(*line_index))
                            .and_then(|entry| *entry)?;
                        if !line_map.primary {
                            return Some(div().into_any_element());
                        }

                        return side_by_side
                            .side_by_side_hunks
                            .get(*hunk_index)
                            .and_then(|side_by_side_hunk| {
                                side_by_side_hunk.rows.get(line_map.row_index)
                            })
                            .map(|side_by_side_row| {
                                render_structural_side_by_side_diff_row(
                                    state,
                                    gutter_layout,
                                    detail,
                                    parsed,
                                    path,
                                    *hunk_index,
                                    hunk.header.as_str(),
                                    line_map.row_index,
                                    side_by_side_row,
                                    selected_anchor,
                                    file_lsp_context,
                                    side_by_side_scroll_handles,
                                    side_by_side_column_widths.unwrap_or_else(|| {
                                        structural_side_by_side_column_widths(
                                            side_by_side,
                                            gutter_layout.reserve_waypoint_slot,
                                            false,
                                        )
                                    }),
                                    cx,
                                )
                                .into_any_element()
                            });
                    }

                    if let Some(side_by_side) = normal_side_by_side {
                        let line_map = side_by_side
                            .line_map
                            .get(*hunk_index)
                            .and_then(|lines| lines.get(*line_index))
                            .and_then(|entry| *entry)?;
                        if !line_map.primary {
                            return Some(div().into_any_element());
                        }

                        return side_by_side
                            .hunks
                            .get(*hunk_index)
                            .and_then(|side_by_side_hunk| {
                                side_by_side_hunk.rows.get(line_map.row_index)
                            })
                            .map(|side_by_side_row| {
                                render_normal_side_by_side_diff_row(
                                    state,
                                    gutter_layout.reserve_waypoint_slot,
                                    detail,
                                    parsed,
                                    path,
                                    *hunk_index,
                                    hunk.header.as_str(),
                                    line_map.row_index,
                                    hunk,
                                    highlighted_hunks.and_then(|hunks| hunks.get(*hunk_index)),
                                    side_by_side_row,
                                    selected_anchor,
                                    file_lsp_context,
                                    side_by_side_scroll_handles,
                                    side_by_side_column_widths.unwrap_or_else(|| {
                                        normal_side_by_side_column_widths(
                                            parsed,
                                            side_by_side,
                                            gutter_layout.reserve_waypoint_slot,
                                            false,
                                        )
                                    }),
                                    cx,
                                )
                                .into_any_element()
                            });
                    }

                    hunk.lines.get(*line_index).map(|line| {
                        let hunk_header = hunk.header.as_str();
                        let highlight = highlighted_hunks
                            .and_then(|hunks| hunks.get(*hunk_index))
                            .and_then(|lines| lines.get(*line_index))
                            .cloned()
                            .unwrap_or_default();
                        let line_lsp_context = build_diff_line_lsp_context(file_lsp_context, line);
                        let temp_source_target = detail.and_then(|detail| {
                            temp_source_target_for_diff_line(detail, parsed, line)
                        });
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
                            temp_source_target,
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
