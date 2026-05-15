use gpui::prelude::*;
use gpui::*;

use crate::icons::{lucide_icon, LucideIcon};
use crate::onboarding::{
    derive_gh_setup_status, GhSetupState, WizardKind, WizardStepDefinition, WizardStepTarget,
    WizardTone,
};
use crate::state::AppState;
use crate::theme::*;

const COACHMARK_WIDTH: f32 = 386.0;

pub(super) fn refresh_onboarding_gh_status(
    state: &Entity<AppState>,
    _window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).active_onboarding_wizard.is_none() {
        return;
    }

    state.update(cx, |state, cx| {
        state.set_onboarding_gh_status(crate::onboarding::GhSetupStatus::checking());
        cx.notify();
    });

    let model = state.clone();
    cx.spawn(async move |cx| {
        let gh_result = cx
            .background_executor()
            .spawn(async { crate::gh::run(&["--version"]) })
            .await;
        let auth_result = cx
            .background_executor()
            .spawn(async { crate::github::check_live_auth_state() })
            .await;
        let status = derive_gh_setup_status(gh_result, auth_result);

        model
            .update(cx, |state, cx| {
                if status.state == GhSetupState::Ready {
                    state.gh_available = true;
                }
                state.gh_version = status.version.clone();
                state.set_onboarding_gh_status(status);
                cx.notify();
            })
            .ok();
    })
    .detach();
}

pub(super) fn render_onboarding_wizard(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let Some(session) = state.read(cx).active_onboarding_wizard.clone() else {
        return div().into_any_element();
    };

    let step = session.active_step();
    let step_count = session.step_count();
    let step_number = session.step_index + 1;
    let is_first_step = session.is_first_step();
    let is_last_step = session.is_last_step();
    let next_label = if is_last_step {
        session.definition.complete_label.to_string()
    } else {
        "Next".to_string()
    };
    let skip_label = match session.definition.kind {
        WizardKind::Welcome => "Skip tour",
        WizardKind::Feature => "Not now",
    };
    let title_prefix = if session.forced {
        "Forced wizard"
    } else {
        match session.definition.kind {
            WizardKind::Welcome => "First run",
            WizardKind::Feature => "New feature",
        }
    };

    let state_for_skip = state.clone();
    let state_for_back = state.clone();
    let state_for_next = state.clone();
    let state_for_close = state.clone();

    div()
        .absolute()
        .inset_0()
        .child(
            coachmark_position(step.target)
                .w(px(COACHMARK_WIDTH))
                .rounded(radius_lg())
                .border_1()
                .border_color(focus_border())
                .bg(bg_overlay())
                .shadow_sm()
                .occlude()
                .overflow_hidden()
                .child(render_coachmark_header(
                    title_prefix,
                    step,
                    step_number,
                    step_count,
                    state_for_close,
                ))
                .child(render_step_content(
                    state,
                    step,
                    step_number,
                    step_count,
                    cx,
                ))
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(12.0))
                        .border_t(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(wizard_secondary_button(
                            skip_label.to_string(),
                            false,
                            move |_, _, cx| {
                                state_for_skip.update(cx, |state, cx| {
                                    state.complete_active_onboarding_wizard();
                                    cx.notify();
                                });
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(wizard_secondary_button(
                                    "Back".to_string(),
                                    is_first_step,
                                    move |_, _, cx| {
                                        state_for_back.update(cx, |state, cx| {
                                            state.previous_onboarding_step();
                                            cx.notify();
                                        });
                                    },
                                ))
                                .child(wizard_primary_button(
                                    next_label,
                                    false,
                                    move |_, _, cx| {
                                        state_for_next.update(cx, |state, cx| {
                                            state.next_onboarding_step();
                                            cx.notify();
                                        });
                                    },
                                )),
                        ),
                ),
        )
        .with_animation(
            "onboarding-coachmark",
            Animation::new(std::time::Duration::from_millis(160)).with_easing(ease_in_out),
            move |el, delta| {
                el.mt(px(8.0 * (1.0 - delta.clamp(0.0, 1.0))))
                    .opacity(delta.clamp(0.0, 1.0))
            },
        )
        .into_any_element()
}

fn coachmark_position(target: WizardStepTarget) -> Div {
    let base = div().absolute();
    match target {
        WizardStepTarget::GithubSetup => base.top(px(82.0)).right(px(22.0)),
        WizardStepTarget::TutorialReview => base.top(px(82.0)).right(px(22.0)),
        WizardStepTarget::GuidedReview => base.top(px(82.0)).right(px(22.0)),
        WizardStepTarget::LocalReview => base.left(px(232.0)).bottom(px(96.0)),
        WizardStepTarget::ReviewFeedback => base.right(px(22.0)).bottom(px(22.0)),
    }
}

fn render_coachmark_header(
    title_prefix: &str,
    step: WizardStepDefinition,
    step_number: usize,
    step_count: usize,
    state_for_close: Entity<AppState>,
) -> impl IntoElement {
    let tone = wizard_tone_color(step.tone);

    div()
        .px(px(14.0))
        .py(px(12.0))
        .border_b(px(1.0))
        .border_color(border_muted())
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .items_start()
                .gap(px(10.0))
                .child(wizard_icon_chip(wizard_tone_icon(step.tone), tone))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_family(mono_font_family())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_subtle())
                                .child(format!(
                                    "{} / STEP {} OF {} / {}",
                                    title_prefix.to_ascii_uppercase(),
                                    step_number,
                                    step_count,
                                    step.target.label().to_ascii_uppercase()
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(17.0))
                                .line_height(px(22.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(step.title.to_string()),
                        ),
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
                .cursor_pointer()
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    state_for_close.update(cx, |state, cx| {
                        state.complete_active_onboarding_wizard();
                        cx.notify();
                    });
                })
                .child(lucide_icon(LucideIcon::X, 16.0, fg_muted())),
        )
}

fn render_step_content(
    state: &Entity<AppState>,
    step: WizardStepDefinition,
    step_number: usize,
    step_count: usize,
    cx: &App,
) -> impl IntoElement {
    let tone = wizard_tone_color(step.tone);

    div()
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(fg_default())
                        .whitespace_normal()
                        .child(step.body.to_string()),
                )
                .child(div().flex_shrink_0().child(render_wizard_progress(
                    step_number,
                    step_count,
                    tone,
                ))),
        )
        .child(if step.target == WizardStepTarget::GithubSetup {
            render_github_setup_content(state, cx).into_any_element()
        } else {
            render_bullets(step, tone).into_any_element()
        })
}

fn render_github_setup_content(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let status = state.read(cx).onboarding_gh_status.clone();
    let state_for_retry = state.clone();
    let status_color = match status.state {
        GhSetupState::Checking => fg_subtle(),
        GhSetupState::Ready => success(),
        GhSetupState::Missing => danger(),
        GhSetupState::NeedsAuth => warning(),
    };
    let status_icon = match status.state {
        GhSetupState::Checking => LucideIcon::RefreshCw,
        GhSetupState::Ready => LucideIcon::Check,
        GhSetupState::Missing | GhSetupState::NeedsAuth => LucideIcon::AlertTriangle,
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .rounded(radius_sm())
                .border_1()
                .border_color(with_alpha(status_color, 0.34))
                .bg(with_alpha(status_color, 0.1))
                .p(px(10.0))
                .flex()
                .items_start()
                .gap(px(9.0))
                .child(lucide_icon(status_icon, 15.0, status_color))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(status.state.label()),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(17.0))
                                .text_color(fg_default())
                                .child(status_summary(&status)),
                        ),
                ),
        )
        .child(render_command_copy_row("Install", "brew install gh"))
        .child(render_command_copy_row("Authenticate", "gh auth login"))
        .child(render_command_copy_row("Setup git", "gh auth setup-git"))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(16.0))
                        .text_color(fg_subtle())
                        .child("Setup can be completed later; the tutorial still works offline."),
                )
                .child(wizard_secondary_button(
                    "Retry".to_string(),
                    status.state == GhSetupState::Checking,
                    move |_, window, cx| {
                        refresh_onboarding_gh_status(&state_for_retry, window, cx);
                    },
                )),
        )
}

fn status_summary(status: &crate::onboarding::GhSetupStatus) -> String {
    match status.state {
        GhSetupState::Ready => {
            let login = status.login.as_deref().unwrap_or("authenticated user");
            let version = status.version.as_deref().unwrap_or("gh");
            format!("{version} is authenticated as {login}.")
        }
        GhSetupState::Missing | GhSetupState::NeedsAuth => status.message.clone(),
        GhSetupState::Checking => status.message.clone(),
    }
}

fn render_command_copy_row(label: &'static str, command: &'static str) -> impl IntoElement {
    div()
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_surface())
        .px(px(10.0))
        .py(px(9.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(fg_subtle())
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family(mono_font_family())
                        .text_color(fg_emphasis())
                        .line_clamp(1)
                        .child(command),
                ),
        )
        .child(
            div()
                .w(px(28.0))
                .h(px(28.0))
                .rounded(radius_sm())
                .border_1()
                .border_color(border_muted())
                .bg(control_button_bg())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(control_button_hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
                })
                .child(lucide_icon(LucideIcon::Copy, 14.0, fg_muted())),
        )
}

fn render_bullets(step: WizardStepDefinition, tone: Rgba) -> impl IntoElement {
    div()
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_surface())
        .overflow_hidden()
        .children(step.bullets.iter().enumerate().map(|(index, bullet)| {
            div()
                .px(px(12.0))
                .py(px(10.0))
                .when(index > 0, |el| {
                    el.border_t(px(1.0)).border_color(border_muted())
                })
                .flex()
                .items_start()
                .gap(px(9.0))
                .child(
                    div()
                        .mt(px(1.0))
                        .w(px(18.0))
                        .h(px(18.0))
                        .flex_shrink_0()
                        .rounded(px(5.0))
                        .bg(with_alpha(tone, 0.12))
                        .border_1()
                        .border_color(with_alpha(tone, 0.28))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(lucide_icon(LucideIcon::Check, 11.0, tone)),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "onboarding-{}-bullet-{index}",
                            step.id
                        )))
                        .min_w_0()
                        .text_size(px(12.0))
                        .line_height(px(17.0))
                        .text_color(fg_default())
                        .child((*bullet).to_string()),
                )
        }))
}

fn render_wizard_progress(step_number: usize, step_count: usize, tone: Rgba) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .children((0..step_count).map(move |index| {
            div()
                .w(px(if index + 1 == step_number { 18.0 } else { 7.0 }))
                .h(px(7.0))
                .rounded(px(4.0))
                .bg(if index < step_number {
                    tone
                } else {
                    border_muted()
                })
        }))
}

fn wizard_icon_chip(icon: LucideIcon, tone: Rgba) -> impl IntoElement {
    div()
        .w(px(34.0))
        .h(px(34.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(with_alpha(tone, 0.34))
        .bg(with_alpha(tone, 0.12))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(lucide_icon(icon, 17.0, tone))
}

fn wizard_secondary_button(
    label: String,
    disabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(12.0))
        .py(px(7.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(control_button_bg())
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if disabled { fg_subtle() } else { fg_muted() })
        .opacity(if disabled { 0.62 } else { 1.0 })
        .when(!disabled, |el| {
            el.cursor_pointer()
                .hover(|style| {
                    style
                        .bg(control_button_hover_bg())
                        .text_color(fg_emphasis())
                        .border_color(border_default())
                })
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(label)
}

fn wizard_primary_button(
    label: String,
    disabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(if disabled {
            control_button_bg()
        } else {
            primary_action_bg()
        })
        .text_color(if disabled {
            fg_subtle()
        } else {
            fg_on_primary_action()
        })
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .opacity(if disabled { 0.72 } else { 1.0 })
        .when(!disabled, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(primary_action_hover()))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(label)
}

fn wizard_tone_icon(tone: WizardTone) -> LucideIcon {
    match tone {
        WizardTone::Welcome => LucideIcon::Sparkles,
        WizardTone::Workspace => LucideIcon::Plug,
        WizardTone::Review => LucideIcon::GitPullRequest,
        WizardTone::Ai => LucideIcon::Route,
        WizardTone::Local => LucideIcon::Folder,
    }
}

fn wizard_tone_color(tone: WizardTone) -> Rgba {
    match tone {
        WizardTone::Welcome => accent(),
        WizardTone::Workspace => info(),
        WizardTone::Review => success(),
        WizardTone::Ai => warning(),
        WizardTone::Local => fg_emphasis(),
    }
}
