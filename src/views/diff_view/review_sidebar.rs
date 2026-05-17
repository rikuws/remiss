use super::*;

pub(super) fn prepare_review_stack(
    app_state: &AppState,
    detail: &PullRequestDetail,
) -> Arc<ReviewStack> {
    if let Some(stack) = prepare_discovered_review_stack(
        app_state,
        detail,
        "review-stack:real",
        StackDiscoveryOptions {
            enable_ai_virtual: false,
            enable_sem_virtual: false,
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
            enable_sem_virtual: false,
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
        structural_evidence: None,
        semantic_review: None,
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

pub(super) fn default_stack_layer<'a>(
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

pub(super) fn prepare_review_queue(
    app_state: &AppState,
    detail: &PullRequestDetail,
) -> Arc<ReviewQueue> {
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

pub(super) fn prepare_semantic_diff_file(
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

pub(super) fn render_review_sidebar_pane(
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
        ReviewCenterMode::SemanticDiff => render_changed_files_pane(
            state,
            detail,
            selected_path,
            ReviewFileRowOpenMode::Diff,
            cx,
        )
        .into_any_element(),
        ReviewCenterMode::StructuralDiff => render_changed_files_pane(
            state,
            detail,
            selected_path,
            ReviewFileRowOpenMode::Structural,
            cx,
        )
        .into_any_element(),
        ReviewCenterMode::SourceBrowser => {
            render_source_file_tree(state, detail, selected_path, cx).into_any_element()
        }
        ReviewCenterMode::GuidedReview | ReviewCenterMode::AiTour | ReviewCenterMode::Stack => {
            render_stack_navigation_pane(state, detail, review_stack, cx).into_any_element()
        }
    }
}

fn render_changed_files_pane(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    open_mode: ReviewFileRowOpenMode,
    cx: &App,
) -> impl IntoElement {
    let (tree_rows, file_count, additions, deletions, structural_status) = {
        let app_state = state.read(cx);
        let (file_count, additions, deletions) = review_file_tree_totals(detail, None);
        let structural_status = if matches!(open_mode, ReviewFileRowOpenMode::Structural) {
            app_state
                .active_detail_state()
                .and_then(|detail_state| detail_state.structural_diff_warmup.status_text())
        } else {
            None
        };
        (
            prepare_review_file_tree_rows(&app_state, detail, None),
            file_count,
            additions,
            deletions,
            structural_status,
        )
    };
    let list_state = {
        let app_state = state.read(cx);
        prepare_review_file_tree_list_state_for_scope(&app_state, "changed-file-tree")
    };
    if list_state.item_count() != tree_rows.len() {
        list_state.reset(tree_rows.len());
    }
    let selected_path = selected_path.map(str::to_string);
    let on_file_open = file_tree_row_open_handler();

    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_r(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(render_file_tree_header(
            state.clone(),
            "Changed Files",
            file_count,
            Some((additions, deletions)),
        ))
        .when_some(structural_status, |el, status| {
            el.child(render_structural_warmup_status(status))
        })
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
                        let tree_rows = tree_rows.clone();
                        let selected_path = selected_path.clone();
                        let on_file_open = on_file_open.clone();
                        move |ix, _window, cx| match tree_rows[ix].clone() {
                            ReviewFileTreeRow::Directory { name, depth } => {
                                render_file_tree_directory_row(name, depth).into_any_element()
                            }
                            ReviewFileTreeRow::File {
                                path,
                                name,
                                depth,
                                additions,
                                deletions,
                            } => {
                                let is_reviewed = state.read(cx).is_review_file_reviewed(&path);
                                render_file_tree_file_row(
                                    state.clone(),
                                    path,
                                    name,
                                    additions,
                                    deletions,
                                    depth,
                                    selected_path.as_deref(),
                                    open_mode,
                                    is_reviewed,
                                    on_file_open.clone(),
                                )
                                .into_any_element()
                            }
                        }
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                ),
        )
}

fn file_tree_row_open_handler() -> ReviewFileRowOpenHandler {
    Arc::new(handle_file_tree_row_open)
}

fn handle_file_tree_row_open(
    state: &Entity<AppState>,
    open_mode: ReviewFileRowOpenMode,
    window: &mut Window,
    cx: &mut App,
) {
    match open_mode {
        ReviewFileRowOpenMode::Diff | ReviewFileRowOpenMode::Stack => {
            ensure_selected_file_content_loaded(state, window, cx);
        }
        ReviewFileRowOpenMode::Structural => {
            ensure_selected_structural_diff_loaded(state, window, cx);
            ensure_selected_file_content_loaded(state, window, cx);
        }
        ReviewFileRowOpenMode::Source => {
            ensure_active_review_focus_loaded(state, window, cx);
        }
    }
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
                    "Guided Review",
                    "Review groups",
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
                "Guided Review",
                "Review groups",
                "0".to_string(),
            ))
            .child(
                div()
                    .px(px(14.0))
                    .py(px(12.0))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(fg_muted())
                    .child("Generate Guided Review to navigate review groups here."),
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

    div().pb(px(10.0)).child(
        div()
            .px(px(8.0))
            .py(px(8.0))
            .rounded(radius_sm())
            .border_1()
            .border_color(transparent())
            .bg(bg_surface())
            .cursor_pointer()
            .hover(|style| style.bg(hover_bg()))
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                list_state.scroll_to(ListOffset {
                    item_ix: target_index,
                    offset_in_item: px(0.0),
                });
            })
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
                            .gap(px(8.0))
                            .flex_wrap()
                            .child(ai_tour_metric_text(&format!(
                                "{} file{}",
                                metrics.file_count,
                                if metrics.file_count == 1 { "" } else { "s" }
                            )))
                            .child(ai_tour_delta_metric(metrics.additions, metrics.deletions))
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
    let guided_review_generation_state = {
        let app_state = state.read(cx);
        app_state
            .active_detail_state()
            .map(|detail_state| {
                let guide = &detail_state.review_partner_state;
                let ai_stack = &detail_state.ai_stack_state;
                let guide_pending = guide.document.is_none() && (guide.loading || guide.generating);
                let fallback_stack_visible =
                    ai_stack.stack.is_none() && review_stack.source != StackSource::GitHubNative;
                (guide_pending, fallback_stack_visible)
            })
            .unwrap_or((false, false))
    };
    let showing_temporary_guide_stack =
        guided_review_generation_state.0 && guided_review_generation_state.1;
    let selected_layer_id = review_stack
        .selected_layer(session.selected_stack_layer_id.as_deref())
        .map(|layer| layer.id.clone());
    let selected_layer_index = review_stack.selected_layer_index(selected_layer_id.as_deref());
    let progress_label = match (selected_layer_index, review_stack.layers.len()) {
        (Some(index), total) if total > 0 => format!("{} of {total}", index + 1),
        (_, total) => format!("0 of {total}"),
    };
    let trunk_branch = review_stack
        .trunk_branch
        .clone()
        .unwrap_or_else(|| detail.base_ref_name.clone());
    let list_state = {
        let app_state = state.read(cx);
        prepare_stack_timeline_list_state(&app_state)
    };
    let stack_filter = build_layer_diff_filter(
        review_stack.as_ref(),
        session.stack_diff_mode,
        selected_layer_id.as_deref(),
        &session.reviewed_stack_atom_ids,
    );
    let visible_paths = stack_filter
        .as_ref()
        .map(|filter| stack_file_paths_for_filter(review_stack.as_ref(), filter));
    let file_tree_label = stack_filter
        .as_ref()
        .map(|filter| review_file_tree_label(filter.mode))
        .unwrap_or_else(|| review_file_tree_label(StackDiffMode::WholePr));
    let (tree_rows, visible_file_count, visible_additions, visible_deletions) = {
        let app_state = state.read(cx);
        let (file_count, additions, deletions) =
            review_file_tree_totals(detail, visible_paths.as_ref());

        (
            prepare_review_file_tree_rows(&app_state, detail, visible_paths.as_ref()),
            file_count,
            additions,
            deletions,
        )
    };
    let file_tree_list_state = {
        let app_state = state.read(cx);
        prepare_review_file_tree_list_state_for_scope(&app_state, "stack-file-tree")
    };
    if file_tree_list_state.item_count() != tree_rows.len() {
        file_tree_list_state.reset(tree_rows.len());
    }
    let selected_path = state.read(cx).selected_file_path.clone();
    sync_stack_timeline_item_count(&list_state, review_stack.layers.len() + 1);
    let stack_timeline_height =
        px(((review_stack.layers.len().max(3) as f32 * 36.0) + 26.0).min(220.0));

    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_r(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(render_stack_view_header(
            progress_label,
            showing_temporary_guide_stack,
        ))
        .child(
            div()
                .id("stack-nav-scroll")
                .h(stack_timeline_height)
                .max_h(px(220.0))
                .flex_shrink_0()
                .min_h_0()
                .flex()
                .flex_col()
                .px(px(14.0))
                .pb(px(8.0))
                .child(
                    list(list_state, {
                        let state = state.clone();
                        let detail = Arc::new(detail.clone());
                        let review_stack = review_stack.clone();
                        let selected_layer_id = selected_layer_id.clone();
                        let trunk_branch = trunk_branch.clone();
                        move |ix, _window, _cx| {
                            let layer_count = review_stack.layers.len();
                            if ix < layer_count {
                                let layer = &review_stack.layers[layer_count - ix - 1];
                                render_stack_view_layer_card(
                                    &state,
                                    detail.as_ref(),
                                    review_stack.as_ref(),
                                    layer,
                                    selected_layer_id.as_deref() == Some(layer.id.as_str()),
                                    ix > 0,
                                    true,
                                )
                                .into_any_element()
                            } else {
                                render_stack_base_branch_row(&trunk_branch, layer_count > 0)
                                    .into_any_element()
                            }
                        }
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                ),
        )
        .child(render_stack_file_tree_section(
            state,
            tree_rows,
            file_tree_list_state,
            selected_path.as_deref(),
            file_tree_label,
            visible_file_count,
            visible_additions,
            visible_deletions,
        ))
}

fn render_stack_file_tree_section(
    state: &Entity<AppState>,
    tree_rows: Arc<Vec<ReviewFileTreeRow>>,
    list_state: ListState,
    selected_path: Option<&str>,
    file_tree_label: &str,
    visible_file_count: usize,
    visible_additions: i64,
    visible_deletions: i64,
) -> impl IntoElement {
    let selected_path = selected_path.map(str::to_string);
    let on_file_open = file_tree_row_open_handler();

    div()
        .flex_grow()
        .min_h_0()
        .flex()
        .flex_col()
        .border_t(px(1.0))
        .border_color(diff_annotation_border())
        .child(render_file_tree_header(
            state.clone(),
            file_tree_label,
            visible_file_count,
            Some((visible_additions, visible_deletions)),
        ))
        .child(
            div()
                .id("stack-file-tree-scroll")
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                .px(px(6.0))
                .py(px(6.0))
                .child(if tree_rows.is_empty() {
                    div()
                        .px(px(8.0))
                        .py(px(8.0))
                        .text_size(px(11.0))
                        .line_height(px(16.0))
                        .text_color(fg_muted())
                        .child("No files in this stack slice.")
                        .into_any_element()
                } else {
                    list(list_state, {
                        let state = state.clone();
                        let tree_rows = tree_rows.clone();
                        let selected_path = selected_path.clone();
                        let on_file_open = on_file_open.clone();
                        move |ix, _window, cx| match tree_rows[ix].clone() {
                            ReviewFileTreeRow::Directory { name, depth } => {
                                render_file_tree_directory_row(name, depth).into_any_element()
                            }
                            ReviewFileTreeRow::File {
                                path,
                                name,
                                depth,
                                additions,
                                deletions,
                            } => {
                                let is_reviewed = state.read(cx).is_review_file_reviewed(&path);
                                render_file_tree_file_row(
                                    state.clone(),
                                    path,
                                    name,
                                    additions,
                                    deletions,
                                    depth,
                                    selected_path.as_deref(),
                                    ReviewFileRowOpenMode::Stack,
                                    is_reviewed,
                                    on_file_open.clone(),
                                )
                                .into_any_element()
                            }
                        }
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0()
                    .into_any_element()
                }),
        )
}

fn render_stack_view_header(progress_label: String, draft: bool) -> impl IntoElement {
    let label = if draft { "GUIDE DRAFT" } else { "GUIDE" };
    div()
        .px(px(14.0))
        .pt(px(20.0))
        .pb(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_family(mono_font_family())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_muted())
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_family(mono_font_family())
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg_muted())
                .child(progress_label),
        )
}

fn render_stack_view_layer_card(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    stack: &ReviewStack,
    layer: &ReviewStackLayer,
    is_active: bool,
    connector_above: bool,
    connector_below: bool,
) -> impl IntoElement {
    let state_for_open = state.clone();
    let layer_id = layer.id.clone();
    let first_file = first_changed_file_for_stack_layer(stack, layer, detail);
    let route_summary = stack_layer_pull_request_summary(&detail.repository, detail.number, layer);
    let (number_label, title_label) = stack_view_layer_title_parts(layer);
    let row_bg = if is_active {
        bg_emphasis()
    } else {
        transparent()
    };
    let hover_bg = if is_active {
        bg_emphasis()
    } else {
        bg_selected()
    };

    div()
        .w_full()
        .h(px(36.0))
        .relative()
        .child(
            div()
                .w_full()
                .h(px(32.0))
                .px(px(8.0))
                .rounded(px(4.0))
                .bg(row_bg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    open_stack_layer(
                        &state_for_open,
                        route_summary.clone(),
                        layer_id.clone(),
                        first_file.clone(),
                        window,
                        cx,
                    );
                })
                .child(
                    div()
                        .h_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(lucide_icon(LucideIcon::GitBranch, 13.0, success()))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .when_some(number_label, |el, label| {
                                    el.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(12.0))
                                            .font_family(mono_font_family())
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if is_active {
                                                fg_default()
                                            } else {
                                                fg_muted()
                                            })
                                            .child(label),
                                    )
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(12.0))
                                        .font_weight(if is_active {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(if is_active {
                                            fg_emphasis()
                                        } else {
                                            fg_muted()
                                        })
                                        .whitespace_nowrap()
                                        .overflow_x_hidden()
                                        .text_ellipsis()
                                        .child(title_label),
                                ),
                        ),
                ),
        )
        .when(connector_above, |el| {
            el.child(render_stack_timeline_segment(0.0, 8.0))
        })
        .when(connector_below, |el| {
            el.child(render_stack_timeline_segment(24.0, 12.0))
        })
}

fn stack_view_layer_title_parts(layer: &ReviewStackLayer) -> (Option<String>, String) {
    if let Some(pr) = layer.pr.as_ref() {
        let number = format!("#{}", pr.number);
        let title = layer
            .title
            .strip_prefix(number.as_str())
            .map(str::trim_start)
            .filter(|title| !title.is_empty())
            .unwrap_or(layer.title.as_str())
            .to_string();
        return (Some(number), title);
    }

    (None, layer.title.clone())
}

fn render_stack_base_branch_row(branch_name: &str, connector_above: bool) -> impl IntoElement {
    div()
        .w_full()
        .h(px(26.0))
        .relative()
        .when(connector_above, |el| {
            el.child(render_stack_timeline_segment(0.0, 6.0))
        })
        .child(
            div()
                .h_full()
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(lucide_icon(LucideIcon::Circle, 13.0, fg_subtle()))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_muted())
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(branch_name.to_string()),
                ),
        )
}

fn render_stack_timeline_segment(top: f32, height: f32) -> impl IntoElement {
    div()
        .absolute()
        .left(px(14.0))
        .top(px(top))
        .w(px(2.0))
        .h(px(height))
        .bg(bg_selected())
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

fn prepare_stack_timeline_list_state(app_state: &AppState) -> ListState {
    let state_key = stack_timeline_state_key(app_state);
    let mut list_states = app_state.review_nav_list_states.borrow_mut();
    list_states
        .entry(state_key)
        .or_insert_with(|| ListState::new(0, ListAlignment::Top, px(36.0)))
        .clone()
}

pub(super) fn reset_stack_timeline_list_state(app_state: &AppState) {
    let state_key = stack_timeline_state_key(app_state);
    app_state
        .review_nav_list_states
        .borrow_mut()
        .remove(&state_key);
}

fn stack_timeline_state_key(app_state: &AppState) -> String {
    format!(
        "{}:stack-timeline",
        app_state.active_pr_key.as_deref().unwrap_or("detached"),
    )
}

pub(super) fn sync_stack_timeline_item_count(list_state: &ListState, item_count: usize) {
    if list_state.item_count() == item_count {
        return;
    }

    let should_scroll_to_bottom = list_state.item_count() == 0 && item_count > 0;
    if should_scroll_to_bottom {
        list_state.reset(item_count);
        list_state.scroll_to(ListOffset {
            item_ix: item_count,
            offset_in_item: px(0.0),
        });
    } else {
        reset_list_state_preserving_scroll(list_state, item_count);
    }
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
                state.read(cx).is_review_file_reviewed(&file.path),
                file_tree_row_open_handler(),
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
        .bg(if is_selected {
            bg_emphasis()
        } else {
            bg_overlay()
        })
        .cursor_pointer()
        .hover(move |style| {
            style.bg(if is_selected {
                bg_emphasis()
            } else {
                bg_selected()
            })
        })
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
        .bg(bg_overlay())
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
        .bg(bg_overlay())
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
        .bg(bg_overlay())
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
        .bg(bg_overlay())
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

pub(super) fn open_review_location_card(
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
        ReviewCenterMode::StructuralDiff => {
            state.update(cx, |state, cx| {
                state.navigate_to_review_location(location.clone(), true);
                state.persist_active_review_session();
                cx.notify();
            });
            ensure_active_review_focus_loaded(state, window, cx);
        }
        ReviewCenterMode::AiTour | ReviewCenterMode::Stack | ReviewCenterMode::GuidedReview => {
            let mut location = location.clone();
            location.mode = ReviewCenterMode::GuidedReview;
            state.update(cx, |state, cx| {
                state.navigate_to_review_location(location, true);
                state.persist_active_review_session();
                cx.notify();
            });
            ensure_active_review_focus_loaded(state, window, cx);
            review_intelligence::trigger_review_intelligence(
                state,
                window,
                cx,
                review_intelligence::ReviewIntelligenceScope::StackOnly,
                false,
            );
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

pub(super) fn default_waymark_name(
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

pub(super) fn metric_pill(
    label: impl Into<String>,
    fg: gpui::Rgba,
    bg: gpui::Rgba,
) -> impl IntoElement {
    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(transparent())
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
        (
            crate::stacks::model::StackKind::Virtual,
            crate::stacks::model::StackSource::VirtualSemantic,
        ) if stack
            .provider
            .as_ref()
            .is_some_and(|provider| provider.provider == "sem_virtual_stack") =>
        {
            format!("Virtual stack · Sem · {} layers", stack.layers.len())
        }
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
        (
            crate::stacks::model::StackKind::Virtual,
            crate::stacks::model::StackSource::VirtualSemantic,
        ) if stack
            .provider
            .as_ref()
            .is_some_and(|provider| provider.provider == "sem_virtual_stack") =>
        {
            "Generated locally from Sem semantic evidence"
        }
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
                    .bg(bg_overlay())
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
        .border_color(transparent())
        .bg(bg_emphasis())
        .when(can_expand, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(bg_emphasis()))
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
                                .border_color(transparent())
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
    let first_file = first_changed_file_for_stack_layer(stack, layer, detail);
    let route_summary = stack_layer_pull_request_summary(&detail.repository, detail.number, layer);
    let line_count = layer.metrics.changed_lines;
    let thread_count = layer.metrics.unresolved_thread_count;
    let confidence = layer.confidence;

    div()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if is_active {
            bg_emphasis()
        } else {
            bg_surface()
        })
        .cursor_pointer()
        .hover(move |style| {
            style.bg(if is_active {
                bg_emphasis()
            } else {
                bg_selected()
            })
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_stack_layer(
                &state_for_open,
                route_summary.clone(),
                layer_id.clone(),
                first_file.clone(),
                window,
                cx,
            );
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
                                .border_color(transparent())
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
        .border_color(transparent())
        .text_size(px(9.0))
        .font_family(mono_font_family())
        .text_color(fg_muted())
        .child(label.to_string())
}

fn open_stack_layer(
    state: &Entity<AppState>,
    route_summary: Option<github::PullRequestSummary>,
    layer_id: String,
    first_file: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(summary) = route_summary {
        let summary_repository = summary.repository.clone();
        let open_pull_requests = {
            let state = state.read(cx);
            state
                .active_detail()
                .filter(|detail| detail.repository == summary_repository)
                .and_then(|_| state.active_detail_state())
                .and_then(|detail_state| detail_state.stack_open_pull_requests.clone())
        };

        super::super::sections::open_pull_request(state, summary, window, cx);

        state.update(cx, |state, cx| {
            state.active_surface = PullRequestSurface::Files;
            state.pr_header_compact = false;
            state.set_review_file_tree_visible(true);
            state.set_review_center_mode(ReviewCenterMode::GuidedReview);
            state.set_selected_stack_layer(Some(layer_id));
            state.set_stack_diff_mode(StackDiffMode::CurrentLayerOnly);
            state.selected_file_path = None;
            state.selected_diff_anchor = None;
            if let (Some(detail_key), Some(open_pull_requests)) =
                (state.active_pr_key.clone(), open_pull_requests)
            {
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    detail_state.stack_open_pull_requests = Some(open_pull_requests);
                    detail_state.stack_open_pull_requests_loading = false;
                    detail_state.stack_open_pull_requests_error = None;
                }
                state.review_stack_cache.borrow_mut().clear();
            }
            state.ensure_active_selected_file_is_valid();
            state.persist_active_review_session();
            cx.notify();
        });

        ensure_active_stack_refs_loaded(state, window, cx);
        ensure_active_review_focus_loaded(state, window, cx);
        ensure_selected_file_content_loaded(state, window, cx);
        return;
    }

    state.update(cx, |state, cx| {
        state.set_selected_stack_layer(Some(layer_id));
        state.set_stack_diff_mode(StackDiffMode::CurrentLayerOnly);
        state.set_review_center_mode(ReviewCenterMode::GuidedReview);
        if let Some(path) = first_file {
            state.selected_file_path = Some(path);
            state.selected_diff_anchor = None;
        }
        state.ensure_active_selected_file_is_valid();
        state.persist_active_review_session();
        cx.notify();
    });
    ensure_active_review_focus_loaded(state, window, cx);
    ensure_selected_file_content_loaded(state, window, cx);
}

fn stack_layer_pull_request_summary(
    current_repository: &str,
    current_number: i64,
    layer: &ReviewStackLayer,
) -> Option<github::PullRequestSummary> {
    let pr = layer.pr.as_ref()?;
    if pr.repository == current_repository && pr.number == current_number {
        return None;
    }

    Some(github::PullRequestSummary {
        local_key: None,
        repository: pr.repository.clone(),
        number: pr.number,
        title: pr.title.clone(),
        author_login: "unknown".to_string(),
        author_avatar_url: None,
        is_draft: pr.is_draft,
        comments_count: 0,
        additions: layer.metrics.additions as i64,
        deletions: layer.metrics.deletions as i64,
        changed_files: layer.metrics.file_count as i64,
        state: pr.state.clone(),
        review_decision: pr.review_decision.clone(),
        updated_at: String::new(),
        url: pr.url.clone(),
    })
}

fn first_changed_file_for_stack_layer(
    stack: &ReviewStack,
    layer: &ReviewStackLayer,
    detail: &PullRequestDetail,
) -> Option<String> {
    stack
        .atoms_for_layer(layer)
        .into_iter()
        .find(|atom| detail.files.iter().any(|file| file.path == atom.path))
        .map(|atom| atom.path.clone())
}

fn render_source_file_tree(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    cx: &App,
) -> impl IntoElement {
    let (tree_rows, visible_file_count, loading, error, visible_additions, visible_deletions) = {
        let app_state = state.read(cx);
        let (changed_file_count, additions, deletions) = review_file_tree_totals(detail, None);
        let source_tree = app_state
            .active_detail_state()
            .map(|detail_state| detail_state.source_file_tree.clone())
            .unwrap_or_default();

        (
            source_tree.rows,
            if source_tree.file_count > 0 {
                source_tree.file_count
            } else {
                changed_file_count
            },
            source_tree.loading,
            source_tree.error,
            additions,
            deletions,
        )
    };
    let list_state = {
        let app_state = state.read(cx);
        prepare_review_file_tree_list_state_for_scope(&app_state, "source-file-tree")
    };
    let tree_row_count = tree_rows.as_ref().map(|rows| rows.len()).unwrap_or(0);
    if list_state.item_count() != tree_row_count {
        list_state.reset(tree_row_count);
    }
    let selected_path = selected_path.map(str::to_string);
    let diff_totals = (visible_additions != 0 || visible_deletions != 0)
        .then_some((visible_additions, visible_deletions));
    let status_message = error
        .as_ref()
        .map(|error| (error.clone(), true))
        .unwrap_or_else(|| {
            if loading {
                (
                    "Loading repository files from the local checkout...".to_string(),
                    false,
                )
            } else {
                (
                    "Repository files will appear after the local checkout is ready.".to_string(),
                    false,
                )
            }
        });
    let on_file_open = file_tree_row_open_handler();

    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_r(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(render_file_tree_header(
            state.clone(),
            "Repository",
            visible_file_count,
            diff_totals,
        ))
        .child(
            div()
                .id("file-tree-scroll")
                .flex_grow()
                .min_h_0()
                .flex()
                .flex_col()
                .px(px(6.0))
                .py(px(6.0))
                .child(if let Some(tree_rows) = tree_rows {
                    list(list_state, {
                        let state = state.clone();
                        let tree_rows = tree_rows.clone();
                        let selected_path = selected_path.clone();
                        let on_file_open = on_file_open.clone();
                        move |ix, _window, cx| match tree_rows[ix].clone() {
                            ReviewFileTreeRow::Directory { name, depth } => {
                                render_file_tree_directory_row(name, depth).into_any_element()
                            }
                            ReviewFileTreeRow::File {
                                path,
                                name,
                                depth,
                                additions,
                                deletions,
                            } => {
                                let is_reviewed = state.read(cx).is_review_file_reviewed(&path);
                                render_file_tree_file_row(
                                    state.clone(),
                                    path,
                                    name,
                                    additions,
                                    deletions,
                                    depth,
                                    selected_path.as_deref(),
                                    ReviewFileRowOpenMode::Source,
                                    is_reviewed,
                                    on_file_open.clone(),
                                )
                                .into_any_element()
                            }
                        }
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0()
                    .into_any_element()
                } else {
                    render_file_tree_state_message(status_message.0, status_message.1)
                        .into_any_element()
                }),
        )
}
fn prepare_review_file_tree_list_state_for_scope(app_state: &AppState, scope: &str) -> ListState {
    let key = review_cache_key(app_state.active_pr_key.as_deref(), scope);
    let mut list_states = app_state.review_file_tree_list_states.borrow_mut();
    list_states
        .entry(key)
        .or_insert_with(|| ListState::new(0, ListAlignment::Top, px(REVIEW_FILE_TREE_ROW_HEIGHT)))
        .clone()
}

pub(super) fn prepare_review_file_tree_rows(
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

fn review_file_tree_label(mode: StackDiffMode) -> &'static str {
    match mode {
        StackDiffMode::WholePr => "Files",
        StackDiffMode::CurrentLayerOnly => "Layer Files",
        StackDiffMode::UpToCurrentLayer => "Stack Files",
        StackDiffMode::CurrentAndDependents => "Dependent Files",
        StackDiffMode::SinceLastReviewed => "Unreviewed Files",
    }
}

pub(super) fn stack_file_paths_for_filter(
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
