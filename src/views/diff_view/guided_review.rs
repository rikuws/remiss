use super::*;

pub(super) fn render_guided_review_view(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
    review_stack: Arc<ReviewStack>,
    stack_filter: Option<LayerDiffFilter>,
    guided_review_lens: ReviewGuideLens,
    normal_diff_layout: DiffLayout,
    structural_diff_layout: DiffLayout,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    let diff_center_mode = if guided_review_lens == ReviewGuideLens::Structural {
        ReviewCenterMode::StructuralDiff
    } else {
        ReviewCenterMode::Stack
    };
    let diff_layout = if guided_review_lens == ReviewGuideLens::Structural {
        structural_diff_layout
    } else {
        normal_diff_layout
    };

    div()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .flex()
        .child(
            render_combined_diff_files(
                state,
                app_state,
                detail,
                selected_path,
                selected_anchor,
                review_stack.clone(),
                stack_filter,
                diff_center_mode,
                diff_layout,
                cx,
            )
            .into_any_element(),
        )
        .child(render_guided_review_panel(
            state,
            review_stack.as_ref(),
            window,
            cx,
        ))
        .into_any_element()
}

#[derive(Clone)]
struct GuidedReviewPanelResizeDrag {
    id: String,
    state: Entity<AppState>,
    start_pointer_x: Rc<RefCell<Option<Pixels>>>,
    start_width: f32,
}

impl GuidedReviewPanelResizeDrag {
    fn new(id: String, state: Entity<AppState>, start_width: f32) -> Self {
        Self {
            id,
            state,
            start_pointer_x: Rc::new(RefCell::new(None)),
            start_width,
        }
    }

    fn drag_to(&self, pointer_x: Pixels, window: &mut Window, cx: &mut App) {
        let start_pointer_x = {
            let mut start_pointer_x = self.start_pointer_x.borrow_mut();
            *start_pointer_x.get_or_insert(pointer_x)
        };
        let delta = f32::from(start_pointer_x - pointer_x);
        let width = (self.start_width + delta)
            .clamp(GUIDED_REVIEW_PANEL_MIN_WIDTH, GUIDED_REVIEW_PANEL_MAX_WIDTH);

        self.state.update(cx, |state, cx| {
            state.set_guided_review_panel_width(width);
            state.persist_active_review_session();
            cx.notify();
        });
        window.refresh();
    }
}

fn render_guided_review_panel(
    state: &Entity<AppState>,
    review_stack: &ReviewStack,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let (
        guide,
        loading,
        generating,
        progress_text,
        error,
        fallback_reason,
        focus_key,
        focus_label,
        focus_record,
        focus_loading,
        focus_error,
        provider,
        panel_width,
    ) = {
        let app_state = state.read(cx);
        let guide_state = app_state
            .active_detail_state()
            .map(|detail_state| detail_state.review_partner_state.clone())
            .unwrap_or_default();
        let panel_width = app_state
            .active_review_session()
            .map(|session| session.guided_review_panel_width)
            .unwrap_or(GUIDED_REVIEW_PANEL_DEFAULT_WIDTH);
        let selected_layer_id = app_state.active_review_session().and_then(|session| {
            review_stack
                .selected_layer(session.selected_stack_layer_id.as_deref())
                .map(|layer| layer.id.clone())
        });
        let selected_focus_target = guide_state.document.as_ref().and_then(|partner| {
            selected_layer_id.as_deref().and_then(|layer_id| {
                crate::review_partner::focus_target_for_layer(partner, layer_id)
            })
        });
        let focus_key = selected_focus_target
            .as_ref()
            .map(|target| target.key.clone())
            .or_else(|| guide_state.active_focus_key.clone())
            .or_else(|| {
                guide_state
                    .document
                    .as_ref()
                    .and_then(|partner| partner.focus_records.first())
                    .map(|record| record.key.clone())
            });
        let focus_record = guide_state.document.as_ref().and_then(|partner| {
            focus_key
                .as_deref()
                .and_then(|focus_key| partner.focus_record(focus_key))
                .cloned()
        });
        let focus_target = focus_record
            .as_ref()
            .map(|record| record.target.clone())
            .or(selected_focus_target)
            .or_else(|| {
                guide_state
                    .document
                    .as_ref()
                    .and_then(|partner| partner.focus_targets.first())
                    .cloned()
            });
        let focus_label = guide_state
            .active_focus_label
            .clone()
            .or_else(|| focus_target.as_ref().map(|target| target.subtitle.clone()))
            .or_else(|| focus_record.as_ref().map(|record| record.subtitle.clone()))
            .map(|label| review_partner_display_subtitle(&label));
        let focus_loading = focus_key
            .as_ref()
            .map(|key| guide_state.loading_focus_keys.contains(key))
            .unwrap_or(false);
        let focus_error = focus_key
            .as_ref()
            .and_then(|key| guide_state.focus_errors.get(key))
            .cloned();
        let fallback_reason = guide_state.document.as_ref().and_then(|document| {
            document.fallback_reason.clone().or_else(|| {
                document
                    .warnings
                    .iter()
                    .find(|warning| warning.contains("AI Review Partner context unavailable"))
                    .cloned()
            })
        });
        (
            guide_state.document.clone(),
            guide_state.loading,
            guide_state.generating,
            guide_state.progress_text,
            guide_state.error,
            fallback_reason,
            focus_key,
            focus_label,
            focus_record,
            focus_loading,
            focus_error,
            app_state.selected_tour_provider(),
            crate::review_session::sanitize_guided_review_panel_width(panel_width),
        )
    };
    let state_for_retry = state.clone();
    let resize_drag_id = "guided-review-panel-resize".to_string();
    let resize_drag =
        GuidedReviewPanelResizeDrag::new(resize_drag_id.clone(), state.clone(), panel_width);
    let resize_drag_id_for_move = resize_drag_id.clone();
    let activity_phase =
        (loading || generating || focus_loading).then(review_partner_activity_phase);
    if activity_phase.is_some() {
        window.request_animation_frame();
    }
    let panel_subtitle = provider.label().to_string();

    div()
        .relative()
        .w(px(panel_width))
        .flex_shrink_0()
        .min_h_0()
        .bg(diff_editor_chrome())
        .border_l(px(1.0))
        .border_color(diff_annotation_border())
        .flex()
        .flex_col()
        .child(
            div()
                .absolute()
                .left(px(-3.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(6.0))
                .id(ElementId::Name(resize_drag_id.into()))
                .cursor(CursorStyle::ResizeLeftRight)
                .on_drag(resize_drag, |_, _, _, cx| {
                    cx.new(|_| DiffScrollbarDragPreview)
                })
                .on_drag_move(
                    move |event: &DragMoveEvent<GuidedReviewPanelResizeDrag>, window, cx| {
                        let drag = event.drag(cx).clone();
                        if drag.id != resize_drag_id_for_move {
                            return;
                        }
                        drag.drag_to(event.event.position.x, window, cx);
                    },
                )
                .child(
                    div()
                        .absolute()
                        .left(px(2.0))
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .w(px(1.0))
                        .bg(diff_annotation_border()),
                ),
        )
        .child(
            div()
                .px(px(14.0))
                .py(px(11.0))
                .border_b(px(1.0))
                .border_color(diff_annotation_border())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Review Partner"),
                        )
                        .child(
                            div()
                                .mt(px(2.0))
                                .text_size(px(10.0))
                                .font_family(mono_font_family())
                                .text_color(fg_muted())
                                .child(panel_subtitle),
                        ),
                )
                .child(review_partner_action_icon_button(
                    LucideIcon::RefreshCw,
                    "Regenerate Review Partner",
                    move |_, window, cx| {
                        review_intelligence::trigger_review_intelligence(
                            &state_for_retry,
                            window,
                            cx,
                            review_intelligence::ReviewIntelligenceScope::StackOnly,
                            true,
                        );
                    },
                )),
        )
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .id("guided-review-scroll")
                .overflow_y_scroll()
                .px(px(14.0))
                .py(px(14.0))
                .flex()
                .flex_col()
                .gap(px(14.0))
                .when(loading || generating, |el| {
                    el.child(render_guided_review_partner_status(
                        "Preparing context",
                        progress_text.as_deref().unwrap_or(
                            "Checking usages, removed symbols, similar code, and stack context.",
                        ),
                        LucideIcon::Sparkles,
                        accent(),
                        activity_phase,
                    ))
                })
                .when_some(error.clone(), |el, error| {
                    el.child(render_guided_review_partner_status(
                        "Context unavailable",
                        &error,
                        LucideIcon::CircleHelp,
                        danger(),
                        None,
                    ))
                })
                .when_some(fallback_reason.clone(), |el, reason| {
                    el.child(render_guided_review_partner_status(
                        "Fallback context",
                        &reason,
                        LucideIcon::CircleHelp,
                        warning(),
                        None,
                    ))
                })
                .when_some(focus_record.as_ref(), |el, record| {
                    el.child(render_guided_review_focus_record(state, record, cx))
                })
                .when(
                    guide.is_some()
                        && focus_record.is_none()
                        && focus_key.is_some()
                        && (focus_loading || focus_error.is_none()),
                    |el| {
                        el.child(render_guided_review_partner_status(
                            "Generating focus context",
                            focus_label.as_deref().unwrap_or(
                                "Review Partner is preparing context for this stack layer.",
                            ),
                            LucideIcon::Sparkles,
                            accent(),
                            activity_phase,
                        ))
                    },
                )
                .when_some(focus_error.clone(), |el, error| {
                    el.child(render_guided_review_partner_status(
                        "Focus context unavailable",
                        &error,
                        LucideIcon::CircleHelp,
                        warning(),
                        None,
                    ))
                })
                .when(
                    guide.is_some() && focus_record.is_none() && focus_key.is_none(),
                    |el| {
                        el.child(render_guided_review_partner_status(
                            "No stack layer selected",
                            "Select a stack layer to load its explanation context.",
                            LucideIcon::Sparkles,
                            fg_muted(),
                            None,
                        ))
                    },
                )
                .when(
                    guide.is_none() && !loading && !generating && error.is_none(),
                    |el| {
                        el.child(render_guided_review_partner_status(
                            "No context yet",
                            "Review Partner context appears here after stack layers are prepared.",
                            LucideIcon::Sparkles,
                            fg_muted(),
                            None,
                        ))
                    },
                ),
        )
}

fn render_guided_review_partner_status(
    title: &str,
    message: &str,
    icon: LucideIcon,
    tone: Rgba,
    activity_phase: Option<f32>,
) -> impl IntoElement {
    div()
        .rounded(px(6.0))
        .bg(with_alpha(bg_overlay(), 0.62))
        .p(px(12.0))
        .flex()
        .gap(px(10.0))
        .items_start()
        .child(review_partner_icon_chip(icon, tone))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(fg_muted())
                        .child(message.to_string()),
                )
                .when_some(activity_phase, |el, phase| {
                    el.child(render_review_partner_activity_indicator(tone, phase))
                }),
        )
}

fn review_partner_activity_phase() -> f32 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    (elapsed % 1200) as f32 / 1200.0
}

fn render_review_partner_activity_indicator(tone: Rgba, phase: f32) -> impl IntoElement {
    div()
        .mt(px(2.0))
        .h(px(10.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .children((0..3).map(move |index| {
            div().size(px(4.0)).rounded(px(999.0)).bg(with_alpha(
                tone,
                review_partner_activity_dot_alpha(phase, index),
            ))
        }))
}

fn review_partner_activity_dot_alpha(phase: f32, index: usize) -> f32 {
    let dot_phase = (phase + 1.0 - (index as f32 * 0.18)).fract();
    let pulse = 1.0 - ((dot_phase - 0.5).abs() * 2.0).clamp(0.0, 1.0);
    0.24 + (pulse * 0.64)
}

fn render_guided_review_focus_record(
    state: &Entity<AppState>,
    record: &crate::review_partner::ReviewPartnerFocusRecord,
    cx: &App,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .pb(px(3.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .line_height(px(19.0))
                        .whitespace_normal()
                        .child(SelectableText::new(
                            format!("review-partner-focus-{}-title", record.key),
                            record.title.clone(),
                        )),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(fg_muted())
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(review_partner_display_subtitle(&record.subtitle)),
                ),
        )
        .child(render_review_partner_summary(record))
        .when(!record.usage_context.is_empty(), |el| {
            el.child(render_review_partner_usage_section(state, record, cx))
        })
        .when(
            !record.codebase_fit.follows && !record.codebase_fit.evidence.is_empty(),
            |el| {
                el.child(render_review_partner_codebase_fit_section(
                    state, record, cx,
                ))
            },
        )
        .children(
            record
                .sections
                .iter()
                .enumerate()
                .filter_map(|(index, section)| {
                    if section.items.is_empty() {
                        return None;
                    }
                    let (icon, tone) = review_partner_focus_section_style(&section.title);
                    Some(
                        render_review_partner_secondary_section(
                            state, record, index, section, icon, tone, cx,
                        )
                        .into_any_element(),
                    )
                }),
        )
}

fn review_partner_display_subtitle(subtitle: &str) -> String {
    [
        "Focused change · ",
        "Focused hunk · ",
        "Hunk context · ",
        "File context · ",
    ]
    .iter()
    .find_map(|prefix| subtitle.strip_prefix(prefix))
    .unwrap_or(subtitle)
    .to_string()
}

fn review_partner_focus_section_style(label: &str) -> (LucideIcon, Rgba) {
    match label {
        "Usage context" => (LucideIcon::GitCompareArrows, fg_muted()),
        "Codebase fit" => (LucideIcon::ListChecks, fg_muted()),
        "Similar code" => (LucideIcon::SearchCode, fg_muted()),
        "Removed impact" => (LucideIcon::ArchiveX, warning()),
        "Concerns" => (LucideIcon::ShieldAlert, danger()),
        "Focused change" => (LucideIcon::FileCode2, fg_muted()),
        _ => (LucideIcon::Sparkles, accent()),
    }
}

fn render_review_partner_summary(
    record: &crate::review_partner::ReviewPartnerFocusRecord,
) -> impl IntoElement {
    let summary = if record.summary.trim().is_empty() {
        record.title.clone()
    } else {
        record.summary.clone()
    };

    div()
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(render_review_partner_section_header(
            "Summary".to_string(),
            None,
            LucideIcon::Sparkles,
            accent(),
        ))
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_size(px(13.0))
                .line_height(px(21.0))
                .text_color(fg_default())
                .whitespace_normal()
                .child(summary),
        )
}

fn render_review_partner_usage_section(
    state: &Entity<AppState>,
    record: &crate::review_partner::ReviewPartnerFocusRecord,
    cx: &App,
) -> impl IntoElement {
    let symbol_count = record.usage_context.len();
    let usage_count = record
        .usage_context
        .iter()
        .map(|group| group.usages.len())
        .sum::<usize>();
    let detail = format!(
        "{} symbol{}, {} occurrence{}",
        symbol_count,
        if symbol_count == 1 { "" } else { "s" },
        usage_count,
        if usage_count == 1 { "" } else { "s" }
    );
    let body = record
        .usage_context
        .iter()
        .enumerate()
        .map(|(index, group)| render_review_partner_usage_group(state, record, index, group, cx))
        .collect::<Vec<_>>();

    render_review_partner_flat_section(
        "Usages".to_string(),
        Some(detail),
        LucideIcon::GitCompareArrows,
        fg_muted(),
        body,
    )
}

fn render_review_partner_usage_group(
    state: &Entity<AppState>,
    record: &crate::review_partner::ReviewPartnerFocusRecord,
    index: usize,
    group: &crate::review_partner::ReviewPartnerUsageGroup,
    cx: &App,
) -> AnyElement {
    let key = review_partner_disclosure_key(record, &format!("usage-{index}"));
    let body = group
        .usages
        .iter()
        .enumerate()
        .map(|(usage_index, item)| {
            render_review_partner_usage_item_row(state, record, index, usage_index, item)
                .into_any_element()
        })
        .collect::<Vec<_>>();

    render_review_partner_disclosure(
        state,
        key,
        false,
        LucideIcon::FileCode2,
        fg_muted(),
        group.symbol.clone(),
        Some(group.summary.clone()),
        body,
        cx,
    )
    .into_any_element()
}

fn render_review_partner_codebase_fit_section(
    state: &Entity<AppState>,
    record: &crate::review_partner::ReviewPartnerFocusRecord,
    _cx: &App,
) -> AnyElement {
    let fit = &record.codebase_fit;
    let body = fit
        .evidence
        .iter()
        .enumerate()
        .map(|(index, item)| {
            render_review_partner_item_row(state, "Codebase fit", index, item, None, warning())
                .into_any_element()
        })
        .collect::<Vec<_>>();

    render_review_partner_flat_section(
        "Codebase fit".to_string(),
        Some(fit.summary.clone()),
        LucideIcon::ListChecks,
        warning(),
        body,
    )
    .into_any_element()
}

fn render_review_partner_secondary_section(
    state: &Entity<AppState>,
    _record: &crate::review_partner::ReviewPartnerFocusRecord,
    _section_index: usize,
    section: &crate::review_partner::ReviewPartnerFocusSection,
    icon: LucideIcon,
    tone: Rgba,
    _cx: &App,
) -> impl IntoElement {
    let detail = format!(
        "{} item{}",
        section.items.len(),
        if section.items.len() == 1 { "" } else { "s" }
    );
    let body = section
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            render_review_partner_item_row(state, &section.title, index, item, None, tone)
                .into_any_element()
        })
        .collect::<Vec<_>>();

    render_review_partner_flat_section(section.title.clone(), Some(detail), icon, tone, body)
}

fn render_review_partner_flat_section(
    title: String,
    detail: Option<String>,
    icon: LucideIcon,
    tone: Rgba,
    body: Vec<AnyElement>,
) -> impl IntoElement {
    div()
        .pt(px(12.0))
        .border_t(px(1.0))
        .border_color(with_alpha(diff_annotation_border(), 0.8))
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(render_review_partner_section_header(
            title, detail, icon, tone,
        ))
        .children(body)
}

fn render_review_partner_section_header(
    title: String,
    detail: Option<String>,
    icon: LucideIcon,
    tone: Rgba,
) -> impl IntoElement {
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(7.0))
        .child(lucide_icon(icon, 13.0, tone))
        .child(
            div()
                .min_w_0()
                .flex_grow()
                .flex()
                .items_baseline()
                .gap(px(7.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(title),
                )
                .when_some(detail, |el, detail| {
                    el.child(
                        div()
                            .min_w_0()
                            .text_size(px(10.0))
                            .font_family(mono_font_family())
                            .text_color(fg_muted())
                            .whitespace_nowrap()
                            .overflow_x_hidden()
                            .text_ellipsis()
                            .child(detail),
                    )
                }),
        )
}

fn render_review_partner_disclosure(
    state: &Entity<AppState>,
    key: String,
    default_expanded: bool,
    icon: LucideIcon,
    tone: Rgba,
    title: String,
    detail: Option<String>,
    body: Vec<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let expanded = state
        .read(cx)
        .is_review_partner_disclosure_expanded(&key, default_expanded);
    let state_for_toggle = state.clone();
    let key_for_toggle = key.clone();
    let header_key = format!("{key}:header");
    let tooltip = if expanded {
        "Collapse section"
    } else {
        "Expand section"
    };

    div()
        .id(ElementId::Name(key.into()))
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id(ElementId::Name(header_key.into()))
                .rounded(px(4.0))
                .px(px(3.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
                .hover(|style| style.bg(bg_selected()))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    state_for_toggle.update(cx, |state, cx| {
                        state.toggle_review_partner_disclosure(&key_for_toggle, default_expanded);
                        state.persist_active_review_session();
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .child(
                    div()
                        .w(px(13.0))
                        .h(px(18.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(lucide_icon(
                            if expanded {
                                LucideIcon::ChevronDown
                            } else {
                                LucideIcon::ChevronRight
                            },
                            13.0,
                            fg_muted(),
                        )),
                )
                .child(lucide_icon(icon, 12.0, tone))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_family(mono_font_family())
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(title),
                        )
                        .when_some(detail, |el, detail| {
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .line_height(px(17.0))
                                    .text_color(fg_muted())
                                    .whitespace_normal()
                                    .child(detail),
                            )
                        }),
                ),
        )
        .when(expanded && !body.is_empty(), |el| {
            el.child(
                div()
                    .pl(px(22.0))
                    .pt(px(4.0))
                    .pb(px(7.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(body),
            )
        })
}

fn review_partner_disclosure_key(
    record: &crate::review_partner::ReviewPartnerFocusRecord,
    suffix: &str,
) -> String {
    format!("review-partner:{}:{suffix}", record.key)
}

fn render_review_partner_item_row(
    state: &Entity<AppState>,
    section: &str,
    index: usize,
    item: &crate::review_partner::ReviewPartnerItem,
    suppress_title_matching: Option<&str>,
    _tone: Rgba,
) -> impl IntoElement {
    let location_side = if section == "Removed impact" {
        TempSourceSide::Base
    } else {
        TempSourceSide::Head
    };
    let show_title = should_show_review_partner_item_title(
        item.title.as_str(),
        item.detail.as_str(),
        suppress_title_matching,
    );

    div()
        .w_full()
        .min_w_0()
        .rounded(px(4.0))
        .px(px(7.0))
        .py(px(6.0))
        .flex()
        .hover(|style| style.bg(bg_selected()))
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .when(show_title, |el| {
                    el.child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(fg_emphasis())
                            .whitespace_normal()
                            .child(SelectableText::new(
                                format!("review-partner-{section}-{index}-title"),
                                item.title.clone(),
                            )),
                    )
                })
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(19.0))
                        .font_weight(if show_title {
                            FontWeight::NORMAL
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if show_title {
                            fg_default()
                        } else {
                            fg_emphasis()
                        })
                        .whitespace_normal()
                        .child(SelectableText::new(
                            format!("review-partner-{section}-{index}-detail"),
                            item.detail.clone(),
                        )),
                )
                .when_some(item.path.as_ref(), |el, path| {
                    el.child(render_review_partner_location_link(
                        state,
                        path.clone(),
                        item.line,
                        location_side,
                    ))
                }),
        )
}

fn render_review_partner_usage_item_row(
    state: &Entity<AppState>,
    record: &crate::review_partner::ReviewPartnerFocusRecord,
    group_index: usize,
    usage_index: usize,
    item: &crate::review_partner::ReviewPartnerItem,
) -> impl IntoElement {
    let source_hint = item.path.as_deref().unwrap_or("rust");
    let snippet = item.detail.trim();
    let snippet = if snippet.is_empty() { " " } else { snippet };

    div()
        .w_full()
        .min_w_0()
        .rounded(px(4.0))
        .px(px(7.0))
        .py(px(6.0))
        .flex()
        .hover(|style| style.bg(bg_selected()))
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(18.0))
                        .font_family(mono_font_family())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_default())
                        .whitespace_normal()
                        .child(render_review_partner_code_snippet(
                            format!(
                                "review-partner-{}-usage-{group_index}-{usage_index}-snippet",
                                record.key
                            ),
                            source_hint,
                            snippet,
                        )),
                )
                .when_some(item.path.as_ref(), |el, path| {
                    el.child(render_review_partner_location_link(
                        state,
                        path.clone(),
                        item.line,
                        TempSourceSide::Head,
                    ))
                }),
        )
}

fn render_review_partner_code_snippet(
    selection_id: String,
    source_hint: &str,
    snippet: &str,
) -> SelectableText {
    let text = snippet.to_string();
    let spans = syntax::highlight_line(source_hint, &text);
    if let Some(runs) = code_text_runs(text.as_str(), spans.as_slice(), fg_default()) {
        SelectableText::new(selection_id, text).with_runs(runs)
    } else {
        SelectableText::new(selection_id, text)
    }
}

fn should_show_review_partner_item_title(
    title: &str,
    detail: &str,
    suppress_matching: Option<&str>,
) -> bool {
    let title = title.trim();
    if title.is_empty() {
        return false;
    }
    if !detail.trim().is_empty() && title.eq_ignore_ascii_case(detail.trim()) {
        return false;
    }
    suppress_matching
        .map(|matching| !title.eq_ignore_ascii_case(matching.trim()))
        .unwrap_or(true)
}

fn render_review_partner_location_link(
    state: &Entity<AppState>,
    path: String,
    line: Option<usize>,
    side: TempSourceSide,
) -> impl IntoElement {
    let location_label = match line {
        Some(line) => format!("{path}:{line}"),
        None => path.clone(),
    };
    let state_for_open = state.clone();
    let path_for_open = path.clone();

    div()
        .mt(px(2.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .min_w_0()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            open_review_partner_source_window(
                &state_for_open,
                path_for_open.clone(),
                line,
                side,
                window,
                cx,
            );
            cx.stop_propagation();
        })
        .child(lucide_icon(LucideIcon::ExternalLink, 10.0, info()))
        .child(
            div()
                .min_w_0()
                .overflow_x_hidden()
                .text_size(px(10.0))
                .line_height(px(15.0))
                .font_family(mono_font_family())
                .text_color(info())
                .whitespace_nowrap()
                .overflow_x_hidden()
                .text_ellipsis()
                .child(location_label),
        )
}

fn open_review_partner_source_window(
    state: &Entity<AppState>,
    path: String,
    line: Option<usize>,
    side: TempSourceSide,
    window: &mut Window,
    cx: &mut App,
) {
    let target = {
        let app_state = state.read(cx);
        let Some(detail) = app_state.active_detail() else {
            return;
        };
        let reference = match side {
            TempSourceSide::Base => detail
                .base_ref_oid
                .clone()
                .or_else(|| Some(detail.base_ref_name.clone())),
            TempSourceSide::Head => detail
                .head_ref_oid
                .clone()
                .or_else(|| Some(detail.head_ref_name.clone())),
        }
        .map(|reference| reference.trim().to_string())
        .filter(|reference| !reference.is_empty());

        reference.map(|reference| TempSourceTarget {
            path,
            side,
            line: line.unwrap_or(1).max(1),
            reference,
        })
    };

    if let Some(target) = target {
        open_temp_source_window_for_diff_target(state, target, window, cx);
    }
}

fn review_partner_icon_chip(icon: LucideIcon, tone: Rgba) -> impl IntoElement {
    div()
        .w(px(22.0))
        .h(px(22.0))
        .flex_shrink_0()
        .rounded(px(5.0))
        .bg(with_alpha(tone, 0.10))
        .flex()
        .items_center()
        .justify_center()
        .child(lucide_icon(icon, 13.0, tone))
}

fn review_partner_action_icon_button(
    icon: LucideIcon,
    tooltip: &'static str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("review-partner-regenerate")
        .h(px(28.0))
        .w(px(28.0))
        .flex_shrink_0()
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(diff_annotation_bg())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .hover(|style| style.bg(bg_selected()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(icon, 14.0, fg_muted()))
}

pub(super) fn render_local_review_empty_state(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    local_repo_status: Option<&local_repo::LocalRepositoryStatus>,
    local_repo_loading: bool,
    local_repo_error: Option<&str>,
) -> impl IntoElement {
    let state_for_refresh = state.clone();
    let title = if local_repo_loading {
        "Refreshing local review"
    } else {
        "No local changes"
    };
    let message = local_repo_error
        .or_else(|| local_repo_status.map(|status| status.message.as_str()))
        .unwrap_or("This checkout has no reviewable changes ahead of the selected base.");
    let base = detail.base_ref_oid.as_deref().unwrap_or("unknown");
    let head = detail.head_ref_oid.as_deref().unwrap_or("unknown");

    div()
        .flex_grow()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .child(
            nested_panel()
                .max_w(px(560.0))
                .child(eyebrow("Local review"))
                .child(
                    div()
                        .mt(px(8.0))
                        .text_size(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(title),
                )
                .child(
                    div()
                        .mt(px(8.0))
                        .text_size(px(13.0))
                        .line_height(px(19.0))
                        .text_color(fg_default())
                        .child(message.to_string()),
                )
                .child(
                    div()
                        .mt(px(14.0))
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .text_size(px(11.0))
                        .font_family(mono_font_family())
                        .text_color(fg_muted())
                        .child(format!("repo {}", detail.repository))
                        .child(format!("branch {}", detail.head_ref_name))
                        .child(format!("base {}", short_oid(base)))
                        .child(format!("head {}", short_oid(head))),
                )
                .child(
                    div()
                        .mt(px(16.0))
                        .child(review_button("Refresh", move |_, window, cx| {
                            refresh_active_local_review(&state_for_refresh, window, cx);
                        })),
                ),
        )
}

fn short_oid(oid: &str) -> String {
    oid.chars().take(12).collect()
}

pub(super) fn render_guided_review_lens_toggle(
    state: &Entity<AppState>,
    active_lens: ReviewGuideLens,
) -> impl IntoElement {
    let state_for_diff = state.clone();
    let state_for_structural = state.clone();

    div()
        .id("guided-review-lens-toggle")
        .h(px(28.0))
        .p(px(2.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(control_track_bg())
        .flex()
        .items_center()
        .gap(px(1.0))
        .tooltip(|_, cx| build_static_tooltip("Guided Review diff lens", cx))
        .child(diff_layout_segment(
            "Diff",
            active_lens == ReviewGuideLens::Diff,
            false,
            move |_, _, cx| {
                state_for_diff.update(cx, |state, cx| {
                    state.set_guided_review_lens(ReviewGuideLens::Diff);
                    state.persist_active_review_session();
                    cx.notify();
                });
            },
        ))
        .child(diff_layout_segment(
            "Structural",
            active_lens == ReviewGuideLens::Structural,
            false,
            move |_, _, cx| {
                state_for_structural.update(cx, |state, cx| {
                    state.set_guided_review_lens(ReviewGuideLens::Structural);
                    state.persist_active_review_session();
                    cx.notify();
                });
            },
        ))
}

pub(super) fn render_diff_layout_toggle(
    state: &Entity<AppState>,
    center_mode: ReviewCenterMode,
    active_layout: DiffLayout,
    disabled: bool,
) -> impl IntoElement {
    let state_for_unified = state.clone();
    let state_for_side_by_side = state.clone();
    let tooltip = if center_mode == ReviewCenterMode::StructuralDiff {
        "Structural diff layout"
    } else {
        "Diff layout"
    };

    div()
        .id(ElementId::Name(
            format!("diff-layout-toggle-{center_mode:?}").into(),
        ))
        .h(px(28.0))
        .p(px(2.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(control_track_bg())
        .flex()
        .items_center()
        .gap(px(1.0))
        .opacity(if disabled { 0.5 } else { 1.0 })
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .child(diff_layout_segment(
            "Unified",
            active_layout == DiffLayout::Unified,
            disabled,
            move |_, _, cx| {
                state_for_unified.update(cx, |state, cx| {
                    if center_mode == ReviewCenterMode::StructuralDiff {
                        state.set_structural_diff_layout(DiffLayout::Unified);
                    } else {
                        state.set_normal_diff_layout(DiffLayout::Unified);
                    }
                    state.persist_active_review_session();
                    cx.notify();
                });
            },
        ))
        .child(diff_layout_segment(
            "Split",
            active_layout == DiffLayout::SideBySide,
            disabled,
            move |_, _, cx| {
                state_for_side_by_side.update(cx, |state, cx| {
                    if center_mode == ReviewCenterMode::StructuralDiff {
                        state.set_structural_diff_layout(DiffLayout::SideBySide);
                    } else {
                        state.set_normal_diff_layout(DiffLayout::SideBySide);
                    }
                    state.persist_active_review_session();
                    cx.notify();
                });
            },
        ))
}

fn diff_layout_segment(
    label: &'static str,
    active: bool,
    disabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!("diff-layout-{label}-{}", usize::from(active)));

    div()
        .h(px(22.0))
        .px(px(8.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(transparent())
        .bg(if active { bg_emphasis() } else { transparent() })
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .flex()
        .items_center()
        .justify_center()
        .when(!disabled, move |el| {
            el.cursor_pointer()
                .hover(move |style| {
                    style
                        .bg(if active { bg_emphasis() } else { bg_selected() })
                        .text_color(fg_emphasis())
                })
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(label)
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_emphasis(), progress))
                    .text_color(mix_rgba(fg_muted(), fg_emphasis(), progress))
            },
        )
}
