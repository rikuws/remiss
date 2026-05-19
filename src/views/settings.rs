use std::collections::BTreeSet;

use gpui::prelude::*;
use gpui::*;

use crate::branding::APP_NAME;
use crate::icons::{lucide_icon, LucideIcon};
use crate::managed_lsp::{
    self, ManagedServerInstallState, ManagedServerInstallStatus, ManagedServerKind,
};
use crate::review_ai::{self, ReviewAiProvider, ReviewAiProviderStatus};
use crate::selectable_text::SelectableText;
use crate::state::{AppState, ManagedLspSettingsState};
use crate::theme::*;
use crate::{app_storage, diagnostic_logs, platform_macos};

use super::pr_detail::surface_tab;
use super::sections::{
    badge, error_text, eyebrow, ghost_button, panel, panel_state_text, success_text,
};

pub fn ensure_managed_lsp_statuses_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let settings = state.read(cx).managed_lsp_settings.clone();
    let should_refresh = !settings.loaded && !settings.loading;
    if should_refresh {
        trigger_managed_lsp_status_refresh(state, window, cx);
    }
}

pub fn ensure_review_ai_settings_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let review_ai_settings = state.read(cx).review_ai_settings.clone();
    if !review_ai_settings.loaded && !review_ai_settings.loading {
        trigger_review_ai_settings_refresh(state, window, cx);
    }

    let should_refresh_statuses = {
        let state = state.read(cx);
        !state.review_ai_provider_statuses_loaded && !state.review_ai_provider_loading
    };
    if should_refresh_statuses {
        trigger_review_ai_provider_status_refresh(state, window, cx);
    }
}

pub fn prepare_settings_view(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    ensure_review_ai_settings_loaded(state, window, cx);
    ensure_managed_lsp_statuses_loaded(state, window, cx);
    let scroll_handle = state.read(cx).settings_scroll_handle.clone();
    scroll_handle.set_offset(point(px(0.0), px(0.0)));
    window.on_next_frame(move |_, _| {
        scroll_handle.set_offset(point(px(0.0), px(0.0)));
    });
}

pub fn trigger_software_update_check(state: &Entity<AppState>, cx: &mut App) {
    let result = platform_macos::updates::check_for_updates();
    state.update(cx, |state, cx| {
        match result {
            Ok(()) => {
                state.software_update_message =
                    Some("Opened the Remiss update checker.".to_string());
                state.software_update_error = None;
            }
            Err(error) => {
                state.software_update_message = None;
                state.software_update_error = Some(error);
            }
        }
        cx.notify();
    });
}

pub fn trigger_diagnostic_log_export(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.diagnostic_export_loading {
            return;
        }
        state.diagnostic_export_loading = true;
        state.diagnostic_export_message = None;
        state.diagnostic_export_error = None;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async { diagnostic_logs::export_diagnostic_logs_zip() })
                .await;

            model
                .update(cx, |state, cx| {
                    state.diagnostic_export_loading = false;
                    match result {
                        Ok(path) => {
                            diagnostic_logs::reveal_export(&path);
                            state.diagnostic_export_message =
                                Some(format!("Exported logs to {}.", path.display()));
                            state.diagnostic_export_error = None;
                        }
                        Err(error) => {
                            state.diagnostic_export_message = None;
                            state.diagnostic_export_error = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn trigger_managed_lsp_status_refresh(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.managed_lsp_settings.loading {
            return;
        }
        state.managed_lsp_settings.loading = true;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let statuses = cx
                .background_executor()
                .spawn(async move {
                    ManagedServerKind::all()
                        .iter()
                        .copied()
                        .map(|kind| (kind, managed_lsp::inspect_managed_server(kind)))
                        .collect::<Vec<_>>()
                })
                .await;

            model
                .update(cx, |state, cx| {
                    let settings = &mut state.managed_lsp_settings;
                    settings.statuses = statuses.into_iter().collect();
                    settings.loading = false;
                    settings.loaded = true;
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn trigger_review_ai_settings_refresh(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.review_ai_settings.loading {
            return;
        }
        state.review_ai_settings.loading = true;
        state.review_ai_settings.error = None;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move { review_ai::load_review_ai_settings(&cache) }
                })
                .await;

            model
                .update(cx, |state, cx| {
                    state.review_ai_settings.loading = false;
                    match result {
                        Ok(settings) => {
                            state.review_ai_settings.settings = settings;
                            state.review_ai_settings.loaded = true;
                            state.review_ai_settings.error = None;
                        }
                        Err(error) => {
                            state.review_ai_settings.loaded = false;
                            state.review_ai_settings.error = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn trigger_review_ai_provider_status_refresh(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.review_ai_provider_loading {
            return;
        }
        state.review_ai_provider_loading = true;
        state.review_ai_provider_error = None;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async { review_ai::load_review_ai_provider_statuses() })
                .await;

            model
                .update(cx, |state, cx| {
                    state.review_ai_provider_loading = false;
                    match result {
                        Ok(statuses) => {
                            state.review_ai_provider_statuses = statuses;
                            state.review_ai_provider_statuses_loaded = true;
                            state.review_ai_provider_error = None;
                        }
                        Err(error) => {
                            state.review_ai_provider_error = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn update_theme_preference(
    state: &Entity<AppState>,
    preference: ThemePreference,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).theme_preference == preference {
        return;
    }

    let cache = state.read(cx).cache.clone();
    let code_font_size = state.read(cx).code_font_size_preference;
    let diff_color_theme = state.read(cx).diff_color_theme_preference;
    state.update(cx, |state, cx| {
        state.set_theme_preference(preference);
        cx.notify();
    });

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let _ = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move {
                        crate::theme::save_theme_settings(
                            &cache,
                            &crate::theme::ThemeSettings {
                                preference,
                                code_font_size,
                                diff_color_theme,
                            },
                        )
                    }
                })
                .await;
        })
        .detach();
}

pub fn update_code_font_size_preference(
    state: &Entity<AppState>,
    code_font_size: CodeFontSizePreference,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).code_font_size_preference == code_font_size {
        return;
    }

    let cache = state.read(cx).cache.clone();
    let preference = state.read(cx).theme_preference;
    let diff_color_theme = state.read(cx).diff_color_theme_preference;
    state.update(cx, |state, cx| {
        state.set_code_font_size_preference(code_font_size);
        cx.notify();
    });

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let _ = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move {
                        crate::theme::save_theme_settings(
                            &cache,
                            &crate::theme::ThemeSettings {
                                preference,
                                code_font_size,
                                diff_color_theme,
                            },
                        )
                    }
                })
                .await;
        })
        .detach();
}

pub fn increase_code_font_size_preference(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let next = state.read(cx).code_font_size_preference.larger();
    update_code_font_size_preference(state, next, window, cx);
}

pub fn decrease_code_font_size_preference(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let next = state.read(cx).code_font_size_preference.smaller();
    update_code_font_size_preference(state, next, window, cx);
}

pub fn reset_code_font_size_preference(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    update_code_font_size_preference(state, CodeFontSizePreference::default_size(), window, cx);
}

pub fn update_diff_color_theme_preference(
    state: &Entity<AppState>,
    diff_color_theme: DiffColorThemePreference,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).diff_color_theme_preference == diff_color_theme {
        return;
    }

    save_diff_color_theme_preference(state, diff_color_theme, window, cx);
}

pub fn save_diff_color_theme_preference(
    state: &Entity<AppState>,
    diff_color_theme: DiffColorThemePreference,
    window: &mut Window,
    cx: &mut App,
) {
    let cache = state.read(cx).cache.clone();
    let preference = state.read(cx).theme_preference;
    let code_font_size = state.read(cx).code_font_size_preference;
    state.update(cx, |state, cx| {
        state.set_diff_color_theme_preference(diff_color_theme);
        cx.notify();
    });

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let _ = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move {
                        crate::theme::save_theme_settings(
                            &cache,
                            &crate::theme::ThemeSettings {
                                preference,
                                code_font_size,
                                diff_color_theme,
                            },
                        )
                    }
                })
                .await;
        })
        .detach();
}

pub fn cycle_diff_color_theme_preference(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let next = state.read(cx).diff_color_theme_preference.next();
    update_diff_color_theme_preference(state, next, window, cx);
}

fn trigger_managed_lsp_install(
    state: &Entity<AppState>,
    kind: ManagedServerKind,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        let settings = &mut state.managed_lsp_settings;
        if settings.installing.contains(&kind) {
            return;
        }

        settings.installing.insert(kind);
        settings.install_errors.remove(&kind);
        settings.install_messages.remove(&kind);
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async move { managed_lsp::install_managed_server(kind) })
                .await;

            model
                .update(cx, |state, cx| {
                    let settings = &mut state.managed_lsp_settings;
                    settings.installing.remove(&kind);
                    settings.loaded = true;

                    match result {
                        Ok(status) => {
                            settings.statuses.insert(kind, status);
                            settings.install_errors.remove(&kind);
                            settings.install_messages.insert(
                                kind,
                                format!(
                                    "{} is downloaded.",
                                    managed_lsp::managed_server_display_name(kind)
                                ),
                            );
                        }
                        Err(error) => {
                            settings
                                .statuses
                                .insert(kind, managed_lsp::inspect_managed_server(kind));
                            settings.install_messages.remove(&kind);
                            settings.install_errors.insert(kind, error);
                        }
                    }

                    cx.notify();
                })
                .ok();
        })
        .detach();
}

fn update_review_ai_settings(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    update: impl FnOnce(&mut crate::review_ai::ReviewAiSettings),
) {
    let cache = state.read(cx).cache.clone();
    let mut next_settings = state.read(cx).review_ai_settings.settings.clone();
    update(&mut next_settings);

    state.update(cx, |state, cx| {
        state.review_ai_settings.settings = next_settings.clone();
        state.review_ai_settings.loaded = true;
        state.review_ai_settings.error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let settings = next_settings.clone();
                    async move { review_ai::save_review_ai_settings(&cache, &settings) }
                })
                .await;

            if let Err(error) = result {
                model
                    .update(cx, |state, cx| {
                        state.review_ai_settings.error = Some(error);
                        cx.notify();
                    })
                    .ok();
            }
        })
        .detach();
}

pub fn render_settings_view(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let settings = &s.managed_lsp_settings;
    let loading = settings.loading;
    let loaded = settings.loaded;
    let storage_root = app_storage::data_dir_root();

    div()
        .p(px(40.0))
        .px(px(48.0))
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .id("settings-scroll")
        .overflow_y_scroll()
        .track_scroll(&s.settings_scroll_handle)
        .child(
            div().w_full().flex().justify_center().child(
                div()
                    .w_full()
                    .min_w_0()
                    .max_w(px(1040.0))
                    .flex()
                    .flex_col()
                    .gap(px(24.0))
                    .child(render_theme_settings_panel(state, &s))
                    .child(render_software_update_panel(state, &s))
                    .child(render_diagnostic_logs_panel(state, &s))
                    .child(render_review_intelligence_settings_panel(state, &s))
                    .child(
                        panel().child(
                            div()
                                .p(px(28.0))
                                .px(px(32.0))
                                .flex()
                                .flex_col()
                                .gap(px(16.0))
                                .child(eyebrow("Settings / Language Servers"))
                                .child(
                                    div()
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child("Managed language servers"),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(fg_muted())
                                        .max_w(px(760.0))
                                        .child(
                                            "Download or repair the LSPs Remiss can manage itself. This screen also surfaces install failures and broken local metadata.",
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.0))
                                        .items_center()
                                        .child(ghost_button(
                                            if loading { "Refreshing..." } else { "Refresh statuses" },
                                            {
                                                let state = state.clone();
                                                move |_, window, cx| {
                                                    trigger_managed_lsp_status_refresh(
                                                        &state, window, cx,
                                                    );
                                                }
                                            },
                                        ))
                                        .when(loading, |el| {
                                            el.child(panel_state_text(
                                                "Checking managed server state...",
                                            ))
                                        }),
                                ),
                        ),
                    )
                    .when(!loaded && !loading, |el| {
                        el.child(panel_state_text(
                            "Open this screen after startup to check which managed servers are already installed.",
                        ))
                    })
                    .child(
                        panel().child(
                            div()
                                .p(px(24.0))
                                .px(px(32.0))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child("Storage"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(fg_muted())
                                        .child("App-managed files are stored here."),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(SelectableText::new(
                                            "settings-storage-root",
                                            storage_root.display().to_string(),
                                        )),
                                ),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .children(
                                ManagedServerKind::all()
                                    .iter()
                                    .copied()
                                    .map(|kind| {
                                        render_managed_lsp_card(state, settings, kind)
                                            .into_any_element()
                                    }),
                            ),
                    ),
            ),
        )
}

fn render_software_update_panel(state: &Entity<AppState>, s: &AppState) -> impl IntoElement {
    let status = platform_macos::updates::updater_status();
    let message = s.software_update_message.clone();
    let error = s.software_update_error.clone();
    let running_version = format!("{APP_NAME} v{}", platform_macos::app_short_version());

    panel().child(
        div()
            .p(px(28.0))
            .px(px(32.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(eyebrow("Settings / Updates"))
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_emphasis())
                    .child("Software updates"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(fg_muted())
                    .max_w(px(760.0))
                    .child(status.detail),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Running version"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_family(mono_font_family())
                            .text_color(fg_subtle())
                            .child(SelectableText::new(
                                "settings-remiss-version",
                                running_version,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_wrap()
                    .child(badge(if status.available {
                        "updater ready"
                    } else {
                        "updater unavailable"
                    }))
                    .child(ghost_button("Check for Updates", {
                        let state = state.clone();
                        move |_, _, cx| {
                            trigger_software_update_check(&state, cx);
                        }
                    })),
            )
            .when_some(message, |el, message| el.child(success_text(&message)))
            .when_some(error, |el, error| el.child(error_text(&error))),
    )
}

fn render_diagnostic_logs_panel(state: &Entity<AppState>, s: &AppState) -> impl IntoElement {
    let loading = s.diagnostic_export_loading;
    let message = s.diagnostic_export_message.clone();
    let error = s.diagnostic_export_error.clone();

    panel().child(
        div()
            .p(px(28.0))
            .px(px(32.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(eyebrow("Settings / Diagnostics"))
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_emphasis())
                    .child("Diagnostic logs"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(fg_muted())
                    .max_w(px(760.0))
                    .child(
                        "Export recent Copilot, AI stack, and checkout logs as a zip file in Downloads.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_wrap()
                    .child(badge("local archive"))
                    .child(ghost_button(
                        if loading {
                            "Exporting logs..."
                        } else {
                            "Export Logs Zip"
                        },
                        {
                            let state = state.clone();
                            move |_, window, cx| {
                                trigger_diagnostic_log_export(&state, window, cx);
                            }
                        },
                    ))
                    .when(loading, |el| {
                        el.child(panel_state_text(
                            "Collecting logs from Application Support...",
                        ))
                    }),
            )
            .when_some(message, |el, message| el.child(success_text(&message)))
            .when_some(error, |el, error| el.child(error_text(&error))),
    )
}

fn render_theme_settings_panel(state: &Entity<AppState>, s: &AppState) -> impl IntoElement {
    let theme_preference = s.theme_preference;
    let resolved_theme = s.resolved_theme();
    let system_appearance = appearance_label(s.window_appearance);
    let summary_copy = match theme_preference {
        ThemePreference::System => format!(
            "{APP_NAME} follows the operating system by default. The current system appearance is {system_appearance}."
        ),
        ThemePreference::Light => {
            "Manual override is active. Switch back to System to follow the operating system again."
                .to_string()
        }
        ThemePreference::Dark => {
            "Manual override is active. Switch back to System to follow the operating system again."
                .to_string()
        }
    };

    panel().child(
        div()
            .p(px(28.0))
            .px(px(32.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(eyebrow("Settings / Appearance"))
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_emphasis())
                    .child("Theme"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(fg_muted())
                    .max_w(px(760.0))
                    .child(summary_copy),
            )
            .child(div().flex().gap(px(4.0)).flex_wrap().children(
                ThemePreference::all().iter().map(|candidate| {
                    let candidate = *candidate;
                    let state = state.clone();
                    surface_tab(
                        candidate.label(),
                        theme_preference == candidate,
                        move |_, window, cx| {
                            update_theme_preference(&state, candidate, window, cx);
                        },
                    )
                }),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_wrap()
                    .child(badge(&format!(
                        "active {}",
                        resolved_theme.label().to_lowercase()
                    )))
                    .child(badge(&format!(
                        "system {}",
                        system_appearance.to_lowercase()
                    ))),
            )
            .child(render_code_font_size_control(
                state,
                s.code_font_size_preference,
            )),
    )
}

fn render_code_font_size_control(
    state: &Entity<AppState>,
    code_font_size: CodeFontSizePreference,
) -> impl IntoElement {
    let can_decrease = code_font_size.size_px() > CODE_FONT_SIZE_MIN;
    let can_increase = code_font_size.size_px() < CODE_FONT_SIZE_MAX;
    let can_reset = code_font_size != CodeFontSizePreference::default_size();

    div()
        .pt(px(6.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child("Code font size"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(font_size_icon_button(LucideIcon::Minus, can_decrease, {
                    let state = state.clone();
                    move |_, window, cx| {
                        decrease_code_font_size_preference(&state, window, cx);
                    }
                }))
                .child(
                    div()
                        .min_w(px(68.0))
                        .h(px(30.0))
                        .px(px(12.0))
                        .rounded(radius_sm())
                        .bg(bg_inset())
                        .border_1()
                        .border_color(border_muted())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(code_font_size.label()),
                )
                .child(font_size_icon_button(LucideIcon::Plus, can_increase, {
                    let state = state.clone();
                    move |_, window, cx| {
                        increase_code_font_size_preference(&state, window, cx);
                    }
                }))
                .child(font_size_icon_button(LucideIcon::RotateCcw, can_reset, {
                    let state = state.clone();
                    move |_, window, cx| {
                        reset_code_font_size_preference(&state, window, cx);
                    }
                })),
        )
}

fn font_size_icon_button(
    icon: LucideIcon,
    enabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let button = div()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(radius_sm())
        .bg(control_button_bg())
        .border_1()
        .border_color(transparent())
        .flex()
        .items_center()
        .justify_center()
        .child(lucide_icon(icon, 14.0, fg_muted()));

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

fn render_review_intelligence_settings_panel(
    state: &Entity<AppState>,
    s: &AppState,
) -> impl IntoElement {
    let settings_state = s.review_ai_settings.clone();
    let configured_provider = settings_state.settings.provider;
    let provider_statuses = s.review_ai_provider_statuses.clone();
    let provider_status = s.selected_review_ai_provider_status().cloned();
    let provider_loading = s.review_ai_provider_loading;
    let provider_error = s.review_ai_provider_error.clone();
    let repository_names = workspace_repository_names(s);

    panel().child(
        div()
            .p(px(28.0))
            .px(px(32.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(eyebrow("Settings / Review Intelligence"))
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_emphasis())
                    .child("Review Partner and briefs"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(fg_muted())
                    .max_w(px(760.0))
                    .child(
                        "Choose the provider Remiss uses for Review Partner, Review Brief, and Guided Review. Automatic generation prewarms reviewer context per repository and only regenerates when the pull request code version changes.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .items_center()
                    .flex_wrap()
                    .child(ghost_button(
                        if settings_state.loading {
                            "Loading settings..."
                        } else {
                            "Reload settings"
                        },
                        {
                            let state = state.clone();
                            move |_, window, cx| {
                                trigger_review_ai_settings_refresh(&state, window, cx);
                            }
                        },
                    ))
                    .child(ghost_button(
                        if provider_loading {
                            "Refreshing providers..."
                        } else {
                            "Refresh providers"
                        },
                        {
                            let state = state.clone();
                            move |_, window, cx| {
                                trigger_review_ai_provider_status_refresh(&state, window, cx);
                            }
                        },
                    ))
                    .when(settings_state.loading, |el| {
                        el.child(panel_state_text(
                            "Loading saved review intelligence settings...",
                        ))
                    })
                    .when(provider_loading, |el| {
                        el.child(panel_state_text("Checking available providers..."))
                    })
                    .when(settings_state.background_syncing, |el| {
                        el.child(panel_state_text(
                            "Refreshing automatic review intelligence...",
                        ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("AI provider"),
                    )
                    .child(
                        div().flex().gap(px(4.0)).flex_wrap().children(
                            ReviewAiProvider::all().iter().map(|candidate| {
                                let candidate = *candidate;
                                let label = provider_tab_label(candidate, &provider_statuses);
                                let state = state.clone();
                                surface_tab(
                                    &label,
                                    configured_provider == candidate,
                                    move |_, window, cx| {
                                        update_review_ai_settings(
                                            &state,
                                            window,
                                            cx,
                                            move |settings| {
                                                settings.provider = candidate;
                                            },
                                        );
                                    },
                                )
                            }),
                        ),
                    )
                    .when_some(provider_status, |el, status| {
                        let primary = if status.available && status.authenticated {
                            success_text(&status.message).into_any_element()
                        } else {
                            error_text(&status.message).into_any_element()
                        };

                        el.child(primary).child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_subtle())
                                .child(status.detail),
                        )
                    }),
            )
            .when_some(settings_state.error.clone(), |el, error| {
                el.child(error_text(&error))
            })
            .when_some(provider_error, |el, error| el.child(error_text(&error)))
            .when_some(settings_state.background_error.clone(), |el, error| {
                el.child(error_text(&error))
            })
            .when_some(settings_state.background_message.clone(), |el, message| {
                el.child(panel_state_text(&message))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Automatic background generation"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .max_w(px(760.0))
                            .child(
                                "Repositories stay disabled by default. When you enable one, Remiss refreshes the managed checkout for matching pull requests and caches review intelligence in the background.",
                            ),
                    )
                    .when(repository_names.is_empty(), |el| {
                        el.child(panel_state_text(
                            "Workspace repositories will appear here after pull requests load. Previously enabled repositories stay listed so you can disable them later.",
                        ))
                    })
                    .children(repository_names.into_iter().map(|repository| {
                        let enabled = settings_state
                            .settings
                            .automatically_generates_for(&repository);
                        render_review_intelligence_repository_row(state, &repository, enabled)
                    })),
            ),
    )
}

fn render_managed_lsp_card(
    state: &Entity<AppState>,
    settings: &ManagedLspSettingsState,
    kind: ManagedServerKind,
) -> impl IntoElement {
    let status =
        settings
            .statuses
            .get(&kind)
            .cloned()
            .unwrap_or_else(|| ManagedServerInstallStatus {
                state: ManagedServerInstallState::NotInstalled,
                version: None,
                detail: "Status has not been checked yet.".to_string(),
            });
    let installing = settings.installing.contains(&kind);
    let install_error = settings.install_errors.get(&kind).cloned();
    let install_message = settings.install_messages.get(&kind).cloned();

    panel().child(
        div()
            .p(px(24.0))
            .px(px(28.0))
            .flex()
            .justify_between()
            .gap(px(24.0))
            .items_start()
            .child(
                div()
                    .flex_grow()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .flex_wrap()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child(kind.language_label()),
                            )
                            .child(managed_server_state_badge(status.state))
                            .when_some(status.version.clone(), |el, version| {
                                el.child(badge(&format!("v{version}")))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_family(mono_font_family())
                            .text_color(fg_subtle())
                            .child(managed_lsp::managed_server_display_name(kind)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(fg_muted())
                            .child(status.detail),
                    )
                    .when_some(kind.runtime_note(), |el, note| {
                        el.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_subtle())
                                .child(note),
                        )
                    })
                    .when_some(install_message, |el, message| {
                        el.child(success_text(&message))
                    })
                    .when_some(install_error, |el, error| el.child(error_text(&error))),
            )
            .child(
                ghost_button(install_button_label(status.state, installing), {
                    let state = state.clone();
                    move |_, window, cx| {
                        trigger_managed_lsp_install(&state, kind, window, cx);
                    }
                })
                .into_any_element(),
            ),
    )
}

fn install_button_label(state: ManagedServerInstallState, installing: bool) -> &'static str {
    if installing {
        return "Downloading...";
    }

    match state {
        ManagedServerInstallState::NotInstalled => "Download",
        ManagedServerInstallState::Installed => "Download again",
        ManagedServerInstallState::Broken => "Repair",
    }
}

fn managed_server_state_badge(state: ManagedServerInstallState) -> impl IntoElement {
    let (label, background, foreground) = match state {
        ManagedServerInstallState::NotInstalled => ("Not installed", bg_subtle(), fg_muted()),
        ManagedServerInstallState::Installed => ("Installed", success_muted(), success()),
        ManagedServerInstallState::Broken => ("Broken", danger_muted(), danger()),
    };

    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(background)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground)
        .child(label)
}

fn provider_tab_label(provider: ReviewAiProvider, statuses: &[ReviewAiProviderStatus]) -> String {
    match statuses.iter().find(|status| status.provider == provider) {
        Some(status) if status.available && status.authenticated => {
            format!("{} • ready", provider.label())
        }
        Some(status) if status.available => format!("{} • needs auth", provider.label()),
        Some(_) => format!("{} • unavailable", provider.label()),
        None => provider.label().to_string(),
    }
}

fn workspace_repository_names(s: &AppState) -> Vec<String> {
    let mut repositories = BTreeSet::new();
    if let Some(workspace) = s.workspace.as_ref() {
        for queue in &workspace.queues {
            for item in &queue.items {
                repositories.insert(item.repository.clone());
            }
        }
    }

    repositories.extend(
        s.review_ai_settings
            .settings
            .automatic_repositories
            .iter()
            .cloned(),
    );
    repositories.into_iter().collect()
}

fn render_review_intelligence_repository_row(
    state: &Entity<AppState>,
    repository: &str,
    enabled: bool,
) -> impl IntoElement {
    let repository_name = repository.to_string();
    let secondary_copy = if enabled {
        "Automatic background review intelligence is enabled."
    } else {
        "Automatic background review intelligence is disabled."
    };

    div()
        .p(px(16.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(bg_surface())
        .flex()
        .justify_between()
        .items_start()
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .min_w_0()
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(repository.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(secondary_copy),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_wrap()
                .child(badge(if enabled {
                    "background on"
                } else {
                    "background off"
                }))
                .child(ghost_button(
                    if enabled {
                        "Disable background generation"
                    } else {
                        "Enable background generation"
                    },
                    {
                        let state = state.clone();
                        move |_, window, cx| {
                            let repository = repository_name.clone();
                            update_review_ai_settings(&state, window, cx, move |settings| {
                                settings.set_automatic_generation_for(&repository, !enabled);
                            });
                        }
                    },
                )),
        )
}
