use gpui::prelude::*;
use gpui::*;
use std::collections::BTreeSet;

use crate::icons::{lucide_icon, LucideIcon};
use crate::review_filters::{
    ActivityFilter, DraftFilter, FreshnessFilter, PullRequestFilter, PullRequestFilterPreset,
    PullRequestFilterScope, PullRequestFilterToggle, ReviewDecisionFilter, SizeFilter, TrustFilter,
};
use crate::selectable_text::{AppTextFieldKind, AppTextInput};
use crate::state::AppState;
use crate::theme::*;

use super::super::tooltips::build_static_tooltip;
use super::{error_text, subtle_pill};

#[derive(Clone, Copy)]
struct FilterCriterion {
    label: &'static str,
    active: bool,
    toggle: PullRequestFilterToggle,
    visible: bool,
}

impl FilterCriterion {
    fn new(label: &'static str, active: bool, toggle: PullRequestFilterToggle) -> Self {
        Self {
            label,
            active,
            toggle,
            visible: true,
        }
    }

    fn visible(
        label: &'static str,
        active: bool,
        toggle: PullRequestFilterToggle,
        visible: bool,
    ) -> Self {
        Self {
            label,
            active,
            toggle,
            visible,
        }
    }
}

pub(super) fn render_pull_request_filter_bar(
    state: &Entity<AppState>,
    scope: PullRequestFilterScope,
    active_labels: Vec<String>,
    visible_count: usize,
    total_count: usize,
    hidden_count: usize,
    dialog_open: bool,
) -> impl IntoElement {
    let status = filter_status_text(visible_count, total_count, hidden_count);
    let active_count = active_labels.len();
    let state_for_dialog = state.clone();

    div()
        .px(px(28.0))
        .pb(px(12.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .flex_wrap()
        .child(filter_status_marker(&status))
        .child(filter_dialog_button(
            active_count,
            dialog_open,
            move |_, _, cx| {
                state_for_dialog.update(cx, |state, cx| {
                    state.open_pull_request_filter_dialog(scope);
                    cx.notify();
                });
            },
        ))
        .when(!active_labels.is_empty(), |el| {
            el.children(active_labels.into_iter().map(|label| subtle_pill(&label)))
        })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_pull_request_filter_dialog(
    state: &Entity<AppState>,
    scope: PullRequestFilterScope,
    presets: Vec<PullRequestFilterPreset>,
    active_preset_ids: Vec<String>,
    filter: &PullRequestFilter,
    active_labels: Vec<String>,
    visible_count: usize,
    total_count: usize,
    hidden_count: usize,
    has_muted: bool,
    creator_scope: Option<PullRequestFilterScope>,
    filter_preset_name: String,
    filter_message: Option<String>,
) -> impl IntoElement {
    let creator_open = creator_scope == Some(scope);
    let filter_message = creator_open.then_some(filter_message).flatten();
    let can_save_filter =
        filter != &PullRequestFilter::default() && !filter_preset_name.trim().is_empty();
    let state_for_close = state.clone();
    let status = filter_status_text(visible_count, total_count, hidden_count);

    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(74.0))
        .pb(px(28.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    state_for_close.update(cx, |state, cx| {
                        state.close_pull_request_filter_dialog();
                        cx.notify();
                    });
                }),
        )
        .child(
            div()
                .relative()
                .w(px(640.0))
                .max_h(px(640.0))
                .rounded(radius_lg())
                .border_1()
                .border_color(transparent())
                .bg(bg_overlay())
                .shadow(dialog_shadow())
                .occlude()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(render_filter_dialog_header(state, scope, &status))
                .child(
                    div()
                        .id("pull-request-filter-dialog-scroll")
                        .max_h(px(560.0))
                        .overflow_y_scroll()
                        .p(px(20.0))
                        .flex()
                        .flex_col()
                        .gap(px(18.0))
                        .child(render_saved_filters(
                            state,
                            scope,
                            presets,
                            active_preset_ids,
                        ))
                        .when(creator_open, |el| {
                            el.child(render_pull_request_filter_creator(
                                state,
                                scope,
                                filter_preset_name,
                                can_save_filter,
                                filter_message,
                            ))
                        })
                        .child(render_criteria(state, scope, filter, has_muted))
                        .when(!active_labels.is_empty(), |el| {
                            el.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(9.0))
                                    .child(filter_dialog_section_label("Active"))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(5.0))
                                            .flex_wrap()
                                            .children(
                                                active_labels
                                                    .into_iter()
                                                    .map(|label| subtle_pill(&label)),
                                            ),
                                    ),
                            )
                        }),
                ),
        )
}

fn render_filter_dialog_header(
    state: &Entity<AppState>,
    scope: PullRequestFilterScope,
    status: &str,
) -> impl IntoElement {
    div()
        .px(px(20.0))
        .py(px(15.0))
        .border_b(px(1.0))
        .border_color(border_muted())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(filter_dialog_title(scope)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family(mono_font_family())
                        .text_color(fg_subtle())
                        .child(status.to_string()),
                ),
        )
        .child(filter_icon_button(LucideIcon::X, "Close filters", {
            let state = state.clone();
            move |_, _, cx| {
                state.update(cx, |state, cx| {
                    state.close_pull_request_filter_dialog();
                    cx.notify();
                });
            }
        }))
}

fn render_saved_filters(
    state: &Entity<AppState>,
    scope: PullRequestFilterScope,
    presets: Vec<PullRequestFilterPreset>,
    active_preset_ids: Vec<String>,
) -> impl IntoElement {
    let state_for_creator = state.clone();
    let active_preset_ids = active_preset_ids.into_iter().collect::<BTreeSet<_>>();

    div()
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(filter_dialog_section_label("Saved filters"))
                .child(filter_toolbar_button(
                    "Save current",
                    LucideIcon::Plus,
                    true,
                    move |_, _, cx| {
                        state_for_creator.update(cx, |state, cx| {
                            state.open_pull_request_filter_creator(scope);
                            cx.notify();
                        });
                    },
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .flex_wrap()
                .children(presets.into_iter().enumerate().map(|(preset_ix, preset)| {
                    let active = active_preset_ids.contains(&preset.id);
                    let custom = preset.is_custom();
                    let preset_id_for_select = preset.id.clone();
                    let preset_id_for_delete = preset.id.clone();
                    let state_for_select = state.clone();
                    let state_for_delete = state.clone();
                    saved_filter_chip(
                        preset_ix,
                        &preset.label,
                        active,
                        custom,
                        move |_, _, cx| {
                            state_for_select.update(cx, |state, cx| {
                                state.toggle_pull_request_filter_preset(
                                    scope,
                                    &preset_id_for_select,
                                );
                                cx.notify();
                            });
                        },
                        move |_, _, cx| {
                            state_for_delete.update(cx, |state, cx| {
                                state.delete_custom_pull_request_filter_preset(
                                    scope,
                                    &preset_id_for_delete,
                                );
                                cx.notify();
                            });
                        },
                    )
                })),
        )
}

fn render_criteria(
    state: &Entity<AppState>,
    scope: PullRequestFilterScope,
    filter: &PullRequestFilter,
    has_muted: bool,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(filter_dialog_section_label("Criteria"))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .flex_wrap()
                .children(
                    filter_criteria(filter, has_muted)
                        .into_iter()
                        .map(|criterion| {
                            let state = state.clone();
                            filter_chip(criterion.label, criterion.active, move |_, _, cx| {
                                state.update(cx, |state, cx| {
                                    state.toggle_pull_request_filter(scope, criterion.toggle);
                                    cx.notify();
                                });
                            })
                        }),
                ),
        )
}

fn filter_criteria(filter: &PullRequestFilter, has_muted: bool) -> Vec<FilterCriterion> {
    [
        FilterCriterion::new(
            "Trusted",
            filter.trust == TrustFilter::Trusted,
            PullRequestFilterToggle::Trusted,
        ),
        FilterCriterion::new(
            "Vouched",
            filter.trust == TrustFilter::Vouched,
            PullRequestFilterToggle::Vouched,
        ),
        FilterCriterion::new(
            "First-time",
            filter.trust == TrustFilter::FirstTime,
            PullRequestFilterToggle::FirstTime,
        ),
        FilterCriterion::new(
            "Unknown",
            filter.trust == TrustFilter::Unknown,
            PullRequestFilterToggle::TrustUnknown,
        ),
        FilterCriterion::new(
            "Denounced",
            filter.trust == TrustFilter::Denounced,
            PullRequestFilterToggle::Denounced,
        ),
        FilterCriterion::new(
            "Ready",
            filter.draft == DraftFilter::Ready,
            PullRequestFilterToggle::Ready,
        ),
        FilterCriterion::new(
            "Draft",
            filter.draft == DraftFilter::Draft,
            PullRequestFilterToggle::Draft,
        ),
        FilterCriterion::new(
            "Unread",
            filter.activity == ActivityFilter::Unread,
            PullRequestFilterToggle::Unread,
        ),
        FilterCriterion::new(
            "Fresh",
            filter.freshness == FreshnessFilter::Fresh,
            PullRequestFilterToggle::Fresh,
        ),
        FilterCriterion::new(
            "Stale",
            filter.freshness == FreshnessFilter::Stale,
            PullRequestFilterToggle::Stale,
        ),
        FilterCriterion::new(
            "Large",
            filter.size == SizeFilter::Large,
            PullRequestFilterToggle::Large,
        ),
        FilterCriterion::new(
            "Needs review",
            filter.review_decision == ReviewDecisionFilter::ReviewRequired,
            PullRequestFilterToggle::NeedsReview,
        ),
        FilterCriterion::visible(
            "Muted",
            filter.include_muted,
            PullRequestFilterToggle::IncludeMuted,
            has_muted || filter.include_muted,
        ),
    ]
    .into_iter()
    .filter(|criterion| criterion.visible)
    .collect()
}

fn filter_status_text(visible_count: usize, total_count: usize, hidden_count: usize) -> String {
    let mut status = format!("{visible_count}/{total_count}");
    if hidden_count > 0 {
        status.push_str(&format!(" visible, {hidden_count} hidden"));
    } else {
        status.push_str(" visible");
    }
    status
}

fn filter_dialog_title(scope: PullRequestFilterScope) -> &'static str {
    match scope {
        PullRequestFilterScope::Overview => "Overview filters",
        PullRequestFilterScope::Pulls => "Pull request filters",
        PullRequestFilterScope::Reviews => "Review filters",
    }
}

fn filter_status_marker(label: &str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .mr(px(2.0))
        .text_size(px(11.0))
        .font_family(mono_font_family())
        .text_color(fg_subtle())
        .child(label.to_string())
}

fn filter_dialog_button(
    active_count: usize,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("pull-request-filter-button")
        .h(px(28.0))
        .px(px(9.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
        .bg(if active {
            bg_selected()
        } else {
            control_button_bg()
        })
        .flex()
        .items_center()
        .gap(px(5.0))
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .hover(move |style| {
            style
                .bg(if active {
                    bg_selected()
                } else {
                    control_button_hover_bg()
                })
                .text_color(fg_emphasis())
        })
        .tooltip(|_, cx| build_static_tooltip("Choose filters", cx))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(LucideIcon::ListFilter, 12.0, fg_muted()))
        .child("Filters".to_string())
        .when(active_count > 0, |el| {
            el.child(
                div()
                    .min_w(px(16.0))
                    .h(px(16.0))
                    .px(px(4.0))
                    .rounded(px(999.0))
                    .bg(bg_emphasis())
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(mono_font_family())
                    .text_size(px(10.0))
                    .text_color(fg_emphasis())
                    .child(active_count.to_string()),
            )
        })
}

fn filter_dialog_section_label(label: &str) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg_subtle())
        .child(label.to_string().to_uppercase())
}

fn saved_filter_chip(
    id_suffix: usize,
    label: &str,
    active: bool,
    custom: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_delete: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .max_w(px(180.0))
        .px(px(9.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
        .bg(if active { bg_selected() } else { bg_overlay() })
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .flex()
        .items_center()
        .gap(px(5.0))
        .hover(move |style| {
            style
                .bg(if active { bg_selected() } else { hover_bg() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .min_w_0()
                .line_clamp(1)
                .overflow_hidden()
                .child(label.to_string()),
        )
        .when(custom, |el| {
            el.pr(px(4.0)).child(
                div()
                    .id(("saved-filter-delete", id_suffix))
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded(radius_sm())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(fg_subtle())
                    .hover(|style| {
                        style
                            .bg(control_button_hover_bg())
                            .text_color(fg_emphasis())
                    })
                    .tooltip(|_, cx| build_static_tooltip("Delete saved filter", cx))
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        cx.stop_propagation();
                        on_delete(event, window, cx);
                    })
                    .child(lucide_icon(LucideIcon::X, 11.0, fg_subtle())),
            )
        })
}

fn render_pull_request_filter_creator(
    state: &Entity<AppState>,
    scope: PullRequestFilterScope,
    filter_preset_name: String,
    can_save_filter: bool,
    filter_message: Option<String>,
) -> impl IntoElement {
    let state_for_save = state.clone();
    let state_for_cancel = state.clone();

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .flex_wrap()
        .child(
            div()
                .w(px(220.0))
                .min_w(px(180.0))
                .h(px(30.0))
                .px(px(10.0))
                .rounded(radius_sm())
                .border_1()
                .border_color(focus_border())
                .bg(bg_surface())
                .flex()
                .items_center()
                .text_size(px(12.0))
                .text_color(if filter_preset_name.is_empty() {
                    fg_subtle()
                } else {
                    fg_emphasis()
                })
                .child(
                    AppTextInput::new(
                        format!("filter-name-{}", scope.key()),
                        state.clone(),
                        AppTextFieldKind::PullRequestFilterName,
                        "Filter name",
                    )
                    .autofocus(true),
                ),
        )
        .child(filter_toolbar_button(
            "Save",
            LucideIcon::Check,
            can_save_filter,
            move |_, _, cx| {
                state_for_save.update(cx, |state, cx| {
                    state.save_current_pull_request_filter_preset();
                    cx.notify();
                });
            },
        ))
        .child(filter_icon_button(
            LucideIcon::X,
            "Cancel",
            move |_, _, cx| {
                state_for_cancel.update(cx, |state, cx| {
                    state.close_pull_request_filter_creator();
                    cx.notify();
                });
            },
        ))
        .when_some(filter_message, |el, message| el.child(error_text(&message)))
}

fn filter_toolbar_button(
    label: &str,
    icon: LucideIcon,
    enabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let button = div()
        .h(px(28.0))
        .px(px(9.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(control_button_bg())
        .flex()
        .items_center()
        .gap(px(5.0))
        .text_color(fg_muted())
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .child(lucide_icon(icon, 12.0, fg_muted()))
        .child(label.to_string());

    if enabled {
        button
            .hover(|style| {
                style
                    .bg(control_button_hover_bg())
                    .text_color(fg_emphasis())
            })
            .on_mouse_down(MouseButton::Left, on_click)
    } else {
        button.opacity(0.42)
    }
}

fn filter_icon_button(
    icon: LucideIcon,
    tooltip: &'static str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(tooltip)
        .w(px(28.0))
        .h(px(28.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(control_button_bg())
        .flex()
        .items_center()
        .justify_center()
        .hover(|style| {
            style
                .bg(control_button_hover_bg())
                .text_color(fg_emphasis())
        })
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(icon, 13.0, fg_muted()))
}

fn filter_chip(
    label: &str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(9.0))
        .py(px(4.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
        .bg(if active { bg_selected() } else { bg_overlay() })
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .hover(move |style| {
            style
                .bg(if active { bg_selected() } else { hover_bg() })
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
}
