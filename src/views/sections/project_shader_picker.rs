use gpui::prelude::*;
use gpui::*;

use crate::icons::{lucide_icon, LucideIcon};
use crate::shader_surface::{OverviewShaderVariant, ShaderCornerMask};
use crate::state::{AppState, ProjectShaderPickerState};
use crate::theme::*;

use super::error_text;
use super::material::shader_material_surface_variant;

pub(super) fn render_project_shader_picker(
    state: &Entity<AppState>,
    picker: ProjectShaderPickerState,
    settings_error: Option<String>,
    cx: &App,
) -> impl IntoElement {
    let selected = state.read(cx).shader_for_project(&picker.project);
    let close_state = state.clone();
    let project_display = if picker.project == "__mine__" {
        picker.label.clone()
    } else {
        picker.project.clone()
    };

    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(82.0))
        .pb(px(28.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    close_state.update(cx, |s, cx| {
                        s.close_project_shader_picker();
                        cx.notify();
                    });
                }),
        )
        .child(
            div()
                .relative()
                .w(px(460.0))
                .rounded(radius_lg())
                .border_1()
                .border_color(transparent())
                .bg(bg_overlay())
                .shadow(dialog_shadow())
                .occlude()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(20.0))
                        .py(px(15.0))
                        .border_b(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child("Project shader"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(project_display),
                                ),
                        )
                        .child(
                            div()
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded(radius_sm())
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(hover_bg()))
                                .on_mouse_down(MouseButton::Left, {
                                    let state = state.clone();
                                    move |_, _, cx| {
                                        cx.stop_propagation();
                                        state.update(cx, |s, cx| {
                                            s.close_project_shader_picker();
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(lucide_icon(LucideIcon::X, 17.0, fg_muted())),
                        ),
                )
                .child(
                    div()
                        .p(px(14.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children(OverviewShaderVariant::ALL.into_iter().map(|variant| {
                            project_shader_choice_row(&picker.project, variant, selected, state)
                        }))
                        .when_some(settings_error, |el, error| {
                            el.child(div().pt(px(4.0)).child(error_text(&error)))
                        }),
                ),
        )
}

fn project_shader_choice_row(
    project: &str,
    variant: OverviewShaderVariant,
    selected: OverviewShaderVariant,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let is_selected = variant == selected;
    let project = project.to_string();
    let label = variant.label();
    let sample_seed = format!("shader-choice-{project}-{label}");
    let state = state.clone();

    div()
        .w_full()
        .rounded(radius())
        .border_1()
        .border_color(transparent())
        .bg(if is_selected {
            control_selected_bg()
        } else {
            bg_surface()
        })
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            state.update(cx, |s, cx| {
                s.set_project_shader(&project, variant);
                cx.notify();
            });
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(8.0))
                .child(
                    shader_material_surface_variant(
                        &sample_seed,
                        variant,
                        ShaderCornerMask::ALL,
                        bg_surface(),
                        radius_sm(),
                        true,
                    )
                    .w(px(76.0))
                    .h(px(40.0))
                    .flex_shrink_0(),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(label),
                        ),
                )
                .when(is_selected, |el| {
                    el.child(lucide_icon(LucideIcon::Check, 16.0, focus()))
                }),
        )
}
