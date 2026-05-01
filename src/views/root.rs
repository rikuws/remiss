use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use crate::github;
use crate::icons::{lucide_icon, LucideIcon};
use crate::review_session::{load_review_session, ReviewCenterMode};
use crate::state::*;
use crate::theme::*;

use super::ai_tour::refresh_active_tour;
use super::diff_view::{
    ensure_active_review_focus_loaded, enter_files_surface, enter_stack_review_mode,
};
use super::palette::render_palette;
use super::pr_detail::render_pr_workspace;
use super::sections::render_section_workspace;
use super::settings::{prepare_settings_view, update_theme_preference};
use super::workspace_sync::{
    sync_workspace_flow, trigger_sync_workspace, wait_for_workspace_poll_interval,
};

pub struct RootView {
    state: Entity<AppState>,
}

const APP_SIDEBAR_EXPANDED_WIDTH: f32 = 216.0;
const APP_SIDEBAR_HIDDEN_WIDTH: f32 = 0.0;
const APP_SIDEBAR_TRAFFIC_LIGHT_CLEARANCE: f32 = 74.0;
pub(crate) const APP_CHROME_HEIGHT: f32 = 64.0;
const APP_TITLEBAR_TOGGLE_LEFT: f32 = 88.0;
const APP_TITLEBAR_TOGGLE_SIZE: f32 = 24.0;
const APP_TITLEBAR_TOGGLE_TOP: f32 = (APP_CHROME_HEIGHT - APP_TITLEBAR_TOGGLE_SIZE) / 2.0;
const APP_TITLEBAR_TOGGLE_ICON_SIZE: f32 = 13.0;
const APP_CHROME_HIDDEN_LEFT_INSET: f32 = 206.0;
const APP_SIDEBAR_ANIMATION_MS: u64 = 220;
const NOTIFICATION_DRAWER_ANIMATION_MS: u64 = 160;

impl RootView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_appearance = window.appearance();
        state.update(cx, |state, _| {
            state.set_window_appearance(initial_appearance);
        });
        cx.observe_window_appearance(window, {
            let state = state.clone();
            move |_, window, cx| {
                let appearance = window.appearance();
                state.update(cx, |state, cx| {
                    state.set_window_appearance(appearance);
                    cx.notify();
                });
            }
        })
        .detach();

        // Bootstrap: load workspace from cache, then sync in background.
        let model = state.clone();
        cx.spawn_in(window, async move |_this, cx| {
            // Load bootstrap status
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };

            let result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move { github::load_workspace_snapshot(&cache) }
                })
                .await;

            model
                .update(cx, |state, cx| {
                    state.workspace_loading = false;
                    state.bootstrap_loading = false;
                    match &result {
                        Ok(ws) => {
                            state.gh_available = ws.auth.is_authenticated;
                            state.workspace = Some(ws.clone());
                        }
                        Err(e) => {
                            state.workspace_error = Some(e.clone());
                        }
                    }
                    cx.notify();
                })
                .ok();

            maybe_bootstrap_debug_pull_request(&model, cache.as_ref(), cx).await;

            // Check gh version
            let gh_result = cx
                .background_executor()
                .spawn(async { crate::gh::run(&["--version"]) })
                .await;

            model
                .update(cx, |state, cx| {
                    if let Ok(output) = gh_result {
                        if output.exit_code == Some(0) {
                            state.gh_available = true;
                            state.gh_version = output.stdout.lines().next().map(str::to_string);
                        }
                    }
                    cx.notify();
                })
                .ok();

            // Now sync workspace in background.
            model
                .update(cx, |state, cx| {
                    state.workspace_syncing = true;
                    cx.notify();
                })
                .ok();

            sync_workspace_flow(model.clone(), cx).await;

            loop {
                wait_for_workspace_poll_interval(cx).await;

                let should_sync = model
                    .read_with(cx, |state, _| {
                        state.is_authenticated() && !state.workspace_syncing
                    })
                    .ok()
                    .unwrap_or(false);
                if !should_sync {
                    continue;
                }

                model
                    .update(cx, |state, cx| {
                        if state.workspace_syncing {
                            return;
                        }

                        state.workspace_syncing = true;
                        cx.notify();
                    })
                    .ok();

                sync_workspace_flow(model.clone(), cx).await;
            }
        })
        .detach();

        Self { state }
    }
}

async fn maybe_bootstrap_debug_pull_request(
    model: &Entity<AppState>,
    cache: &crate::cache::CacheStore,
    cx: &mut AsyncWindowContext,
) {
    let Some(debug_target) = std::env::var("REMISS_DEBUG_OPEN_PR")
        .or_else(|_| std::env::var("REVIEWBUDDY_DEBUG_OPEN_PR"))
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };

    let Some((repository, number)) = parse_debug_pull_request_target(&debug_target) else {
        return;
    };

    let snapshot = match cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = repository.clone();
            async move { github::load_pull_request_detail(&cache, &repository, number) }
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(_) => return,
    };

    let Some(detail) = snapshot.detail.clone() else {
        return;
    };

    let review_session = load_review_session(cache, &pr_key(&repository, number))
        .ok()
        .flatten();
    let summary = github::PullRequestSummary {
        repository: detail.repository.clone(),
        number: detail.number,
        title: detail.title.clone(),
        author_login: detail.author_login.clone(),
        author_avatar_url: detail.author_avatar_url.clone(),
        is_draft: detail.is_draft,
        comments_count: detail.comments_count,
        additions: detail.additions,
        deletions: detail.deletions,
        changed_files: detail.changed_files,
        state: detail.state.clone(),
        review_decision: detail.review_decision.clone(),
        updated_at: detail.updated_at.clone(),
        url: detail.url.clone(),
    };
    let detail_key = pr_key(&repository, number);

    model
        .update(cx, |state, cx| {
            if !state
                .open_tabs
                .iter()
                .any(|tab| pr_key(&tab.repository, tab.number) == detail_key)
            {
                state.open_tabs.insert(0, summary);
            }

            state.set_active_section(SectionId::Pulls);
            state.active_surface = PullRequestSurface::Files;
            state.active_pr_key = Some(detail_key.clone());
            state.pr_header_compact = false;
            state.review_body.clear();
            state.review_editor_active = false;
            state.review_message = None;
            state.review_success = false;

            let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
            detail_state.snapshot = Some(snapshot.clone());
            detail_state.loading = false;
            detail_state.syncing = false;
            detail_state.error = None;

            state.apply_review_session_document(&detail_key, review_session.clone());
            cx.notify();
        })
        .ok();
}

fn parse_debug_pull_request_target(target: &str) -> Option<(String, i64)> {
    let (repository, number) = target.trim().rsplit_once('#')?;
    let number = number.parse::<i64>().ok()?;
    Some((repository.to_string(), number))
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let palette_open = state.palette_open;
        let notification_drawer_open = state.notification_drawer_open;

        div()
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(bg_canvas())
            .text_color(fg_default())
            .text_size(px(14.0))
            .font_family(ui_font_family())
            .child(render_app_sidebar(&self.state, cx))
            .child(render_main_column(&self.state, cx))
            .child(render_titlebar_sidebar_toggle(&self.state, cx))
            .when(notification_drawer_open, |el| {
                el.child(render_notification_drawer(&self.state, cx))
            })
            .when(palette_open, |el| el.child(render_palette(&self.state, cx)))
    }
}

fn render_app_sidebar(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let hidden = s.app_sidebar_collapsed;
    let active_section = s.active_section;
    let is_authenticated = s.is_authenticated();
    let workspace_syncing = s.workspace_syncing;
    let workspace_error = s.workspace_error.clone();
    let theme_preference = s.theme_preference;
    let sidebar_width = if hidden {
        APP_SIDEBAR_HIDDEN_WIDTH
    } else {
        APP_SIDEBAR_EXPANDED_WIDTH
    };
    let sync_label = if workspace_syncing {
        "Syncing workspace"
    } else {
        "Sync workspace"
    };
    let status_label = if workspace_syncing {
        "Syncing now"
    } else if workspace_error.is_some() {
        "Sync issue"
    } else if is_authenticated {
        "GitHub connected"
    } else {
        "gh needs auth"
    };
    let sync_color = if workspace_syncing {
        accent()
    } else if workspace_error.is_some() {
        danger()
    } else if is_authenticated {
        success()
    } else {
        fg_muted()
    };

    let state_for_nav = state.clone();
    let state_for_sync = state.clone();
    let state_for_theme = state.clone();
    let animation_key = ("app-sidebar", usize::from(hidden));

    div()
        .w(px(sidebar_width))
        .flex_shrink_0()
        .min_h_0()
        .bg(bg_overlay())
        .border_r(if hidden { px(0.0) } else { px(1.0) })
        .border_color(border_muted())
        .overflow_hidden()
        .child(
            div()
                .w(px(APP_SIDEBAR_EXPANDED_WIDTH))
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .justify_between()
                .opacity(if hidden { 0.0 } else { 1.0 })
                .child(
                    div()
                        .px(px(14.0))
                        .pt(px(APP_SIDEBAR_TRAFFIC_LIGHT_CLEARANCE))
                        .pb(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children(
                            SectionId::all()
                                .iter()
                                .filter(|section| **section != SectionId::Issues)
                                .map(|section| {
                                    let section = *section;
                                    let count = s.section_count(section);
                                    let state = state_for_nav.clone();
                                    sidebar_nav_button(
                                        section.label(),
                                        sidebar_icon_for_section(section),
                                        count,
                                        active_section == section,
                                        false,
                                        move |_, window, cx| {
                                            if section == SectionId::Settings {
                                                prepare_settings_view(&state, window, cx);
                                            }
                                            state.update(cx, |s, cx| {
                                                s.set_active_section(section);
                                                s.active_pr_key = None;
                                                s.palette_open = false;
                                                s.palette_selected_index = 0;
                                                cx.notify();
                                            });
                                        },
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .px(px(14.0))
                        .pb(px(14.0))
                        .pt(px(12.0))
                        .border_t(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(7.0))
                                .rounded(radius_sm())
                                .bg(bg_surface())
                                .border_1()
                                .border_color(border_muted())
                                .text_size(px(11.0))
                                .font_family(mono_font_family())
                                .text_color(sync_color)
                                .child(status_label),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .text_size(px(10.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child("THEME"),
                                )
                                .child(div().flex().gap(px(6.0)).flex_row().children(
                                    ThemePreference::all().iter().map(|candidate| {
                                        let candidate = *candidate;
                                        let state = state_for_theme.clone();
                                        sidebar_theme_button(
                                            theme_icon(candidate),
                                            theme_preference == candidate,
                                            false,
                                            move |_, window, cx| {
                                                update_theme_preference(
                                                    &state, candidate, window, cx,
                                                );
                                            },
                                        )
                                    }),
                                )),
                        )
                        .child(sidebar_action_button(
                            LucideIcon::RefreshCw,
                            sync_label,
                            false,
                            sync_color,
                            move |_, window, cx| {
                                trigger_sync_workspace(&state_for_sync, window, cx)
                            },
                        )),
                ),
        )
        .with_animation(
            animation_key,
            Animation::new(Duration::from_millis(APP_SIDEBAR_ANIMATION_MS))
                .with_easing(ease_in_out),
            move |el, delta| {
                let progress = sidebar_hidden_progress(hidden, delta);
                el.w(lerp_px(
                    APP_SIDEBAR_EXPANDED_WIDTH,
                    APP_SIDEBAR_HIDDEN_WIDTH,
                    progress,
                ))
            },
        )
}

fn render_main_column(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    div()
        .flex_grow()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .child(render_workspace_chrome(state, cx))
        .child(render_workspace_body(state, cx))
}

fn render_titlebar_sidebar_toggle(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let hidden = state.read(cx).app_sidebar_collapsed;
    let state = state.clone();
    let tooltip = if hidden {
        "Show sidebar"
    } else {
        "Hide sidebar"
    };

    div()
        .absolute()
        .left(px(APP_TITLEBAR_TOGGLE_LEFT))
        .top(px(APP_TITLEBAR_TOGGLE_TOP))
        .child(titlebar_icon_button(
            "titlebar-sidebar-toggle",
            LucideIcon::PanelLeft,
            tooltip,
            false,
            move |_, _, cx| {
                state.update(cx, |state, cx| {
                    state.app_sidebar_collapsed = !hidden;
                    cx.notify();
                });
            },
        ))
}

fn render_workspace_chrome(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let active_pr_key = s.active_pr_key.clone();
    let active_surface = s.active_surface;
    let active_center_mode = s
        .active_review_session()
        .map(|session| session.center_mode)
        .unwrap_or(ReviewCenterMode::SemanticDiff);
    let active_code_lens = s.active_code_lens_mode();
    let has_active_pr = active_pr_key.is_some();
    let unread_count = s
        .active_detail()
        .map(|detail| s.unread_review_comment_ids_for_detail(detail).len())
        .unwrap_or(0);
    let drawer_open = s.notification_drawer_open;
    let sidebar_hidden = s.app_sidebar_collapsed;
    let tabs: Vec<_> = s.open_tabs.clone();
    let state_for_tabs = state.clone();
    let state_for_notifications = state.clone();
    let state_for_briefing = state.clone();
    let state_for_review = state.clone();
    let state_for_code = state.clone();
    let state_for_ai_tour = state.clone();
    let state_for_stack = state.clone();
    let state_for_diff_lens = state.clone();
    let state_for_source_lens = state.clone();
    let code_mode_active = matches!(
        active_center_mode,
        ReviewCenterMode::SemanticDiff | ReviewCenterMode::SourceBrowser
    );

    div()
        .h(px(APP_CHROME_HEIGHT))
        .flex_shrink_0()
        .bg(bg_canvas())
        .border_b(px(1.0))
        .border_color(border_muted())
        .pl(if sidebar_hidden {
            px(APP_CHROME_HIDDEN_LEFT_INSET)
        } else {
            px(14.0)
        })
        .pr(px(14.0))
        .py(px(10.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(render_workspace_tabs(state_for_tabs, active_pr_key, tabs))
        .when(
            has_active_pr && active_surface == PullRequestSurface::Files && code_mode_active,
            |el| {
                el.child(chrome_segmented_control(vec![
                    chrome_segment(
                        "Diff",
                        active_code_lens == ReviewCenterMode::SemanticDiff,
                        false,
                        move |_, _, cx| {
                            state_for_diff_lens.update(cx, |state, cx| {
                                state.set_review_center_mode(ReviewCenterMode::SemanticDiff);
                                state.persist_active_review_session();
                                cx.notify();
                            });
                        },
                    ),
                    chrome_segment(
                        "Source",
                        active_code_lens == ReviewCenterMode::SourceBrowser,
                        false,
                        move |_, window, cx| {
                            state_for_source_lens.update(cx, |state, cx| {
                                state.set_review_center_mode(ReviewCenterMode::SourceBrowser);
                                state.persist_active_review_session();
                                cx.notify();
                            });
                            ensure_active_review_focus_loaded(&state_for_source_lens, window, cx);
                        },
                    ),
                ]))
            },
        )
        .when(has_active_pr, |el| {
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_shrink_0()
                    .child(chrome_segmented_control(vec![
                        chrome_segment(
                            "Briefing",
                            active_surface == PullRequestSurface::Overview,
                            false,
                            move |_, _, cx| {
                                state_for_briefing.update(cx, |state, cx| {
                                    state.active_surface = PullRequestSurface::Overview;
                                    state.pr_header_compact = false;
                                    state.persist_active_review_session();
                                    cx.notify();
                                });
                            },
                        ),
                        chrome_segment(
                            "Review",
                            active_surface == PullRequestSurface::Files,
                            false,
                            move |_, window, cx| {
                                enter_files_surface(&state_for_review, window, cx);
                            },
                        ),
                    ]))
                    .child(chrome_segmented_control(vec![
                        chrome_segment(
                            "Code",
                            code_mode_active,
                            active_surface != PullRequestSurface::Files,
                            move |_, window, cx| {
                                state_for_code.update(cx, |state, cx| {
                                    state.active_surface = PullRequestSurface::Files;
                                    state.enter_code_review_mode();
                                    state.persist_active_review_session();
                                    cx.notify();
                                });
                                ensure_active_review_focus_loaded(&state_for_code, window, cx);
                            },
                        ),
                        chrome_segment(
                            "AI Tour",
                            active_center_mode == ReviewCenterMode::AiTour,
                            active_surface != PullRequestSurface::Files,
                            move |_, window, cx| {
                                state_for_ai_tour.update(cx, |state, cx| {
                                    state.active_surface = PullRequestSurface::Files;
                                    state.set_review_center_mode(ReviewCenterMode::AiTour);
                                    state.persist_active_review_session();
                                    cx.notify();
                                });
                                refresh_active_tour(&state_for_ai_tour, window, cx, true);
                            },
                        ),
                        chrome_segment(
                            "Stack",
                            active_center_mode == ReviewCenterMode::Stack,
                            active_surface != PullRequestSurface::Files,
                            move |_, window, cx| {
                                enter_stack_review_mode(&state_for_stack, window, cx);
                            },
                        ),
                    ])),
            )
        })
        .child(
            div()
                .relative()
                .child(titlebar_icon_button(
                    "workspace-notification-drawer",
                    LucideIcon::Bell,
                    "Notifications",
                    drawer_open,
                    move |_, _, cx| {
                        state_for_notifications.update(cx, |state, cx| {
                            state.notification_drawer_open = !drawer_open;
                            cx.notify();
                        });
                    },
                ))
                .when(unread_count > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(-5.0))
                            .right(px(-5.0))
                            .min_w(px(14.0))
                            .h(px(14.0))
                            .px(px(3.0))
                            .rounded(px(999.0))
                            .bg(danger())
                            .text_size(px(9.0))
                            .font_family(mono_font_family())
                            .text_color(bg_canvas())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(unread_count.min(99).to_string()),
                    )
                })
                .into_any_element(),
        )
}

fn titlebar_icon_button(
    id: &'static str,
    icon: LucideIcon,
    tooltip: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id =
        SharedString::from(format!("titlebar-icon-button-{id}-{}", usize::from(active)));
    let focus_border_transparent = with_alpha(focus_border(), 0.0);

    div()
        .id(id)
        .w(px(APP_TITLEBAR_TOGGLE_SIZE))
        .h(px(APP_TITLEBAR_TOGGLE_SIZE))
        .rounded(px(6.0))
        .bg(if active { bg_selected() } else { transparent() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            APP_TITLEBAR_TOGGLE_ICON_SIZE,
            if active { fg_emphasis() } else { fg_subtle() },
        ))
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_selected(), progress))
                    .border_color(mix_rgba(focus_border_transparent, focus_border(), progress))
            },
        )
}

fn render_workspace_tabs(
    state: Entity<AppState>,
    active_pr_key: Option<String>,
    tabs: Vec<github::PullRequestSummary>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .id("workspace-tabs-scroll")
        .overflow_x_scroll()
        .min_w_0()
        .flex_grow()
        .children(tabs.into_iter().map(|tab| {
            let key = pr_key(&tab.repository, tab.number);
            let is_active = active_pr_key.as_deref() == Some(&key);
            let state = state.clone();
            pr_tab(
                &tab.repository,
                tab.number,
                &tab.title,
                tab.additions,
                tab.deletions,
                &tab.state,
                tab.is_draft,
                is_active,
                move |_, _, cx| {
                    state.update(cx, |s, cx| {
                        s.active_pr_key = Some(key.clone());
                        s.set_active_section(SectionId::Pulls);
                        s.palette_open = false;
                        s.palette_selected_index = 0;
                        cx.notify();
                    });
                },
            )
        }))
}

fn render_workspace_body(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let has_active_pr = s.active_pr_key.is_some();

    div()
        .flex_grow()
        .min_h_0()
        .flex()
        .flex_col()
        .child(if has_active_pr {
            render_pr_workspace(state, cx).into_any_element()
        } else {
            render_section_workspace(state, cx).into_any_element()
        })
}

fn chrome_icon_button(
    id: &'static str,
    icon: LucideIcon,
    tooltip: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .w(px(34.0))
        .h(px(34.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            border_muted()
        })
        .bg(if active { bg_selected() } else { bg_overlay() })
        .shadow_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(hover_bg())
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            16.0,
            if active { fg_emphasis() } else { fg_muted() },
        ))
}

fn chrome_segmented_control(children: Vec<AnyElement>) -> impl IntoElement {
    div()
        .h(px(34.0))
        .p(px(3.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_overlay())
        .shadow_sm()
        .flex()
        .items_center()
        .gap(px(2.0))
        .children(children)
}

fn chrome_segment(
    label: &'static str,
    active: bool,
    disabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let animation_id =
        SharedString::from(format!("chrome-segment-{label}-{}", usize::from(active)));
    let focus_border_transparent = with_alpha(focus_border(), 0.0);

    div()
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
        .bg(if active { bg_selected() } else { transparent() })
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .flex()
        .items_center()
        .justify_center()
        .opacity(if disabled { 0.5 } else { 1.0 })
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(hover_bg())
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if !disabled {
                on_click(event, window, cx);
            }
        })
        .child(label)
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_selected(), progress))
                    .border_color(mix_rgba(focus_border_transparent, focus_border(), progress))
                    .text_color(mix_rgba(fg_muted(), fg_emphasis(), progress))
            },
        )
        .into_any_element()
}

fn render_notification_drawer(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let active_detail = s.active_detail();
    let unread_ids = active_detail
        .map(|detail| s.unread_review_comment_ids_for_detail(detail))
        .unwrap_or_default();
    let unread_id_set = unread_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let unread_items = active_detail
        .map(|detail| {
            detail
                .review_threads
                .iter()
                .flat_map(|thread| {
                    thread.comments.iter().filter_map(|comment| {
                        unread_id_set.contains(&comment.id).then(|| {
                            (
                                comment.id.clone(),
                                comment.author_login.clone(),
                                comment.path.clone(),
                                comment.line.or(comment.original_line),
                                truncate_drawer_text(&comment.body, 96),
                            )
                        })
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let state_for_close = state.clone();
    let state_for_mark_read = state.clone();

    div()
        .absolute()
        .top(px(64.0))
        .right(px(16.0))
        .w(px(360.0))
        .max_h(px(520.0))
        .rounded(radius())
        .border_1()
        .border_color(border_default())
        .bg(bg_overlay())
        .shadow_md()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .border_b(px(1.0))
                .border_color(border_muted())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Unread review activity"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(fg_muted())
                                .child(format!(
                                    "{} unread comment{}",
                                    unread_items.len(),
                                    if unread_items.len() == 1 { "" } else { "s" }
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .when(!unread_ids.is_empty(), |el| {
                            let unread_ids = unread_ids.clone();
                            el.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(radius_sm())
                                    .text_size(px(11.0))
                                    .text_color(fg_muted())
                                    .cursor_pointer()
                                    .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        state_for_mark_read.update(cx, |state, cx| {
                                            state.mark_review_comments_read(unread_ids.clone());
                                            state.notification_drawer_open = false;
                                            cx.notify();
                                        });
                                    })
                                    .child("Mark read"),
                            )
                        })
                        .child(chrome_icon_button(
                            "notification-drawer-close",
                            LucideIcon::X,
                            "Close notifications",
                            false,
                            move |_, _, cx| {
                                state_for_close.update(cx, |state, cx| {
                                    state.notification_drawer_open = false;
                                    cx.notify();
                                });
                            },
                        )),
                ),
        )
        .child(
            div()
                .id("notification-drawer-scroll")
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(px(10.0))
                .gap(px(8.0))
                .when(unread_items.is_empty(), |el| {
                    el.child(
                        div()
                            .px(px(10.0))
                            .py(px(18.0))
                            .rounded(radius_sm())
                            .border_1()
                            .border_color(border_muted())
                            .bg(bg_surface())
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .child("No unread review comments."),
                    )
                })
                .children(
                    unread_items
                        .into_iter()
                        .map(|(_id, author, path, line, body)| {
                            div()
                                .rounded(radius_sm())
                                .border_1()
                                .border_color(border_muted())
                                .bg(bg_surface())
                                .px(px(10.0))
                                .py(px(9.0))
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child(author),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .font_family(mono_font_family())
                                                .text_color(fg_muted())
                                                .child(
                                                    line.map(|line| format!("L{line}"))
                                                        .unwrap_or_default(),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_muted())
                                        .overflow_x_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(path),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .line_height(px(17.0))
                                        .text_color(fg_default())
                                        .child(body),
                                )
                        }),
                ),
        )
        .with_animation(
            "notification-drawer-open",
            Animation::new(Duration::from_millis(NOTIFICATION_DRAWER_ANIMATION_MS))
                .with_easing(ease_in_out),
            move |el, delta| {
                el.mt(lerp_px(-8.0, 0.0, delta))
                    .opacity(delta.clamp(0.0, 1.0))
            },
        )
}

fn truncate_drawer_text(text: &str, limit: usize) -> String {
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() <= limit {
        trimmed
    } else {
        let mut out = trimmed.chars().take(limit).collect::<String>();
        out.push('…');
        out
    }
}

fn sidebar_hidden_progress(hidden: bool, delta: f32) -> f32 {
    if hidden {
        delta
    } else {
        1.0 - delta
    }
}

fn lerp_px(from: f32, to: f32, progress: f32) -> Pixels {
    px(from + (to - from) * progress)
}

fn build_static_tooltip(text: &'static str, cx: &mut App) -> AnyView {
    AnyView::from(cx.new(|_| ChromeTooltipView {
        text: SharedString::from(text),
    }))
}

struct ChromeTooltipView {
    text: SharedString,
}

impl Render for ChromeTooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(radius_sm())
            .border_1()
            .border_color(border_muted())
            .bg(bg_overlay())
            .text_size(px(11.0))
            .text_color(fg_default())
            .child(self.text.clone())
    }
}

fn sidebar_icon_for_section(section: SectionId) -> LucideIcon {
    match section {
        SectionId::Overview => LucideIcon::LayoutDashboard,
        SectionId::Pulls => LucideIcon::GitPullRequest,
        SectionId::Reviews => LucideIcon::MessagesSquare,
        SectionId::Settings => LucideIcon::Settings,
        SectionId::Issues => LucideIcon::Inbox,
    }
}

fn theme_icon(preference: ThemePreference) -> LucideIcon {
    match preference {
        ThemePreference::System => LucideIcon::Monitor,
        ThemePreference::Light => LucideIcon::Sun,
        ThemePreference::Dark => LucideIcon::Moon,
    }
}

fn sidebar_nav_button(
    label: &str,
    icon: LucideIcon,
    count: i64,
    active: bool,
    collapsed: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "sidebar-nav-button-{label}-{}",
        usize::from(active)
    ));
    let focus_border_transparent = with_alpha(focus_border(), 0.0);

    div()
        .h(px(38.0))
        .px(px(10.0))
        .when(collapsed, |el| el.px(px(0.0)))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            transparent()
        })
        .when(active, |el| el.bg(bg_selected()))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(hover_bg())
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .justify_center()
                .when(!collapsed, |el| el.justify_start())
                .flex_grow()
                .min_w_0()
                .child(lucide_icon(
                    icon,
                    18.0,
                    if active { fg_emphasis() } else { fg_muted() },
                ))
                .when(!collapsed, |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if active { fg_emphasis() } else { fg_default() })
                            .child(label.to_string()),
                    )
                }),
        )
        .when(!collapsed && count > 0, |el| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .font_family(mono_font_family())
                    .text_color(if active { fg_default() } else { fg_subtle() })
                    .child(count.to_string()),
            )
        })
        .when(collapsed, |el| el.justify_center())
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(transparent(), bg_selected(), progress))
                    .border_color(mix_rgba(focus_border_transparent, focus_border(), progress))
            },
        )
}

fn sidebar_theme_button(
    icon: LucideIcon,
    active: bool,
    collapsed: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "sidebar-theme-button-{}-{}",
        icon.unicode(),
        usize::from(active)
    ));

    div()
        .h(px(34.0))
        .when(collapsed, |el| el.w_full())
        .when(!collapsed, |el| el.flex_1())
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            border_muted()
        })
        .bg(if active { bg_selected() } else { bg_overlay() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            16.0,
            if active { fg_emphasis() } else { fg_muted() },
        ))
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(bg_overlay(), bg_selected(), progress))
                    .border_color(mix_rgba(border_muted(), focus_border(), progress))
            },
        )
}

fn sidebar_utility_button(
    icon: LucideIcon,
    active: bool,
    bordered: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if bordered {
            border_muted()
        } else {
            transparent()
        })
        .bg(if active { bg_selected() } else { bg_surface() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(
            icon,
            16.0,
            if active { fg_emphasis() } else { fg_muted() },
        ))
}

fn sidebar_action_button(
    icon: LucideIcon,
    label: &str,
    collapsed: bool,
    icon_color: Rgba,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(36.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_surface())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .when(!collapsed, |el| el.px(px(10.0)).justify_start())
        .when(collapsed, |el| el.w_full())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(lucide_icon(icon, 16.0, icon_color))
        .when(!collapsed, |el| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg_default())
                    .child(label.to_string()),
            )
        })
}

fn pr_tab(
    repository: &str,
    number: i64,
    title: &str,
    _additions: i64,
    _deletions: i64,
    pr_state: &str,
    is_draft: bool,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "pr-tab-{repository}-{number}-{}",
        usize::from(active)
    ));
    let dot_color = pr_tab_state_dot(pr_state, is_draft);
    let state_badge = pr_tab_state_badge(pr_state, is_draft);
    let repo_short = repository
        .split('/')
        .last()
        .unwrap_or(repository)
        .to_string();
    let tab_label = format!("#{number} {title}");

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(5.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(if active {
            focus_border()
        } else {
            border_muted()
        })
        .bg(if active { bg_selected() } else { bg_overlay() })
        .text_size(px(11.0))
        .max_w(px(280.0))
        .min_w_0()
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if active { bg_selected() } else { hover_bg() })
                .border_color(focus_border())
                .text_color(fg_emphasis())
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .min_w_0()
                .flex_grow()
                .child(
                    div()
                        .w(px(5.0))
                        .h(px(5.0))
                        .rounded(px(999.0))
                        .bg(dot_color)
                        .flex_shrink_0(),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(999.0))
                        .bg(if active { bg_overlay() } else { bg_emphasis() })
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(if active { fg_default() } else { fg_subtle() })
                        .flex_shrink_0()
                        .child(repo_short),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { fg_emphasis() } else { fg_default() })
                        .child(tab_label),
                ),
        )
        .when_some(state_badge, |el, badge| el.child(badge))
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(bg_overlay(), bg_selected(), progress))
                    .border_color(mix_rgba(border_muted(), focus_border(), progress))
            },
        )
}

fn pr_tab_state_dot(pr_state: &str, is_draft: bool) -> Rgba {
    if is_draft {
        return fg_muted();
    }

    match pr_state {
        "MERGED" => info(),
        "CLOSED" => danger(),
        _ => success(),
    }
}

fn pr_tab_state_badge(pr_state: &str, is_draft: bool) -> Option<AnyElement> {
    if is_draft {
        return Some(
            pr_tab_badge("Draft", fg_muted(), bg_emphasis(), border_muted()).into_any_element(),
        );
    }

    match pr_state {
        "MERGED" => Some(pr_tab_badge("Merged", info(), info_muted(), info()).into_any_element()),
        "CLOSED" => Some(
            pr_tab_badge("Closed", danger(), danger_muted(), diff_remove_border())
                .into_any_element(),
        ),
        _ => None,
    }
}

fn pr_tab_badge(label: &str, fg: Rgba, bg: Rgba, border: Rgba) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.to_string())
}
