use super::*;

pub(super) fn render_guided_review_document_view(
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
        guided_review_section_list_state,
    ) = {
        let app_state = state.read(cx);
        let detail_state = app_state.active_detail_state();
        let guided_review_state = app_state.active_guided_review_state();

        (
            app_state.selected_review_ai_provider(),
            app_state.selected_review_ai_provider_status().cloned(),
            app_state.review_ai_provider_loading,
            app_state.review_ai_provider_error.clone(),
            detail_state
                .map(|state| state.local_repository_loading)
                .unwrap_or(false),
            detail_state.and_then(|state| state.local_repository_error.clone()),
            guided_review_state
                .map(|state| state.loading)
                .unwrap_or(false),
            guided_review_state
                .map(|state| state.generating)
                .unwrap_or(false),
            guided_review_state.and_then(|state| state.progress_summary.clone()),
            guided_review_state.and_then(|state| state.progress_detail.clone()),
            guided_review_state.and_then(|state| state.error.clone()),
            guided_review_state.and_then(|state| state.message.clone()),
            guided_review_state
                .map(|state| state.success)
                .unwrap_or(false),
            guided_review_state.and_then(|state| state.document.clone()),
            app_state.guided_review_section_list_state.clone(),
        )
    };

    let state_for_generate = state.clone();
    let generate_label =
        guided_review_generate_label(provider, generated_tour.as_ref(), tour_generating);
    let has_status_messages = provider_error.is_some()
        || local_repo_error.is_some()
        || tour_error.is_some()
        || tour_message.is_some();

    let shell = div().flex_grow().min_h_0().flex().flex_col().bg(bg_inset());

    match generated_tour {
        Some(guided_review) => {
            let section_count = guided_review.sections.len();
            let mut items = Vec::new();
            if section_count > 0 {
                items.push(GuidedReviewContentItem::SemanticOverview);
            }
            if tour_generating || tour_loading || provider_loading {
                items.push(GuidedReviewContentItem::Progress);
            }
            if has_status_messages {
                items.push(GuidedReviewContentItem::StatusMessages);
            }
            if section_count == 0 {
                items.push(GuidedReviewContentItem::Empty);
            } else {
                items.extend((0..section_count).map(GuidedReviewContentItem::Section));
            }
            items.push(GuidedReviewContentItem::Spacer);

            if guided_review_section_list_state.item_count() != items.len() {
                guided_review_section_list_state.reset(items.len());
            }
            let section_targets = items
                .iter()
                .enumerate()
                .filter_map(|(item_ix, item)| match item {
                    GuidedReviewContentItem::Section(section_ix) => Some((*section_ix, item_ix)),
                    _ => None,
                })
                .collect::<Vec<_>>();

            let guided_review = Arc::new(guided_review);
            let detail = Arc::new(detail.clone());
            let state_for_sections = state.clone();
            let tour_for_sections = guided_review.clone();
            let detail_for_sections = detail.clone();
            let list_state_for_overview = guided_review_section_list_state.clone();
            let tour_for_overview = guided_review.clone();
            let section_targets_for_overview = Arc::new(section_targets);
            let items = Arc::new(items);

            shell
                .child(
                    list(
                        guided_review_section_list_state.clone(),
                        move |ix, _window, cx| match items[ix] {
                            GuidedReviewContentItem::SemanticOverview => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_guided_review_semantic_overview(
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
                                            trigger_generate_guided_review(&state, window, cx, false)
                                        }
                                    },
                                ))
                                .into_any_element(),
                            GuidedReviewContentItem::Pending => div().into_any_element(),
                            GuidedReviewContentItem::Progress => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_guided_review_progress_panel(
                                    provider,
                                    provider_loading,
                                    tour_loading,
                                    tour_generating,
                                    tour_progress_summary.as_deref(),
                                    tour_progress_detail.as_deref(),
                                ))
                                .into_any_element(),
                            GuidedReviewContentItem::StatusMessages => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_guided_review_status_messages(
                                    provider_error.as_deref(),
                                    local_repo_error.as_deref(),
                                    tour_error.as_deref(),
                                    tour_message.as_deref(),
                                    tour_success,
                                ))
                                .into_any_element(),
                            GuidedReviewContentItem::Empty => div()
                                .when(ix == 0, |el| el.pt(px(18.0)))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(nested_panel().child(panel_state_text(
                                    "No Guided Review sections were returned for this pull request.",
                                )))
                                .into_any_element(),
                            GuidedReviewContentItem::Section(section_ix) => {
                                let section = &tour_for_sections.sections[section_ix];
                                div()
                                    .px(px(18.0))
                                    .pb(px(14.0))
                                    .child(render_guided_review_section(
                                        &state_for_sections,
                                        detail_for_sections.as_ref(),
                                        tour_for_sections.as_ref(),
                                        section,
                                        cx,
                                    ))
                                    .into_any_element()
                            }
                            GuidedReviewContentItem::Spacer => {
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
            let mut items = vec![GuidedReviewContentItem::Pending];
            if has_status_messages {
                items.push(GuidedReviewContentItem::StatusMessages);
            }
            items.push(GuidedReviewContentItem::Spacer);

            if guided_review_section_list_state.item_count() != items.len() {
                guided_review_section_list_state.reset(items.len());
            }

            let items = Arc::new(items);

            shell
                .child(
                    list(
                        guided_review_section_list_state.clone(),
                        move |ix, _window, _cx| match items[ix] {
                            GuidedReviewContentItem::Pending => div()
                                .pt(px(18.0))
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_guided_review_pending_panel(
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
                                            trigger_generate_guided_review(
                                                &state, window, cx, false,
                                            )
                                        }
                                    },
                                ))
                                .into_any_element(),
                            GuidedReviewContentItem::StatusMessages => div()
                                .px(px(18.0))
                                .pb(px(14.0))
                                .child(render_guided_review_status_messages(
                                    provider_error.as_deref(),
                                    local_repo_error.as_deref(),
                                    tour_error.as_deref(),
                                    tour_message.as_deref(),
                                    tour_success,
                                ))
                                .into_any_element(),
                            GuidedReviewContentItem::Spacer => {
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
enum GuidedReviewContentItem {
    SemanticOverview,
    Pending,
    Progress,
    StatusMessages,
    Empty,
    Section(usize),
    Spacer,
}

fn guided_review_generate_label(
    provider: ReviewAiProvider,
    generated_tour: Option<&GeneratedGuidedReview>,
    generating: bool,
) -> String {
    if generating {
        format!("Generating with {}...", provider.label())
    } else if generated_tour
        .map(|guided_review| guided_review.provider == provider)
        .unwrap_or(false)
    {
        format!("Regenerate with {}", provider.label())
    } else {
        format!("Generate with {}", provider.label())
    }
}

fn guided_review_provider_status_label(status: &ReviewAiProviderStatus) -> &'static str {
    if status.available && status.authenticated {
        "ready"
    } else if status.available {
        "needs auth"
    } else {
        "unavailable"
    }
}

fn render_guided_review_pending_panel(
    provider: ReviewAiProvider,
    provider_status: Option<&ReviewAiProviderStatus>,
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
                .unwrap_or_else(|| "Looking for cached Guided Review".to_string()),
            progress_detail.map(str::to_string).unwrap_or_else(|| {
                "The app is checking whether this pull request head already has a stored Guided Review."
                    .to_string()
            }),
        )
    } else if tour_generating {
        (
            progress_summary
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} is building Guided Review", provider.label())),
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
                "Generate Guided Review".to_string(),
                "Create a short guided walkthrough that groups related changes and shows the matching diff under each explanation.".to_string(),
            )
        }
    } else {
        (
            "Preparing Guided Review".to_string(),
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
                    .child(eyebrow("Guided Review"))
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
                        el.child(badge(guided_review_provider_status_label(status)))
                    })
                    .when(provider_loading, |el| el.child(badge("Checking provider")))
                    .when(local_repo_loading, |el| {
                        el.child(badge("Preparing checkout"))
                    })
                    .child(review_button(generate_label, on_generate)),
            ),
    )
}

fn render_guided_review_progress_panel(
    provider: ReviewAiProvider,
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
            .unwrap_or_else(|| "Loading cached Guided Review".to_string())
    } else if tour_generating {
        progress_summary
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} is updating Guided Review", provider.label()))
    } else {
        "Preparing Guided Review".to_string()
    };
    let body = progress_detail
        .map(str::to_string)
        .unwrap_or_else(|| "Guided Review will update here when the provider returns.".to_string());

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

fn render_guided_review_status_messages(
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

fn render_guided_review_semantic_overview(
    guided_review: &GeneratedGuidedReview,
    provider: ReviewAiProvider,
    provider_status: Option<&ReviewAiProviderStatus>,
    local_repo_loading: bool,
    generate_label: &str,
    list_state: ListState,
    section_targets: Arc<Vec<(usize, usize)>>,
    on_generate: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .rounded(radius())
        .bg(bg_overlay())
        .child(
            div()
                .px(px(18.0))
                .py(px(14.0))
                .border_b(px(1.0))
                .border_color(border_muted())
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
                        .child(guided_review_metric_text(&format!(
                            "{} group{}",
                            guided_review.sections.len(),
                            if guided_review.sections.len() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )))
                        .when(!guided_review.open_questions.is_empty(), |el| {
                            el.child(guided_review_metric_text(&format!(
                                "{} open question{}",
                                guided_review.open_questions.len(),
                                if guided_review.open_questions.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            )))
                        })
                        .when(!guided_review.warnings.is_empty(), |el| {
                            el.child(guided_review_metric_text(&format!(
                                "{} warning{}",
                                guided_review.warnings.len(),
                                if guided_review.warnings.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            )))
                        })
                        .child(guided_review_metric_text(provider.label()))
                        .when_some(provider_status, |el, status| {
                            el.child(guided_review_metric_text(
                                guided_review_provider_status_label(status),
                            ))
                        })
                        .when(local_repo_loading, |el| {
                            el.child(guided_review_metric_text("Preparing checkout"))
                        })
                        .child(review_button(generate_label, on_generate)),
                ),
        )
        .child(
            div().p(px(14.0)).flex().flex_col().gap(px(8.0)).children(
                guided_review
                    .sections
                    .iter()
                    .enumerate()
                    .map(|(section_ix, section)| {
                        render_guided_review_semantic_overview_row(
                            guided_review,
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

fn render_guided_review_semantic_overview_row(
    guided_review: &GeneratedGuidedReview,
    section: &GuidedReviewSection,
    section_ix: usize,
    _show_divider: bool,
    list_state: ListState,
    section_targets: Arc<Vec<(usize, usize)>>,
) -> impl IntoElement {
    let metrics = guided_review_section_metrics(guided_review, section);
    let target_index = section_targets
        .iter()
        .find(|(candidate_ix, _)| *candidate_ix == section_ix)
        .map(|(_, item_ix)| *item_ix)
        .unwrap_or(0);

    div()
        .min_h(px(72.0))
        .rounded(radius_sm())
        .bg(bg_overlay())
        .flex()
        .hover(|style| style.bg(bg_subtle()))
        .on_mouse_down(MouseButton::Left, move |_, _, _| {
            list_state.scroll_to(ListOffset {
                item_ix: target_index,
                offset_in_item: px(0.0),
            });
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .min_w_0()
                .flex_grow()
                .p(px(12.0))
                .child(render_guided_review_category_icon(
                    section.category,
                    34.0,
                    17.0,
                ))
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
                                .child(render_guided_review_priority_chip(section.priority)),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .line_clamp(1)
                                .child(section.summary.clone()),
                        ),
                )
                .child(render_guided_review_section_metrics(metrics)),
        )
}

pub(super) fn render_guided_review_section_metrics(
    metrics: AiGuidedReviewSectionMetrics,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.0))
        .flex_wrap()
        .max_w(px(280.0))
        .child(guided_review_metric_text(&format!(
            "{} file{}",
            metrics.file_count,
            if metrics.file_count == 1 { "" } else { "s" }
        )))
        .child(guided_review_metric_text(&format!(
            "{} thread{}",
            metrics.unresolved_thread_count,
            if metrics.unresolved_thread_count == 1 {
                ""
            } else {
                "s"
            }
        )))
        .child(guided_review_delta_metric(
            metrics.additions,
            metrics.deletions,
        ))
}

pub(super) fn render_guided_review_category_icon(
    category: GuidedReviewSectionCategory,
    tile_size: f32,
    icon_size: f32,
) -> impl IntoElement {
    div()
        .w(px(tile_size))
        .h(px(tile_size))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(guided_review_category_bg(category))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(lucide_icon(
            guided_review_category_lucide_icon(category),
            icon_size,
            guided_review_category_fg(category),
        ))
}

pub(super) fn render_guided_review_priority_chip(
    priority: GuidedReviewSectionPriority,
) -> impl IntoElement {
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(guided_review_priority_bg(priority))
        .border_1()
        .border_color(transparent())
        .flex_shrink_0()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .font_family(mono_font_family())
        .text_color(guided_review_priority_fg(priority))
        .child(priority.label())
}

pub(super) fn guided_review_metric_text(text: &str) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .font_family(mono_font_family())
        .text_color(fg_muted())
        .whitespace_nowrap()
        .child(text.to_string())
}

pub(super) fn guided_review_delta_metric(additions: i64, deletions: i64) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .text_size(px(11.0))
        .font_family(mono_font_family())
        .whitespace_nowrap()
        .child(div().text_color(success()).child(format!("+{additions}")))
        .child(div().text_color(fg_subtle()).child("/"))
        .child(div().text_color(danger()).child(format!("-{deletions}")))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct AiGuidedReviewSectionMetrics {
    pub(super) file_count: usize,
    pub(super) additions: i64,
    pub(super) deletions: i64,
    pub(super) unresolved_thread_count: i64,
}

pub(super) fn guided_review_section_metrics(
    guided_review: &GeneratedGuidedReview,
    section: &GuidedReviewSection,
) -> AiGuidedReviewSectionMetrics {
    let mut metrics = AiGuidedReviewSectionMetrics::default();

    for step_id in &section.step_ids {
        if let Some(step) = guided_review.steps.iter().find(|step| step.id == *step_id) {
            metrics.file_count += 1;
            metrics.additions += step.additions;
            metrics.deletions += step.deletions;
            metrics.unresolved_thread_count += step.unresolved_thread_count;
        }
    }

    metrics
}

fn guided_review_category_lucide_icon(category: GuidedReviewSectionCategory) -> LucideIcon {
    match category {
        GuidedReviewSectionCategory::AuthSecurity => LucideIcon::ShieldCheck,
        GuidedReviewSectionCategory::DataState => LucideIcon::Database,
        GuidedReviewSectionCategory::ApiIo => LucideIcon::Plug,
        GuidedReviewSectionCategory::UiUx => LucideIcon::Palette,
        GuidedReviewSectionCategory::Tests => LucideIcon::FlaskConical,
        GuidedReviewSectionCategory::Docs => LucideIcon::BookOpenText,
        GuidedReviewSectionCategory::Config => LucideIcon::SlidersHorizontal,
        GuidedReviewSectionCategory::Infra => LucideIcon::ServerCog,
        GuidedReviewSectionCategory::Refactor => LucideIcon::GitCompareArrows,
        GuidedReviewSectionCategory::Performance => LucideIcon::Gauge,
        GuidedReviewSectionCategory::Reliability => LucideIcon::BadgeCheck,
        GuidedReviewSectionCategory::Other => LucideIcon::CircleHelp,
    }
}

fn guided_review_category_fg(category: GuidedReviewSectionCategory) -> Rgba {
    match category {
        GuidedReviewSectionCategory::AuthSecurity => danger(),
        GuidedReviewSectionCategory::DataState => accent(),
        GuidedReviewSectionCategory::ApiIo => warning(),
        GuidedReviewSectionCategory::UiUx => fg_emphasis(),
        GuidedReviewSectionCategory::Tests => success(),
        GuidedReviewSectionCategory::Docs => fg_muted(),
        GuidedReviewSectionCategory::Config => warning(),
        GuidedReviewSectionCategory::Infra => accent(),
        GuidedReviewSectionCategory::Refactor => fg_default(),
        GuidedReviewSectionCategory::Performance => warning(),
        GuidedReviewSectionCategory::Reliability => success(),
        GuidedReviewSectionCategory::Other => fg_muted(),
    }
}

fn guided_review_category_bg(category: GuidedReviewSectionCategory) -> Rgba {
    match category {
        GuidedReviewSectionCategory::AuthSecurity => danger_muted(),
        GuidedReviewSectionCategory::DataState => accent_muted(),
        GuidedReviewSectionCategory::ApiIo => warning_muted(),
        GuidedReviewSectionCategory::UiUx => bg_emphasis(),
        GuidedReviewSectionCategory::Tests => success_muted(),
        GuidedReviewSectionCategory::Docs => bg_subtle(),
        GuidedReviewSectionCategory::Config => warning_muted(),
        GuidedReviewSectionCategory::Infra => accent_muted(),
        GuidedReviewSectionCategory::Refactor => bg_subtle(),
        GuidedReviewSectionCategory::Performance => warning_muted(),
        GuidedReviewSectionCategory::Reliability => success_muted(),
        GuidedReviewSectionCategory::Other => bg_subtle(),
    }
}

fn guided_review_priority_fg(priority: GuidedReviewSectionPriority) -> Rgba {
    match priority {
        GuidedReviewSectionPriority::Low => success(),
        GuidedReviewSectionPriority::Medium => warning(),
        GuidedReviewSectionPriority::High => danger(),
    }
}

fn guided_review_priority_bg(priority: GuidedReviewSectionPriority) -> Rgba {
    match priority {
        GuidedReviewSectionPriority::Low => success_muted(),
        GuidedReviewSectionPriority::Medium => warning_muted(),
        GuidedReviewSectionPriority::High => danger_muted(),
    }
}

fn render_guided_review_section(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    generated_tour: &GeneratedGuidedReview,
    section: &GuidedReviewSection,
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
    let metrics = guided_review_section_metrics(generated_tour, section);

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
                                .child(render_guided_review_category_icon(
                                    section.category,
                                    34.0,
                                    17.0,
                                ))
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
                        .gap(px(8.0))
                        .flex_wrap()
                        .child(render_guided_review_priority_chip(section.priority))
                        .child(guided_review_metric_text(&format!(
                            "{} file{}",
                            metrics.file_count,
                            if metrics.file_count == 1 { "" } else { "s" }
                        )))
                        .child(guided_review_delta_metric(
                            metrics.additions,
                            metrics.deletions,
                        )),
                ),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(fg_default())
                .mt(px(10.0))
                .child(SelectableText::new(
                    format!("guided-review-section-summary-{}", section.id),
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
                        format!("guided-review-section-detail-{}", section.id),
                        section.detail.clone(),
                    )),
            )
        })
        .when(!section.review_points.is_empty(), |el| {
            el.child(render_guided_review_points(
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
                    render_guided_review_step_diff(state, detail, section, step, cx)
                        .into_any_element()
                })),
        )
}

fn render_guided_review_points(section_id: &str, review_points: &[String]) -> impl IntoElement {
    div().mt(px(12.0)).flex().flex_col().gap(px(8.0)).children(
        review_points.iter().enumerate().map(|(point_ix, point)| {
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(SelectableText::new(
                    format!("guided-review-point-{section_id}-{point_ix}"),
                    point.clone(),
                ))
        }),
    )
}

fn render_guided_review_step_diff(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    section: &GuidedReviewSection,
    step: &GuidedReviewStep,
    cx: &App,
) -> impl IntoElement {
    let preview_key = format!("guided-review:{}:{}", section.id, step.id);

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
                                    format!("guided-review-step-summary-{}", step.id),
                                    step.summary.clone(),
                                ),
                            ))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .flex_wrap()
                        .child(guided_review_delta_metric(step.additions, step.deletions))
                        .when(step.unresolved_thread_count > 0, |el| {
                            el.child(guided_review_metric_text(&format!(
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
