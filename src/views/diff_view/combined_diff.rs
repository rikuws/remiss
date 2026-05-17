use super::*;

#[derive(Clone)]
pub(super) struct CombinedDiffFileContext {
    pub(super) file: PullRequestFile,
    pub(super) header: ReviewFileHeaderProps,
    pub(super) collapsed: bool,
    pub(super) reviewed: bool,
    pub(super) parsed: Option<Arc<ParsedDiffFile>>,
    pub(super) parsed_override: Option<Arc<ParsedDiffFile>>,
    pub(super) structural_side_by_side: Option<Arc<crate::difftastic::AdaptedDifftasticDiffFile>>,
    pub(super) normal_side_by_side: Option<Arc<NormalSideBySideDiffFile>>,
    pub(super) side_by_side_column_widths: Option<SideBySideColumnWidths>,
    pub(super) rows: Arc<Vec<DiffRenderRow>>,
    pub(super) parsed_file_index: Option<usize>,
    pub(super) highlighted_hunks: Option<Arc<Vec<Vec<DiffLineHighlight>>>>,
    pub(super) gutter_layout: DiffGutterLayout,
    pub(super) file_lsp_context: Option<DiffFileLspContext>,
    pub(super) selected_anchor: Option<DiffAnchor>,
    pub(super) stack_visibility: Option<StackFileVisibility>,
    pub(super) items: Arc<Vec<DiffViewItem>>,
    pub(super) state_message: Option<String>,
}

#[derive(Clone)]
pub(super) enum CombinedDiffViewItem {
    Header(usize),
    StackNotice(usize),
    State {
        file_index: usize,
        message: String,
    },
    Row {
        file_index: usize,
        item: DiffViewItem,
    },
    Footer,
}

#[derive(Clone)]
pub(super) struct CombinedDiffFloatingHeader {
    header: ReviewFileHeaderProps,
    collapsed: bool,
    reviewed: bool,
    collapse_scroll: Option<DiffFileCollapseScrollAdjustment>,
}

#[derive(Clone)]
pub(super) struct DiffFileCollapseScrollAdjustment {
    pub(super) list_state: ListState,
    pub(super) header_item_ix: usize,
    pub(super) expanded_extra_item_count: usize,
}

impl DiffFileCollapseScrollAdjustment {
    pub(super) fn for_combined_file(
        list_state: &ListState,
        header_item_ix: usize,
        context: &CombinedDiffFileContext,
    ) -> Self {
        Self {
            list_state: list_state.clone(),
            header_item_ix,
            expanded_extra_item_count: combined_diff_file_expanded_extra_item_count(context),
        }
    }

    pub(super) fn apply_for_toggle(&self, currently_collapsed: bool) {
        if self.expanded_extra_item_count == 0 {
            return;
        }

        let range_start = self.header_item_ix.saturating_add(1);
        let item_count = self.list_state.item_count();
        if range_start > item_count {
            return;
        }

        if currently_collapsed {
            self.list_state
                .splice(range_start..range_start, self.expanded_extra_item_count);
        } else {
            let range_end = range_start.saturating_add(self.expanded_extra_item_count);
            if range_end > item_count {
                return;
            }

            let scroll_top = self.list_state.logical_scroll_top();
            let scroll_was_inside_collapsed_body =
                (range_start..range_end).contains(&scroll_top.item_ix);

            self.list_state.splice(range_start..range_end, 0);

            if scroll_was_inside_collapsed_body {
                self.list_state.scroll_to(ListOffset {
                    item_ix: self.header_item_ix,
                    offset_in_item: px(0.0),
                });
            }
        }
    }
}

pub(super) fn render_combined_diff_files(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
    review_stack: Arc<ReviewStack>,
    stack_filter: Option<LayerDiffFilter>,
    center_mode: ReviewCenterMode,
    diff_layout: DiffLayout,
    cx: &App,
) -> AnyElement {
    let visible_paths = stack_filter
        .as_ref()
        .map(|filter| stack_file_paths_for_filter(review_stack.as_ref(), filter));
    let tree_rows = prepare_review_file_tree_rows(app_state, detail, visible_paths.as_ref());
    let ordered_files =
        ordered_review_files_from_tree_rows(detail, tree_rows.as_ref(), visible_paths.as_ref());
    let contexts = ordered_files
        .into_iter()
        .map(|file| {
            prepare_combined_diff_file_context(
                state,
                app_state,
                detail,
                file,
                selected_path,
                selected_anchor,
                review_stack.as_ref(),
                stack_filter.as_ref(),
                center_mode,
                diff_layout,
                cx,
            )
        })
        .collect::<Vec<_>>();

    if contexts.is_empty() {
        return panel_state_text("No files returned for this pull request.").into_any_element();
    }

    let wrap_diff_lines = app_state
        .active_review_session()
        .map(|session| session.wrap_diff_lines)
        .unwrap_or(false);
    let combined_side_by_side_widths = if wrap_diff_lines {
        None
    } else {
        combined_side_by_side_column_widths(&contexts)
    };
    let (items, has_side_by_side_rows) = build_combined_diff_view_items(&contexts);
    let view_state = prepare_combined_diff_view_state(app_state, center_mode);
    reset_list_state_preserving_scroll(&view_state.list_state, items.len());
    scroll_combined_diff_list_to_focus(
        &view_state,
        &items,
        &contexts,
        &detail.review_threads,
        selected_path,
        selected_anchor,
    );

    if let Some(active_pr_key) = app_state.active_pr_key.clone() {
        install_combined_diff_scroll_handler(
            state,
            &view_state,
            items.clone(),
            contexts.clone(),
            active_pr_key,
            center_mode,
        );
    }

    let floating_header = app_state
        .pr_header_compact
        .then(|| {
            current_combined_diff_header(
                &view_state.list_state,
                &contexts,
                &items,
                app_state.selected_file_path.as_deref().or(selected_path),
            )
        })
        .flatten();
    let floating_header_path = floating_header
        .as_ref()
        .map(|header| header.header.path.clone());
    let show_top_fade = floating_header.is_some();
    let state = state.clone();
    let render_state = state.clone();
    let items = Arc::new(items);
    let contexts = Arc::new(contexts);
    let list_state = view_state.list_state.clone();
    let render_collapse_list_state = view_state.list_state.clone();
    let scrollbar_list_state = view_state.list_state.clone();
    let scrollbar_activity = view_state.scrollbar_activity.clone();
    let side_by_side_scroll_handles = SideBySideScrollHandles {
        left: view_state.side_by_side_left_scroll.clone(),
        right: view_state.side_by_side_right_scroll.clone(),
    };
    let render_side_by_side_scroll_handles = side_by_side_scroll_handles.clone();
    let render_review_stack = review_stack.clone();
    let render_floating_header_path = floating_header_path.clone();
    let item_count = items.len();
    let rows = list(list_state, move |ix, _window, cx| {
        render_combined_diff_view_item(
            &render_state,
            render_review_stack.clone(),
            render_floating_header_path.as_deref(),
            contexts.as_ref(),
            items[ix].clone(),
            ix,
            wrap_diff_lines,
            &render_side_by_side_scroll_handles,
            combined_side_by_side_widths,
            &render_collapse_list_state,
            cx,
        )
    })
    .with_sizing_behavior(ListSizingBehavior::Auto)
    .flex_grow()
    .min_h_0();

    let use_whole_diff_horizontal_scroll = !wrap_diff_lines && !has_side_by_side_rows;
    let body = if use_whole_diff_horizontal_scroll {
        restrict_diff_scroll_to_axis(div().flex().flex_col().flex_grow().min_h_0().min_w_0())
            .id("combined-diff-horizontal-scroll")
            .overflow_x_scroll()
            .scrollbar_width(px(DIFF_SCROLLBAR_WIDTH))
            .child(rows.min_w(px(DIFF_UNIFIED_MIN_WIDTH)))
            .into_any_element()
    } else {
        rows.into_any_element()
    };
    let body = render_diff_scroll_body(
        body,
        &scrollbar_list_state,
        item_count,
        &scrollbar_activity,
        (!wrap_diff_lines && has_side_by_side_rows).then_some(&side_by_side_scroll_handles),
        DiffScrollbarInsets::combined_body(),
        show_top_fade,
    );

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .bg(diff_editor_bg())
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow()
                .min_h_0()
                .min_w_0()
                .pl(px(DIFF_CONTENT_LEFT_GUTTER))
                .pr(px(DIFF_CONTENT_RIGHT_GUTTER))
                .child(body),
        )
        .when_some(floating_header, |el, header| {
            el.child(render_floating_diff_file_header(
                &state,
                review_stack.clone(),
                header,
                wrap_diff_lines,
            ))
        })
        .into_any_element()
}

pub(super) fn render_diff_scroll_body(
    body: AnyElement,
    list_state: &ListState,
    item_count: usize,
    scrollbar_activity: &DiffScrollbarActivity,
    side_by_side_scroll_handles: Option<&SideBySideScrollHandles>,
    side_by_side_scrollbar_insets: DiffScrollbarInsets,
    show_top_fade: bool,
) -> AnyElement {
    let scrollbars_visible = scrollbar_activity.is_visible();
    let has_side_by_side_scrollbars = side_by_side_scroll_handles
        .map(|handles| {
            handles.left_has_horizontal_scroll() || handles.right_has_horizontal_scroll()
        })
        .unwrap_or(false);
    let side_by_side_scrollbars = scrollbars_visible
        .then(|| {
            side_by_side_scroll_handles.and_then(|handles| {
                render_side_by_side_horizontal_scrollbars(
                    handles,
                    side_by_side_scrollbar_insets,
                    scrollbar_activity,
                )
            })
        })
        .flatten();
    let scroll_activity = scrollbar_activity.clone();

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .when(has_side_by_side_scrollbars, |el| {
            el.pb(px(DIFF_SCROLLBAR_WIDTH + 4.0))
        })
        .on_scroll_wheel(move |_, window, cx| {
            mark_diff_scrollbars_active(&scroll_activity, window, cx);
        })
        .child(body)
        .when(show_top_fade, |el| el.child(render_diff_scroll_top_fade()))
        .when_some(
            scrollbars_visible
                .then(|| render_diff_vertical_scrollbar(list_state, item_count, scrollbar_activity))
                .flatten(),
            |el, scrollbar| el.child(scrollbar),
        )
        .when_some(side_by_side_scrollbars, |el, scrollbar| el.child(scrollbar))
        .into_any_element()
}

fn render_diff_scroll_top_fade() -> AnyElement {
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .h(px(DIFF_SCROLL_TOP_FADE_HEIGHT))
        .bg(linear_gradient(
            180.0,
            linear_color_stop(with_alpha(diff_editor_bg(), 0.96), 0.0),
            linear_color_stop(with_alpha(diff_editor_bg(), 0.0), 1.0),
        ))
        .into_any_element()
}

pub(super) fn restrict_diff_scroll_to_axis(mut element: Div) -> Div {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}

#[derive(Clone)]
struct DiffVerticalScrollbarDrag {
    id: String,
    list_state: ListState,
    start_pointer_y: Rc<RefCell<Option<Pixels>>>,
    start_scroll_offset: f32,
    max_scroll_offset: f32,
    thumb_travel: f32,
}

impl DiffVerticalScrollbarDrag {
    fn new(
        id: String,
        list_state: ListState,
        start_scroll_offset: f32,
        max_scroll_offset: f32,
        thumb_travel: f32,
    ) -> Self {
        Self {
            id,
            list_state,
            start_pointer_y: Rc::new(RefCell::new(None)),
            start_scroll_offset,
            max_scroll_offset,
            thumb_travel,
        }
    }

    fn drag_to(&self, pointer_y: Pixels, window: &mut Window) {
        if self.thumb_travel <= 0.0 || self.max_scroll_offset <= 0.0 {
            return;
        }

        let start_pointer_y = {
            let mut start_pointer_y = self.start_pointer_y.borrow_mut();
            *start_pointer_y.get_or_insert(pointer_y)
        };
        let delta = f32::from(pointer_y - start_pointer_y);
        let scroll_offset = (self.start_scroll_offset
            + delta / self.thumb_travel * self.max_scroll_offset)
            .clamp(0.0, self.max_scroll_offset);
        self.list_state
            .set_offset_from_scrollbar(point(px(0.0), px(-scroll_offset)));
        window.refresh();
    }
}

#[derive(Clone)]
struct DiffHorizontalScrollbarDrag {
    id: String,
    scroll_handle: ScrollHandle,
    start_pointer_x: Rc<RefCell<Option<Pixels>>>,
    start_scroll_offset: f32,
    max_scroll_offset: f32,
    thumb_travel: f32,
}

impl DiffHorizontalScrollbarDrag {
    fn new(
        id: String,
        scroll_handle: ScrollHandle,
        start_scroll_offset: f32,
        max_scroll_offset: f32,
        thumb_travel: f32,
    ) -> Self {
        Self {
            id,
            scroll_handle,
            start_pointer_x: Rc::new(RefCell::new(None)),
            start_scroll_offset,
            max_scroll_offset,
            thumb_travel,
        }
    }

    fn drag_to(&self, pointer_x: Pixels, window: &mut Window) {
        if self.thumb_travel <= 0.0 || self.max_scroll_offset <= 0.0 {
            return;
        }

        let start_pointer_x = {
            let mut start_pointer_x = self.start_pointer_x.borrow_mut();
            *start_pointer_x.get_or_insert(pointer_x)
        };
        let delta = f32::from(pointer_x - start_pointer_x);
        let scroll_offset = (self.start_scroll_offset
            + delta / self.thumb_travel * self.max_scroll_offset)
            .clamp(0.0, self.max_scroll_offset);
        let current_offset = self.scroll_handle.offset();
        self.scroll_handle
            .set_offset(point(px(-scroll_offset), current_offset.y));
        window.refresh();
    }
}

pub(super) struct DiffScrollbarDragPreview;

impl Render for DiffScrollbarDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().w(px(0.0)).h(px(0.0))
    }
}

const DIFF_SCROLLBAR_IDLE_HIDE_DELAY: Duration = Duration::from_millis(650);

fn mark_diff_scrollbars_active(
    scrollbar_activity: &DiffScrollbarActivity,
    window: &mut Window,
    cx: &mut App,
) {
    let (generation, was_hidden) = scrollbar_activity.show();
    if was_hidden {
        window.refresh();
    }

    let scrollbar_activity = scrollbar_activity.clone();
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(DIFF_SCROLLBAR_IDLE_HIDE_DELAY)
                .await;
            cx.update(|window, _cx| {
                if scrollbar_activity.hide_if_current(generation) {
                    window.refresh();
                }
            })
            .ok();
        })
        .detach();
}

fn render_diff_vertical_scrollbar(
    list_state: &ListState,
    item_count: usize,
    scrollbar_activity: &DiffScrollbarActivity,
) -> Option<AnyElement> {
    if item_count <= 1 {
        return None;
    }

    let viewport_height = f32::from(list_state.viewport_bounds().size.height);
    let max_offset = f32::from(list_state.max_offset_for_scrollbar().height);
    if viewport_height <= 0.0 || max_offset <= 0.0 {
        return None;
    }

    let content_height = viewport_height + max_offset;
    let thumb_height =
        (viewport_height * (viewport_height / content_height)).clamp(36.0, viewport_height);
    let scroll_offset =
        (-f32::from(list_state.scroll_px_offset_for_scrollbar().y)).clamp(0.0, max_offset);
    let thumb_top = if max_offset <= 0.0 {
        0.0
    } else {
        scroll_offset / max_offset * (viewport_height - thumb_height)
    };
    let thumb_color: Hsla = with_alpha(fg_muted(), 0.38).into();
    let drag_id = "diff-vertical-scrollbar-thumb".to_string();
    let drag = DiffVerticalScrollbarDrag::new(
        drag_id.clone(),
        list_state.clone(),
        scroll_offset,
        max_offset,
        viewport_height - thumb_height,
    );
    let drag_id_for_move = drag_id.clone();
    let drag_start_activity = scrollbar_activity.clone();
    let drag_move_activity = scrollbar_activity.clone();

    Some(
        div()
            .absolute()
            .top(px(0.0))
            .right(px(2.0))
            .w(px(8.0))
            .h(px(viewport_height))
            .child(
                div()
                    .absolute()
                    .top(px(thumb_top))
                    .right(px(0.0))
                    .w(px(8.0))
                    .h(px(thumb_height))
                    .id(ElementId::Name(drag_id.into()))
                    .cursor(CursorStyle::ResizeUpDown)
                    .on_drag(drag, move |_, _, window, cx| {
                        mark_diff_scrollbars_active(&drag_start_activity, window, cx);
                        cx.new(|_| DiffScrollbarDragPreview)
                    })
                    .on_drag_move(
                        move |event: &DragMoveEvent<DiffVerticalScrollbarDrag>, window, cx| {
                            let drag = event.drag(cx).clone();
                            if drag.id != drag_id_for_move {
                                return;
                            }
                            mark_diff_scrollbars_active(&drag_move_activity, window, cx);
                            drag.drag_to(event.event.position.y, window);
                        },
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(2.0))
                            .w(px(4.0))
                            .h_full()
                            .rounded(px(2.0))
                            .bg(thumb_color),
                    ),
            )
            .into_any_element(),
    )
}

#[derive(Clone, Copy)]
pub(super) struct DiffScrollbarInsets {
    left: f32,
    right: f32,
}

impl DiffScrollbarInsets {
    pub(super) fn none() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
        }
    }

    fn combined_body() -> Self {
        Self {
            left: DIFF_SECTION_BODY_LEFT_MARGIN,
            right: DIFF_SECTION_BODY_RIGHT_MARGIN,
        }
    }
}

fn render_side_by_side_horizontal_scrollbars(
    handles: &SideBySideScrollHandles,
    insets: DiffScrollbarInsets,
    scrollbar_activity: &DiffScrollbarActivity,
) -> Option<AnyElement> {
    let left_has_scroll = handles.left_has_horizontal_scroll();
    let right_has_scroll = handles.right_has_horizontal_scroll();
    if !left_has_scroll && !right_has_scroll {
        return None;
    }

    Some(
        div()
            .absolute()
            .left(px(insets.left))
            .right(px(insets.right))
            .bottom(px(2.0))
            .h(px(DIFF_SCROLLBAR_WIDTH))
            .flex()
            .child(render_side_by_side_horizontal_scrollbar_lane(
                SideBySideDiffSide::Left,
                handles.handle_for(SideBySideDiffSide::Left),
                left_has_scroll,
                scrollbar_activity,
            ))
            .child(render_side_by_side_horizontal_scrollbar_lane(
                SideBySideDiffSide::Right,
                handles.handle_for(SideBySideDiffSide::Right),
                right_has_scroll,
                scrollbar_activity,
            ))
            .into_any_element(),
    )
}

fn render_side_by_side_horizontal_scrollbar_lane(
    side: SideBySideDiffSide,
    handle: &ScrollHandle,
    visible: bool,
    scrollbar_activity: &DiffScrollbarActivity,
) -> AnyElement {
    if !visible {
        return div().flex_1().min_w_0().into_any_element();
    }

    let viewport_width = f32::from(handle.bounds().size.width);
    let max_offset = f32::from(handle.max_offset().width);
    if viewport_width <= 0.0 || max_offset <= 0.0 {
        return div().flex_1().min_w_0().into_any_element();
    }

    let content_width = viewport_width + max_offset;
    let thumb_width =
        (viewport_width * (viewport_width / content_width)).clamp(36.0, viewport_width);
    let scroll_offset = (-f32::from(handle.offset().x)).clamp(0.0, max_offset);
    let thumb_left = scroll_offset / max_offset * (viewport_width - thumb_width);
    let thumb_color: Hsla = with_alpha(fg_muted(), 0.38).into();
    let drag_id = format!("diff-side-by-side-horizontal-scrollbar-{}", side.id_label());
    let drag = DiffHorizontalScrollbarDrag::new(
        drag_id.clone(),
        handle.clone(),
        scroll_offset,
        max_offset,
        viewport_width - thumb_width,
    );
    let drag_id_for_move = drag_id.clone();
    let drag_start_activity = scrollbar_activity.clone();
    let drag_move_activity = scrollbar_activity.clone();

    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(DIFF_SCROLLBAR_WIDTH))
        .child(
            div()
                .absolute()
                .left(px(thumb_left))
                .top(px(0.0))
                .w(px(thumb_width))
                .h(px(DIFF_SCROLLBAR_WIDTH))
                .id(ElementId::Name(drag_id.into()))
                .cursor(CursorStyle::ResizeLeftRight)
                .on_drag(drag, move |_, _, window, cx| {
                    mark_diff_scrollbars_active(&drag_start_activity, window, cx);
                    cx.new(|_| DiffScrollbarDragPreview)
                })
                .on_drag_move(
                    move |event: &DragMoveEvent<DiffHorizontalScrollbarDrag>, window, cx| {
                        let drag = event.drag(cx).clone();
                        if drag.id != drag_id_for_move {
                            return;
                        }
                        mark_diff_scrollbars_active(&drag_move_activity, window, cx);
                        drag.drag_to(event.event.position.x, window);
                    },
                )
                .child(
                    div()
                        .absolute()
                        .top(px(2.0))
                        .w_full()
                        .h(px(4.0))
                        .rounded(px(2.0))
                        .bg(thumb_color),
                ),
        )
        .into_any_element()
}

fn prepare_combined_diff_file_context(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
    review_stack: &ReviewStack,
    stack_filter: Option<&LayerDiffFilter>,
    center_mode: ReviewCenterMode,
    diff_layout: DiffLayout,
    cx: &App,
) -> CombinedDiffFileContext {
    let original_parsed = find_parsed_diff_file(&detail.parsed_diff, &file.path);
    let file_content_state = app_state
        .active_detail_state()
        .and_then(|detail_state| detail_state.file_content_states.get(&file.path));
    let prepared_file = file_content_state.and_then(|state| state.prepared.as_ref());
    let has_review_submission = !crate::local_review::is_local_review_detail(detail);
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

    let mut parsed = original_parsed.cloned().map(Arc::new);
    let mut parsed_override = None;
    let mut structural_side_by_side = None;
    let mut state_message = None;
    let diff_view_state = if center_mode == ReviewCenterMode::StructuralDiff {
        let structural_state = app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.structural_diff_states.get(&file.path));
        match structural_state {
            None => {
                state_message = Some("Preparing structural diff with difftastic...".to_string());
                None
            }
            Some(structural_state) if structural_state.loading => {
                state_message = Some("Building structural diff with difftastic...".to_string());
                None
            }
            Some(structural_state) => {
                if let Some(error) = structural_state.error.as_deref() {
                    state_message = Some(format!("Structural diff unavailable: {error}"));
                    None
                } else if let Some(structural) = structural_state.diff.as_ref() {
                    let request_key = structural_state
                        .request_key
                        .as_deref()
                        .unwrap_or("structural");
                    let view_state = prepare_structural_diff_view_state(
                        app_state,
                        detail,
                        &file.path,
                        request_key,
                        structural,
                    );
                    let structural_parsed = Arc::new(structural.parsed_file.clone());
                    parsed = Some(structural_parsed.clone());
                    parsed_override = Some(structural_parsed);
                    if diff_layout == DiffLayout::SideBySide {
                        structural_side_by_side = Some(structural.clone());
                    }
                    Some(view_state)
                } else {
                    state_message =
                        Some("Preparing structural diff with difftastic...".to_string());
                    None
                }
            }
        }
    } else {
        Some(prepare_diff_view_state(app_state, detail, &file.path))
    };

    let rows = diff_view_state
        .as_ref()
        .map(|state| state.rows.clone())
        .unwrap_or_else(|| Arc::new(Vec::new()));
    let parsed_file_index = diff_view_state
        .as_ref()
        .and_then(|state| state.parsed_file_index);
    let highlighted_hunks = diff_view_state
        .as_ref()
        .and_then(|state| state.highlighted_hunks.clone());
    let normal_side_by_side = (center_mode != ReviewCenterMode::StructuralDiff
        && structural_side_by_side.is_none()
        && diff_layout == DiffLayout::SideBySide)
        .then(|| {
            parsed
                .as_deref()
                .filter(|parsed| !parsed.hunks.is_empty() && !parsed.is_binary)
        })
        .flatten()
        .map(|parsed| Arc::new(build_normal_side_by_side_diff_file(parsed)));
    let stack_visibility =
        stack_filter.map(|filter| stack_file_visibility(review_stack, filter, &file.path));
    let gutter_layout = diff_gutter_layout(file, parsed.as_deref(), reserve_waypoint_slot);
    let wrap_diff_lines = app_state
        .active_review_session()
        .map(|session| session.wrap_diff_lines)
        .unwrap_or(false);
    let side_by_side_column_widths = side_by_side_column_widths_for_file(
        parsed.as_deref(),
        structural_side_by_side.as_deref(),
        normal_side_by_side.as_deref(),
        gutter_layout.reserve_waypoint_slot,
        wrap_diff_lines,
    );
    let file_lsp_context =
        build_diff_file_lsp_context(state, file.path.as_str(), prepared_file, cx);
    let selected_anchor = selected_anchor
        .filter(|anchor| {
            diff_anchor_matches_file(anchor, &file.path)
                && (!anchor.file_path.is_empty() || selected_path == Some(file.path.as_str()))
        })
        .cloned();
    let items = state_message
        .is_none()
        .then(|| {
            build_diff_view_items(
                file,
                parsed.as_deref(),
                prepared_file,
                rows.as_ref(),
                structural_side_by_side.as_deref(),
                normal_side_by_side.as_deref(),
                stack_visibility.as_ref(),
            )
        })
        .unwrap_or_default();
    let mut header = ReviewFileHeaderProps::from_pull_request_file(file);
    header.previous_path = parsed
        .as_ref()
        .and_then(|parsed| parsed.previous_path.clone());
    header.binary = parsed
        .as_ref()
        .map(|parsed| parsed.is_binary)
        .unwrap_or(false);
    header.active = selected_path == Some(file.path.as_str());

    CombinedDiffFileContext {
        file: file.clone(),
        header,
        collapsed: app_state.is_review_file_collapsed(&file.path),
        reviewed: app_state.is_review_file_reviewed(&file.path),
        parsed,
        parsed_override,
        structural_side_by_side,
        normal_side_by_side,
        side_by_side_column_widths,
        rows,
        parsed_file_index,
        highlighted_hunks,
        gutter_layout,
        file_lsp_context,
        selected_anchor,
        stack_visibility,
        items: Arc::new(items),
        state_message,
    }
}

fn build_combined_diff_view_items(
    contexts: &[CombinedDiffFileContext],
) -> (Vec<CombinedDiffViewItem>, bool) {
    let mut items = Vec::new();
    let mut has_side_by_side_rows = false;

    for (file_index, context) in contexts.iter().enumerate() {
        items.push(CombinedDiffViewItem::Header(file_index));
        if context.collapsed {
            continue;
        }
        if context.stack_visibility.is_some() {
            items.push(CombinedDiffViewItem::StackNotice(file_index));
        }
        if let Some(message) = context.state_message.as_ref() {
            items.push(CombinedDiffViewItem::State {
                file_index,
                message: message.clone(),
            });
        } else {
            items.extend(context.items.iter().map(|item| CombinedDiffViewItem::Row {
                file_index,
                item: *item,
            }));
        }
        items.push(CombinedDiffViewItem::Footer);
        has_side_by_side_rows = has_side_by_side_rows
            || context.structural_side_by_side.is_some()
            || context.normal_side_by_side.is_some();
    }

    (items, has_side_by_side_rows)
}

fn combined_diff_file_expanded_extra_item_count(context: &CombinedDiffFileContext) -> usize {
    usize::from(context.stack_visibility.is_some())
        + if context.state_message.is_some() {
            1
        } else {
            context.items.len()
        }
        + 1
}

fn prepare_combined_diff_view_state(
    app_state: &AppState,
    center_mode: ReviewCenterMode,
) -> CombinedDiffViewState {
    let scope = match center_mode {
        ReviewCenterMode::StructuralDiff => "combined-structural-diff",
        ReviewCenterMode::Stack => "combined-stack-diff",
        _ => "combined-files-diff",
    };
    let key = review_cache_key(app_state.active_pr_key.as_deref(), scope);
    let mut list_states = app_state.combined_diff_view_states.borrow_mut();
    list_states
        .entry(key)
        .or_insert_with(CombinedDiffViewState::new)
        .clone()
}

fn scroll_combined_diff_list_to_focus(
    view_state: &CombinedDiffViewState,
    items: &[CombinedDiffViewItem],
    contexts: &[CombinedDiffFileContext],
    review_threads: &[PullRequestReviewThread],
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
) {
    let Some(selected_path) = selected_path else {
        return;
    };

    let target_key = selected_anchor
        .filter(|anchor| diff_anchor_matches_file(anchor, selected_path))
        .and_then(|anchor| diff_focus_key(selected_path, anchor))
        .unwrap_or_else(|| combined_file_focus_key(selected_path));
    let mut last_focus_key = view_state.last_focus_key.borrow_mut();
    if last_focus_key.as_deref() == Some(target_key.as_str()) {
        return;
    }

    let item_ix = selected_anchor
        .filter(|anchor| diff_anchor_matches_file(anchor, selected_path))
        .and_then(|anchor| {
            find_combined_diff_anchor_item_index(
                items,
                contexts,
                review_threads,
                selected_path,
                anchor,
            )
        })
        .or_else(|| find_combined_diff_file_header_index(items, contexts, selected_path));

    if let Some(item_ix) = item_ix {
        view_state.list_state.scroll_to(ListOffset {
            item_ix: item_ix.saturating_sub(2),
            offset_in_item: px(0.0),
        });
        *last_focus_key = Some(target_key);
    }
}

fn install_combined_diff_scroll_handler(
    state: &Entity<AppState>,
    view_state: &CombinedDiffViewState,
    items: Vec<CombinedDiffViewItem>,
    contexts: Vec<CombinedDiffFileContext>,
    active_pr_key: String,
    center_mode: ReviewCenterMode,
) {
    let list_state = view_state.list_state.clone();
    let last_focus_key = view_state.last_focus_key.clone();
    let state_for_scroll = state.clone();
    let items = Arc::new(items);
    let contexts = Arc::new(contexts);
    list_state
        .clone()
        .set_scroll_handler(move |event, window, _| {
            let state = state_for_scroll.clone();
            let list_state = list_state.clone();
            let last_focus_key = last_focus_key.clone();
            let items = items.clone();
            let contexts = contexts.clone();
            let active_pr_key = active_pr_key.clone();
            let visible_range = event.visible_range.clone();
            window.on_next_frame(move |window, cx| {
                let scroll_top = list_state.logical_scroll_top();
                let focus = combined_diff_reading_focus(
                    items.as_ref(),
                    contexts.as_ref(),
                    &list_state,
                    visible_range,
                );
                let compact = scroll_top.item_ix > 0 || scroll_top.offset_in_item > px(0.0);
                let mut should_load_content = false;
                let mut should_load_structural = false;
                state.update(cx, |state, cx| {
                    if state.active_surface != PullRequestSurface::Files
                        || state.active_pr_key.as_deref() != Some(active_pr_key.as_str())
                    {
                        return;
                    }
                    let Some(session_mode) = state
                        .active_review_session()
                        .map(|session| session.center_mode)
                    else {
                        return;
                    };
                    let session_matches = session_mode == center_mode
                        || (session_mode == ReviewCenterMode::GuidedReview
                            && matches!(
                                center_mode,
                                ReviewCenterMode::Stack | ReviewCenterMode::StructuralDiff
                            ));
                    if !session_matches {
                        return;
                    }

                    if let Some(focus) = focus.as_ref() {
                        if state.selected_file_path.as_deref() != Some(focus.file_path.as_str()) {
                            state.selected_file_path = Some(focus.file_path.clone());
                            state.selected_diff_anchor = None;
                            *last_focus_key.borrow_mut() =
                                Some(combined_file_focus_key(&focus.file_path));
                            should_load_content = true;
                            should_load_structural =
                                center_mode == ReviewCenterMode::StructuralDiff;
                            cx.notify();
                        }
                        let review_focus_mode = if session_mode == ReviewCenterMode::GuidedReview {
                            ReviewCenterMode::GuidedReview
                        } else {
                            center_mode
                        };
                        state.set_review_scroll_focus(
                            review_focus_mode,
                            focus.file_path.clone(),
                            focus.line,
                            focus.side.clone(),
                            focus.anchor.clone(),
                        );
                    }

                    if state.pr_header_compact != compact {
                        state.pr_header_compact = compact;
                        cx.notify();
                    }
                });

                if should_load_structural {
                    ensure_selected_structural_diff_loaded(&state, window, cx);
                }
                if should_load_content {
                    ensure_selected_file_content_loaded(&state, window, cx);
                }
            });
        });
}

fn render_combined_diff_view_item(
    state: &Entity<AppState>,
    review_stack: Arc<ReviewStack>,
    floating_header_path: Option<&str>,
    contexts: &[CombinedDiffFileContext],
    item: CombinedDiffViewItem,
    item_ix: usize,
    wrap_diff_lines: bool,
    side_by_side_scroll_handles: &SideBySideScrollHandles,
    combined_side_by_side_column_widths: Option<SideBySideColumnWidths>,
    collapse_list_state: &ListState,
    cx: &App,
) -> AnyElement {
    match item {
        CombinedDiffViewItem::Header(file_index) => contexts
            .get(file_index)
            .map(|context| {
                let hidden_by_floating_header =
                    floating_header_path == Some(context.file.path.as_str());
                div()
                    .ml(px(DIFF_SECTION_HEADER_LEFT_MARGIN))
                    .mr(px(DIFF_SECTION_HEADER_RIGHT_MARGIN))
                    .pt(if item_ix == 0 {
                        px(DIFF_FILE_HEADER_TOP_MARGIN_FIRST)
                    } else {
                        px(DIFF_FILE_HEADER_TOP_MARGIN)
                    })
                    .pb(px(DIFF_FILE_HEADER_BOTTOM_MARGIN))
                    .when(hidden_by_floating_header, |el| el.invisible())
                    .child(render_diff_file_header_row(
                        state,
                        review_stack,
                        context.header.clone(),
                        wrap_diff_lines,
                        format!("row-{item_ix}"),
                        context.collapsed,
                        context.reviewed,
                        Some(DiffFileCollapseScrollAdjustment::for_combined_file(
                            collapse_list_state,
                            item_ix,
                            context,
                        )),
                    ))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        CombinedDiffViewItem::StackNotice(file_index) => contexts
            .get(file_index)
            .and_then(|context| context.stack_visibility.as_ref())
            .map(|visibility| {
                render_combined_diff_body_row(
                    render_stack_layer_diff_notice(visibility).into_any_element(),
                )
            })
            .unwrap_or_else(|| div().into_any_element()),
        CombinedDiffViewItem::State {
            file_index,
            message,
        } => contexts
            .get(file_index)
            .map(|_| {
                render_combined_diff_body_row(render_diff_state_row(message).into_any_element())
            })
            .unwrap_or_else(|| div().into_any_element()),
        CombinedDiffViewItem::Row { file_index, item } => contexts
            .get(file_index)
            .map(|context| {
                render_combined_diff_row_item(
                    state,
                    context,
                    item,
                    side_by_side_scroll_handles,
                    combined_side_by_side_column_widths,
                    cx,
                )
            })
            .unwrap_or_else(|| div().into_any_element()),
        CombinedDiffViewItem::Footer => div().h(px(12.0)).into_any_element(),
    }
}

fn render_combined_diff_row_item(
    state: &Entity<AppState>,
    context: &CombinedDiffFileContext,
    item: DiffViewItem,
    side_by_side_scroll_handles: &SideBySideScrollHandles,
    combined_side_by_side_column_widths: Option<SideBySideColumnWidths>,
    cx: &App,
) -> AnyElement {
    let row = match item {
        DiffViewItem::Gap(gap) => {
            render_diff_gap_row(gap, context.gutter_layout).into_any_element()
        }
        DiffViewItem::StackLayerEmpty => render_diff_state_row(
            "No changed hunks in this file belong to the selected stack layer.",
        )
        .into_any_element(),
        DiffViewItem::Row(row_ix) => context
            .rows
            .get(row_ix)
            .map(|row| {
                render_virtualized_diff_row(
                    state,
                    context.gutter_layout,
                    context.parsed_file_index,
                    context.parsed_override.as_deref(),
                    context.structural_side_by_side.as_deref(),
                    context.normal_side_by_side.as_deref(),
                    context.highlighted_hunks.as_deref(),
                    context.file_lsp_context.as_ref(),
                    row,
                    context.selected_anchor.as_ref(),
                    side_by_side_scroll_handles,
                    combined_side_by_side_column_widths.or(context.side_by_side_column_widths),
                    cx,
                )
                .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
    };

    render_combined_diff_body_row(row)
}

fn render_combined_diff_body_row(child: AnyElement) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .pl(px(DIFF_SECTION_BODY_LEFT_MARGIN))
        .pr(px(DIFF_SECTION_BODY_RIGHT_MARGIN))
        .child(
            div()
                .w_full()
                .min_w_0()
                .border_l(px(1.0))
                .border_r(px(1.0))
                .border_color(diff_annotation_border())
                .overflow_hidden()
                .child(child),
        )
        .into_any_element()
}

fn combined_diff_scroll_focus_for_item_index(
    items: &[CombinedDiffViewItem],
    contexts: &[CombinedDiffFileContext],
    item_ix: usize,
) -> Option<DiffScrollFocus> {
    let item_ix = focus_item_index_around(items.len(), item_ix, |ix| {
        combined_diff_scroll_focus_for_item(&items[ix], contexts).is_some()
    })?;
    combined_diff_scroll_focus_for_item(&items[item_ix], contexts)
}

fn combined_diff_reading_focus(
    items: &[CombinedDiffViewItem],
    contexts: &[CombinedDiffFileContext],
    list_state: &ListState,
    visible_range: std::ops::Range<usize>,
) -> Option<DiffScrollFocus> {
    if let Some(item_ix) = reading_focus_item_index(
        items.len(),
        visible_range,
        list_state.viewport_bounds(),
        |ix| list_state.bounds_for_item(ix),
        |ix| combined_diff_changed_scroll_focus_for_item(&items[ix], contexts).is_some(),
    ) {
        return combined_diff_changed_scroll_focus_for_item(&items[item_ix], contexts);
    }

    let scroll_top = list_state.logical_scroll_top();
    combined_diff_scroll_focus_for_item_index(items, contexts, scroll_top.item_ix)
}

pub(super) fn reading_focus_item_index(
    item_count: usize,
    visible_range: std::ops::Range<usize>,
    viewport: Bounds<Pixels>,
    mut bounds_for_item: impl FnMut(usize) -> Option<Bounds<Pixels>>,
    mut is_changed_focus_item: impl FnMut(usize) -> bool,
) -> Option<usize> {
    let viewport_height = f32::from(viewport.size.height);
    if viewport_height <= 0.0 {
        return None;
    }

    let reading_y = viewport.top() + px(viewport_height * 0.33);
    for ix in visible_range {
        if ix >= item_count {
            continue;
        }
        let Some(bounds) = bounds_for_item(ix) else {
            continue;
        };
        if bounds.bottom() < reading_y {
            continue;
        }
        if is_changed_focus_item(ix) {
            return Some(ix);
        }
    }
    None
}

pub(super) fn focus_item_index_around(
    item_count: usize,
    item_ix: usize,
    mut is_focus_item: impl FnMut(usize) -> bool,
) -> Option<usize> {
    if item_count == 0 {
        return None;
    }

    let item_ix = item_ix.min(item_count.saturating_sub(1));
    for ix in item_ix..item_count {
        if is_focus_item(ix) {
            return Some(ix);
        }
    }

    (0..item_ix).rev().find(|ix| is_focus_item(*ix))
}

fn combined_diff_changed_scroll_focus_for_item(
    item: &CombinedDiffViewItem,
    contexts: &[CombinedDiffFileContext],
) -> Option<DiffScrollFocus> {
    let CombinedDiffViewItem::Row { file_index, item } = item else {
        return None;
    };
    let context = contexts.get(*file_index)?;
    context.parsed.as_deref().and_then(|parsed| {
        diff_scroll_focus_for_item(
            *item,
            context.rows.as_ref(),
            parsed,
            context.file.path.as_str(),
        )
    })
}

fn combined_diff_scroll_focus_for_item(
    item: &CombinedDiffViewItem,
    contexts: &[CombinedDiffFileContext],
) -> Option<DiffScrollFocus> {
    match item {
        CombinedDiffViewItem::Header(file_index)
        | CombinedDiffViewItem::StackNotice(file_index)
        | CombinedDiffViewItem::State { file_index, .. } => contexts
            .get(*file_index)
            .map(|context| combined_file_scroll_focus(&context.file.path)),
        CombinedDiffViewItem::Row { file_index, item } => {
            let context = contexts.get(*file_index)?;
            context
                .parsed
                .as_deref()
                .and_then(|parsed| {
                    diff_scroll_focus_for_item(
                        *item,
                        context.rows.as_ref(),
                        parsed,
                        context.file.path.as_str(),
                    )
                })
                .or_else(|| Some(combined_file_scroll_focus(&context.file.path)))
        }
        CombinedDiffViewItem::Footer => None,
    }
}

fn combined_file_scroll_focus(file_path: &str) -> DiffScrollFocus {
    DiffScrollFocus {
        file_path: file_path.to_string(),
        line: None,
        side: None,
        hunk_header: None,
        anchor: None,
    }
}

fn find_combined_diff_anchor_item_index(
    items: &[CombinedDiffViewItem],
    contexts: &[CombinedDiffFileContext],
    review_threads: &[PullRequestReviewThread],
    file_path: &str,
    anchor: &DiffAnchor,
) -> Option<usize> {
    let context_ix = contexts
        .iter()
        .position(|context| context.file.path == file_path)?;
    let context = contexts.get(context_ix)?;
    let local_ix = find_diff_focus_item_index(
        context.items.as_ref(),
        context.rows.as_ref(),
        context.parsed.as_deref(),
        context.structural_side_by_side.as_deref(),
        context.normal_side_by_side.as_deref(),
        review_threads,
        anchor,
    )?;

    let mut seen_rows = 0usize;
    for (item_ix, item) in items.iter().enumerate() {
        if matches!(
            item,
            CombinedDiffViewItem::Row {
                file_index,
                ..
            } if *file_index == context_ix
        ) {
            if seen_rows == local_ix {
                return Some(item_ix);
            }
            seen_rows += 1;
        }
    }

    None
}

fn find_combined_diff_file_header_index(
    items: &[CombinedDiffViewItem],
    contexts: &[CombinedDiffFileContext],
    file_path: &str,
) -> Option<usize> {
    items.iter().position(|item| {
        matches!(
            item,
            CombinedDiffViewItem::Header(file_index)
                if contexts
                    .get(*file_index)
                    .map(|context| context.file.path == file_path)
                    .unwrap_or(false)
        )
    })
}

fn current_combined_diff_header(
    list_state: &ListState,
    contexts: &[CombinedDiffFileContext],
    items: &[CombinedDiffViewItem],
    selected_path: Option<&str>,
) -> Option<CombinedDiffFloatingHeader> {
    let (context_ix, context) = selected_path
        .and_then(|path| {
            contexts
                .iter()
                .enumerate()
                .find(|(_, context)| context.file.path == path)
        })
        .or_else(|| contexts.iter().enumerate().next())?;
    let header_item_ix = items.iter().position(|item| {
        matches!(
            item,
            CombinedDiffViewItem::Header(file_index) if *file_index == context_ix
        )
    });
    let mut header = context.header.clone();
    header.active = true;
    Some(CombinedDiffFloatingHeader {
        header,
        collapsed: context.collapsed,
        reviewed: context.reviewed,
        collapse_scroll: header_item_ix.map(|header_item_ix| {
            DiffFileCollapseScrollAdjustment::for_combined_file(list_state, header_item_ix, context)
        }),
    })
}

fn render_floating_diff_file_header(
    state: &Entity<AppState>,
    review_stack: Arc<ReviewStack>,
    header: CombinedDiffFloatingHeader,
    wrap_diff_lines: bool,
) -> impl IntoElement {
    div()
        .absolute()
        .top(px(0.0))
        .left(px(DIFF_CONTENT_LEFT_GUTTER))
        .right(px(DIFF_CONTENT_RIGHT_GUTTER))
        .occlude()
        .ml(px(DIFF_SECTION_HEADER_LEFT_MARGIN))
        .mr(px(DIFF_SECTION_HEADER_RIGHT_MARGIN))
        .pt(px(DIFF_FLOATING_FILE_HEADER_TOP_PADDING))
        .pb(px(DIFF_FLOATING_FILE_HEADER_BOTTOM_PADDING))
        .bg(diff_editor_bg())
        .child(render_diff_file_header_row(
            state,
            review_stack,
            header.header,
            wrap_diff_lines,
            "floating",
            header.collapsed,
            header.reviewed,
            header.collapse_scroll,
        ))
}

fn render_diff_file_header_row(
    state: &Entity<AppState>,
    review_stack: Arc<ReviewStack>,
    header: ReviewFileHeaderProps,
    wrap_diff_lines: bool,
    scope: impl Into<String>,
    collapsed: bool,
    reviewed: bool,
    collapse_scroll: Option<DiffFileCollapseScrollAdjustment>,
) -> impl IntoElement {
    let key = header.path.clone();
    let scope = scope.into();

    render_review_file_header_with_controls(
        header,
        Some(
            render_diff_file_collapse_toggle(
                state,
                collapsed,
                scope.clone(),
                key.clone(),
                collapse_scroll,
            )
            .into_any_element(),
        ),
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    render_diff_file_review_toggle(
                        state,
                        review_stack,
                        reviewed,
                        scope.clone(),
                        key.clone(),
                    )
                    .into_any_element(),
                )
                .child(
                    render_diff_line_wrap_toggle(state, wrap_diff_lines, scope, key)
                        .into_any_element(),
                )
                .into_any_element(),
        ),
    )
}

fn render_diff_file_collapse_toggle(
    state: &Entity<AppState>,
    collapsed: bool,
    scope: String,
    key: String,
    collapse_scroll: Option<DiffFileCollapseScrollAdjustment>,
) -> impl IntoElement {
    let state_for_toggle = state.clone();
    let collapse_scroll_for_toggle = collapse_scroll.clone();
    let icon = if collapsed {
        LucideIcon::ChevronRight
    } else {
        LucideIcon::ChevronDown
    };
    let tooltip = if collapsed {
        "Expand file"
    } else {
        "Collapse file"
    };

    div()
        .id(ElementId::Name(
            format!("diff-file-collapse-toggle-{scope}-{key}").into(),
        ))
        .h(px(28.0))
        .w(px(28.0))
        .flex_shrink_0()
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(transparent())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .hover(|style| style.bg(bg_selected()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            if let Some(adjustment) = collapse_scroll_for_toggle.as_ref() {
                adjustment.apply_for_toggle(collapsed);
            }
            state_for_toggle.update(cx, |state, cx| {
                state.set_review_file_collapsed(&key, !collapsed);
                state.persist_active_review_session();
                cx.notify();
            });
            cx.stop_propagation();
        })
        .child(lucide_icon(icon, 14.0, fg_muted()))
}

fn render_diff_file_review_toggle(
    state: &Entity<AppState>,
    review_stack: Arc<ReviewStack>,
    reviewed: bool,
    scope: String,
    key: String,
) -> impl IntoElement {
    let state_for_toggle = state.clone();
    let tooltip = if reviewed {
        "Mark file unreviewed"
    } else {
        "Mark file reviewed"
    };

    div()
        .id(ElementId::Name(
            format!("diff-file-review-toggle-{scope}-{key}").into(),
        ))
        .h(px(28.0))
        .w(px(30.0))
        .flex_shrink_0()
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if reviewed {
            success_muted()
        } else {
            diff_annotation_bg()
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .hover(|style| style.bg(bg_selected()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            state_for_toggle.update(cx, |state, cx| {
                state.set_review_file_reviewed(review_stack.as_ref(), &key, !reviewed);
                state.persist_active_review_session();
                cx.notify();
            });
            cx.stop_propagation();
        })
        .child(lucide_icon(
            LucideIcon::FileCheck,
            14.0,
            if reviewed { success() } else { fg_muted() },
        ))
}

fn render_diff_line_wrap_toggle(
    state: &Entity<AppState>,
    wrap_diff_lines: bool,
    scope: String,
    key: String,
) -> impl IntoElement {
    let state_for_toggle = state.clone();
    let tooltip = if wrap_diff_lines {
        "Disable line wrap"
    } else {
        "Wrap diff lines"
    };
    let icon = if wrap_diff_lines {
        LucideIcon::TextWrap
    } else {
        LucideIcon::ArrowLeftRight
    };

    div()
        .id(ElementId::Name(
            format!("diff-line-wrap-toggle-{scope}-{key}").into(),
        ))
        .h(px(28.0))
        .w(px(30.0))
        .flex_shrink_0()
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if wrap_diff_lines {
            accent_muted()
        } else {
            diff_annotation_bg()
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .hover(|style| style.bg(bg_selected()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            state_for_toggle.update(cx, |state, cx| {
                state.set_diff_line_wrap(!wrap_diff_lines);
                state.persist_active_review_session();
                cx.notify();
            });
            cx.stop_propagation();
        })
        .child(lucide_icon(
            icon,
            14.0,
            if wrap_diff_lines {
                accent()
            } else {
                fg_muted()
            },
        ))
}

fn combined_file_focus_key(file_path: &str) -> String {
    format!("file:{file_path}")
}

fn diff_anchor_matches_file(anchor: &DiffAnchor, fallback_file_path: &str) -> bool {
    if anchor.file_path.is_empty() {
        true
    } else {
        anchor.file_path == fallback_file_path
    }
}
