use super::*;

pub(super) fn render_diff_section_header(label: &str, count: usize) -> impl IntoElement {
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

pub(super) fn render_diff_state_row(message: impl Into<String>) -> impl IntoElement {
    let message = message.into();
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
                .child(message),
        )
}

pub(super) fn render_raw_diff_fallback(raw_diff: &str) -> impl IntoElement {
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
            restrict_diff_scroll_to_axis(div())
                .id("raw-diff-horizontal-scroll")
                .overflow_x_scroll()
                .scrollbar_width(px(DIFF_SCROLLBAR_WIDTH))
                .child(
                    div()
                        .min_w(px(DIFF_UNIFIED_MIN_WIDTH))
                        .child(render_highlighted_code_content("diff.patch", raw_diff)),
                )
                .into_any_element()
        })
}

pub(super) fn render_change_type_chip(change_type: &str) -> impl IntoElement {
    let (bg, fg, _border) = match change_type {
        "ADDED" => (success_muted(), success(), diff_add_border()),
        "DELETED" => (danger_muted(), danger(), diff_remove_border()),
        "RENAMED" | "COPIED" => (accent_muted(), accent(), accent()),
        _ => (bg_subtle(), fg_muted(), border_muted()),
    };

    metric_pill(label_for_change_type(change_type).to_string(), fg, bg)
}

pub(super) fn render_file_stat_bar(additions: i64, deletions: i64) -> impl IntoElement {
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

pub(super) fn lerp_px(from: f32, to: f32, progress: f32) -> Pixels {
    px(from + (to - from) * progress)
}

pub(super) fn lerp_rgba(from: Rgba, to: Rgba, progress: f32) -> Rgba {
    Rgba {
        r: from.r + (to.r - from.r) * progress,
        g: from.g + (to.g - from.g) * progress,
        b: from.b + (to.b - from.b) * progress,
        a: from.a + (to.a - from.a) * progress,
    }
}

pub(super) fn render_hunk(
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    hunk: &ParsedDiffHunk,
    line_threads: &[&PullRequestReviewThread],
    selected_anchor: Option<&DiffAnchor>,
    unread_comment_ids: &BTreeSet<String>,
    state: &Entity<AppState>,
    cx: &App,
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
                        cx,
                    )
                })),
        )
}

pub(super) fn render_diff_line_with_threads(
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    line: &ParsedDiffLine,
    threads: &[&PullRequestReviewThread],
    selected_anchor: Option<&DiffAnchor>,
    unread_comment_ids: &BTreeSet<String>,
    state: &Entity<AppState>,
    cx: &App,
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
            None,
            false,
            false,
            false,
            false,
        ))
        .when(!threads.is_empty(), |el| {
            el.child(
                div()
                    .px(px(16.0))
                    .py(px(8.0))
                    .border_b(px(1.0))
                    .border_color(diff_annotation_border())
                    .bg(diff_annotation_bg())
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(threads.iter().map(|thread| {
                        render_review_thread(
                            thread,
                            selected_anchor,
                            unread_comment_ids,
                            state,
                            cx,
                            ReviewThreadUiState::default(),
                        )
                    })),
            )
        })
}

pub(super) fn render_diff_line(
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    line: &ParsedDiffLine,
    syntax_spans: Option<&[SyntaxSpan]>,
    emphasis_ranges: Option<&[DiffInlineRange]>,
    selected_anchor: Option<&DiffAnchor>,
    lsp_context: Option<&DiffLineLspContext>,
    line_action: Option<(Entity<AppState>, ReviewLineActionTarget)>,
    source_action: Option<(Entity<AppState>, TempSourceTarget)>,
    has_waypoint: bool,
    force_marker_visible: bool,
    range_selected: bool,
    wrap_diff_lines: bool,
) -> impl IntoElement {
    let is_selected = line_matches_diff_anchor(line, selected_anchor) || range_selected;
    let gutter_line_action = line_action.clone();
    let has_gutter_line_action = gutter_line_action.is_some();
    let source_slot_action = source_action.clone();
    let hover_source_action = source_action.clone();

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
    let marker_visible = is_selected || force_marker_visible;
    let number_color = if is_selected {
        fg_default()
    } else {
        fg_subtle()
    };

    div()
        .flex()
        .w_full()
        .min_w(px(diff_line_min_width(
            gutter_layout,
            line.content.as_str(),
            wrap_diff_lines,
        )))
        .items_start()
        .min_h(diff_row_height_px())
        .bg(row_bg)
        .font_family(mono_font_family())
        .text_size(diff_code_font_size_px())
        .line_height(diff_code_line_height_px())
        .font_weight(FontWeight::MEDIUM)
        .text_color(if marker_visible {
            marker_color
        } else {
            transparent()
        })
        .hover(move |style| style.bg(diff_line_hover_bg()).text_color(marker_color))
        .when(is_selected, |el| {
            el.border_l(px(2.0)).border_color(diff_selected_edge())
        })
        .when_some(hover_source_action, |el, (state, target)| {
            el.on_mouse_move(move |_, _, cx| {
                state.update(cx, |state, cx| {
                    state.hovered_temp_source_target = Some(target.clone());
                    cx.notify();
                });
            })
        })
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .w(px(gutter_layout.gutter_width()))
                .min_h(diff_row_height_px())
                .bg(gutter_bg)
                .border_r(px(1.0))
                .border_color(diff_gutter_separator())
                .when(gutter_layout.reserve_source_slot, |el| {
                    el.child(
                        div()
                            .w(diff_source_slot_width_px())
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(source_slot_action, |slot, (state, target)| {
                                let tooltip_label = format!("Open {} source", target.side.label());
                                slot.child(
                                    div()
                                        .id((
                                            ElementId::named_usize(
                                                "diff-open-source",
                                                line.right_line_number
                                                    .or(line.left_line_number)
                                                    .unwrap_or_default()
                                                    as usize,
                                            ),
                                            SharedString::from(format!(
                                                "{}:{}",
                                                file_path,
                                                target.side.diff_side()
                                            )),
                                        ))
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .rounded(radius_sm())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(diff_editor_surface()))
                                        .tooltip(move |_, cx| {
                                            build_text_tooltip(
                                                SharedString::from(tooltip_label.clone()),
                                                cx,
                                            )
                                        })
                                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                            cx.stop_propagation();
                                            open_temp_source_window_for_diff_target(
                                                &state,
                                                target.clone(),
                                                window,
                                                cx,
                                            );
                                        })
                                        .child(render_diff_open_source_icon()),
                                )
                            }),
                    )
                })
                .when(gutter_layout.reserve_waypoint_slot, |el| {
                    el.child(
                        div()
                            .w(diff_waypoint_slot_width_px())
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(gutter_line_action, |slot, (state, target)| {
                                let move_state = state.clone();
                                let move_target = target.clone();
                                let up_state = state.clone();
                                let up_target = target.clone();
                                slot.child(
                                    div()
                                        .id((
                                            ElementId::named_usize(
                                                "diff-comment",
                                                line.right_line_number
                                                    .or(line.left_line_number)
                                                    .unwrap_or_default()
                                                    as usize,
                                            ),
                                            SharedString::from(file_path.to_string()),
                                        ))
                                        .w_full()
                                        .h_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(diff_editor_surface()))
                                        .tooltip(|_, cx| {
                                            build_static_tooltip("click or drag to comment", cx)
                                        })
                                        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                                            cx.stop_propagation();
                                            if event.modifiers.shift {
                                                let target = review_line_action_target_with_range(
                                                    &state,
                                                    target.clone(),
                                                    true,
                                                    cx,
                                                );
                                                open_review_line_action(
                                                    &state,
                                                    target,
                                                    event.position,
                                                    cx,
                                                );
                                            } else {
                                                begin_review_line_drag(&state, target.clone(), cx);
                                            }
                                        })
                                        .on_mouse_move(move |_, _, cx| {
                                            update_review_line_drag(
                                                &move_state,
                                                move_target.clone(),
                                                cx,
                                            );
                                        })
                                        .on_mouse_up(MouseButton::Left, move |event, _, cx| {
                                            cx.stop_propagation();
                                            finish_review_line_drag(
                                                &up_state,
                                                up_target.clone(),
                                                event.position,
                                                cx,
                                            );
                                        })
                                        .child(if has_waypoint {
                                            render_diff_waypoint_icon().into_any_element()
                                        } else {
                                            div().into_any_element()
                                        }),
                                )
                            })
                            .when(!has_gutter_line_action && has_waypoint, |slot| {
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
                            .w(px(diff_line_number_column_width()))
                            .px(px(DIFF_LINE_NUMBER_CELL_PADDING_X))
                            .flex()
                            .justify_end()
                            .text_size(diff_line_number_font_size_px())
                            .line_height(diff_code_line_height_px())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(number_color)
                            .child(left_num),
                    )
                })
                .when(gutter_layout.show_right_numbers, |el| {
                    el.child(
                        div()
                            .w(px(diff_line_number_column_width()))
                            .px(px(DIFF_LINE_NUMBER_CELL_PADDING_X))
                            .flex()
                            .justify_end()
                            .text_size(diff_line_number_font_size_px())
                            .line_height(diff_code_line_height_px())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(number_color)
                            .child(right_num),
                    )
                }),
        )
        .child(
            div()
                .w(px(diff_marker_column_width()))
                .flex_shrink_0()
                .min_h(diff_row_height_px())
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
            wrap_diff_lines,
        ))
}

pub(super) fn render_syntax_content(
    file_path: &str,
    line: &ParsedDiffLine,
    syntax_spans: Option<&[SyntaxSpan]>,
    emphasis_ranges: Option<&[DiffInlineRange]>,
    fallback_color: Rgba,
    lsp_context: Option<&DiffLineLspContext>,
    line_action: Option<(Entity<AppState>, ReviewLineActionTarget)>,
    wrap_diff_lines: bool,
) -> Div {
    let content = line.content.as_str();
    let content_div = div()
        .flex_grow()
        .min_w(px(if wrap_diff_lines {
            0.0
        } else {
            diff_code_text_width(content)
        }))
        .px(px(8.0))
        .py(px(1.0))
        .text_size(diff_code_font_size_px())
        .line_height(diff_code_line_height_px())
        .font_weight(FontWeight::MEDIUM)
        .font_family(mono_font_family())
        .when(wrap_diff_lines, |el| el.whitespace_normal())
        .when(!wrap_diff_lines, |el| el.whitespace_nowrap());

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
    .or_else(|| code_text_runs(content, spans, fallback_color));

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
        .require_platform_modifier_for_click()
        .on_hover(move |index, _event, window, cx| {
            let Some(index) = index else {
                return;
            };
            let Some(query) = hover_context.query_for_index(index, hover_tokens.as_ref()) else {
                return;
            };
            request_diff_line_lsp_details(query, window, cx);
        })
        .tooltip_with_key(move |index, window, cx| {
            let query = tooltip_context.query_for_index(index, tooltip_tokens.as_ref())?;
            request_diff_line_lsp_details(query.clone(), window, cx);
            Some((
                query.query_key.clone(),
                build_lsp_hover_tooltip_view(
                    query.state.clone(),
                    query.detail_key.clone(),
                    query.query_key.clone(),
                    query.token_label.clone(),
                    cx,
                ),
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

    if spans.is_empty() && rendered_runs.is_none() {
        let mut selectable = SelectableText::new(selection_id, content.to_string());
        if let Some((state, target)) = line_action {
            selectable = selectable.on_click_unmatched(move |window, cx| {
                open_review_line_action(&state, target.clone(), window.mouse_position(), cx);
            });
        }
        return content_div.text_color(fallback_color).child(selectable);
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

pub(super) const DIFF_ROW_HEIGHT: f32 = 25.0;
pub(super) const DIFF_CODE_FONT_SIZE: f32 = 14.0;
pub(super) const DIFF_CODE_LINE_HEIGHT: f32 = 21.0;
pub(super) const DIFF_LINE_NUMBER_FONT_SIZE: f32 = 12.5;
pub(super) const DIFF_LINE_NUMBER_COLUMN_WIDTH: f32 = 40.0;
pub(super) const DIFF_LINE_NUMBER_CELL_PADDING_X: f32 = 8.0;
pub(super) const DIFF_MARKER_COLUMN_WIDTH: f32 = 16.0;
pub(super) const DIFF_SOURCE_SLOT_WIDTH: f32 = DIFF_ROW_HEIGHT;
pub(super) const DIFF_WAYPOINT_SLOT_WIDTH: f32 = DIFF_ROW_HEIGHT;
pub(super) const DIFF_UNIFIED_MIN_WIDTH: f32 = 720.0;
pub(super) const DIFF_SIDE_BY_SIDE_MIN_WIDTH: f32 = 960.0;
pub(super) const DIFF_CODE_CHAR_WIDTH: f32 = 8.4;
pub(super) const DIFF_CODE_MIN_TEXT_WIDTH: f32 = 16.0;
pub(super) const DIFF_CODE_MAX_TEXT_WIDTH: f32 = 16000.0;
pub(super) const REVIEW_THREAD_MAX_WIDTH: f32 = 1040.0;

pub(super) fn diff_row_height_px() -> Pixels {
    code_row_height(DIFF_ROW_HEIGHT)
}

pub(super) fn diff_code_font_size_px() -> Pixels {
    code_text_size(DIFF_CODE_FONT_SIZE)
}

pub(super) fn diff_code_line_height_px() -> Pixels {
    code_line_height(DIFF_CODE_LINE_HEIGHT)
}

pub(super) fn diff_line_number_font_size_px() -> Pixels {
    code_text_size(DIFF_LINE_NUMBER_FONT_SIZE)
}

pub(super) fn diff_line_number_column_width() -> f32 {
    code_measure_width(DIFF_LINE_NUMBER_COLUMN_WIDTH)
}

pub(super) fn diff_marker_column_width() -> f32 {
    code_measure_width(DIFF_MARKER_COLUMN_WIDTH)
}

pub(super) fn diff_source_slot_width_px() -> Pixels {
    code_row_height(DIFF_SOURCE_SLOT_WIDTH)
}

pub(super) fn diff_waypoint_slot_width_px() -> Pixels {
    code_row_height(DIFF_WAYPOINT_SLOT_WIDTH)
}

pub(super) fn diff_source_slot_width() -> f32 {
    f32::from(diff_source_slot_width_px())
}

pub(super) fn diff_waypoint_slot_width() -> f32 {
    f32::from(diff_waypoint_slot_width_px())
}

#[derive(Clone, Copy)]
pub(super) struct DiffGutterLayout {
    pub(super) show_left_numbers: bool,
    pub(super) show_right_numbers: bool,
    pub(super) reserve_waypoint_slot: bool,
    pub(super) reserve_source_slot: bool,
}

impl DiffGutterLayout {
    pub(super) fn gutter_width(self) -> f32 {
        let column_count = self.show_left_numbers as u8 + self.show_right_numbers as u8;
        diff_line_number_column_width() * f32::from(column_count.max(1))
            + if self.reserve_source_slot {
                diff_source_slot_width()
            } else {
                0.0
            }
            + if self.reserve_waypoint_slot {
                diff_waypoint_slot_width()
            } else {
                0.0
            }
    }
}

pub(super) fn diff_code_text_width(content: &str) -> f32 {
    let chars = content.chars().count().max(1) as f32;
    (chars * code_measure_width(DIFF_CODE_CHAR_WIDTH))
        .clamp(DIFF_CODE_MIN_TEXT_WIDTH, DIFF_CODE_MAX_TEXT_WIDTH)
}

pub(super) fn diff_line_min_width(
    gutter_layout: DiffGutterLayout,
    content: &str,
    wrap_diff_lines: bool,
) -> f32 {
    if wrap_diff_lines {
        0.0
    } else {
        gutter_layout.gutter_width()
            + diff_marker_column_width()
            + 16.0
            + diff_code_text_width(content)
    }
}

pub(super) fn side_by_side_cell_min_width(
    gutter_layout: DiffGutterLayout,
    content: Option<&str>,
    wrap_diff_lines: bool,
) -> f32 {
    if wrap_diff_lines {
        0.0
    } else {
        let content_width = content
            .map(diff_code_text_width)
            .unwrap_or(DIFF_CODE_MIN_TEXT_WIDTH);
        (gutter_layout.gutter_width() + diff_marker_column_width() + 16.0 + content_width)
            .max(DIFF_SIDE_BY_SIDE_MIN_WIDTH / 2.0)
    }
}

pub(super) fn diff_gutter_layout(
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
                reserve_source_slot: true,
            };
        }
    }

    match file.change_type.as_str() {
        "ADDED" => DiffGutterLayout {
            show_left_numbers: false,
            show_right_numbers: true,
            reserve_waypoint_slot,
            reserve_source_slot: true,
        },
        "DELETED" => DiffGutterLayout {
            show_left_numbers: true,
            show_right_numbers: false,
            reserve_waypoint_slot,
            reserve_source_slot: true,
        },
        _ => DiffGutterLayout {
            show_left_numbers: true,
            show_right_numbers: true,
            reserve_waypoint_slot,
            reserve_source_slot: true,
        },
    }
}

pub(super) fn diff_gutter_layout_from_parsed(parsed_file: &ParsedDiffFile) -> DiffGutterLayout {
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
        reserve_source_slot: false,
    }
}

pub(super) fn inline_emphasis_background(kind: DiffLineKind) -> Option<Hsla> {
    match kind {
        DiffLineKind::Addition => Some(diff_add_emphasis_bg().into()),
        DiffLineKind::Deletion => Some(diff_remove_emphasis_bg().into()),
        DiffLineKind::Context | DiffLineKind::Meta => None,
    }
}

pub(super) fn decorated_diff_text_runs(
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
                font: mono_code_font(),
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
            font: mono_code_font(),
            color: current_color,
            background_color: current_emphasis.then_some(emphasis_background),
            underline: None,
            strikethrough: None,
        });
    }

    (!runs.is_empty()).then_some(runs)
}

pub(super) fn build_diff_highlights(
    parsed_file: &ParsedDiffFile,
) -> Arc<Vec<Vec<DiffLineHighlight>>> {
    Arc::new(
        parsed_file
            .hunks
            .iter()
            .map(|hunk| {
                let syntax_lines = syntax::highlight_lines(
                    parsed_file.path.as_str(),
                    hunk.lines.iter().map(|line| line.content.as_str()),
                );
                let emphasis_lines = if DIFF_INLINE_EMPHASIS_ENABLED {
                    build_hunk_inline_emphasis(hunk)
                } else {
                    vec![Vec::new(); hunk.lines.len()]
                };

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

pub(super) fn render_hunk_header(
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
