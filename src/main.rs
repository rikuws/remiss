#![allow(dead_code)]
#![allow(
    clippy::collapsible_else_if,
    clippy::collapsible_if,
    clippy::cloned_ref_to_slice_refs,
    clippy::cmp_owned,
    clippy::derivable_impls,
    clippy::double_ended_iterator_last,
    clippy::filter_map_bool_then,
    clippy::items_after_test_module,
    clippy::large_enum_variant,
    clippy::len_zero,
    clippy::manual_div_ceil,
    clippy::manual_is_multiple_of,
    clippy::map_identity,
    clippy::needless_borrow,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_lifetimes,
    clippy::needless_match,
    clippy::needless_range_loop,
    clippy::needless_update,
    clippy::obfuscated_if_else,
    clippy::ptr_arg,
    clippy::question_mark,
    clippy::redundant_closure,
    clippy::result_large_err,
    clippy::single_match,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_lazy_evaluations,
    clippy::useless_vec
)]

mod actors;
mod agents;
mod app_assets;
mod app_http;
mod app_menu;
mod app_storage;
mod branding;
mod cache;
mod cli_binary;
mod code_display;
mod code_symbols;
mod command_runner;
mod commit_timeline;
mod deep_link;
mod demo_data;
mod diagnostic_logs;
mod diff;
mod difftastic;
mod emoji;
mod gh;
mod github;
mod github_files_diff;
mod icons;
mod inline_diff;
mod local_documents;
mod local_feedback;
mod local_repo;
mod local_review;
mod lsp;
mod managed_lsp;
mod markdown;
mod notifications;
mod onboarding;
mod platform_macos;
mod platform_updates;
#[cfg(target_os = "windows")]
mod platform_windows;
mod process_group;
mod review_ai;
mod review_anchors;
mod review_brief;
mod review_context;
mod review_file_header;
mod review_file_tree;
mod review_filters;
mod review_intelligence;
mod review_intelligence_background;
mod review_memory;
mod review_partner;
mod review_queue;
mod review_routes;
mod review_session;
mod screenshot_mode;
mod selectable_text;
mod semantic_diff;
mod semantic_review;
mod sentry_diagnostics;
mod shader_surface;
mod shortcuts;
mod source_browser;
mod stacks;
mod state;
mod structural_diff;
mod structural_diff_cache;
mod structural_evidence;
mod syntax;
mod temp_source_window;
#[cfg(test)]
mod test_git;
mod theme;
mod triage;
mod tutorial_pr;
mod views;
mod vim;
mod window_settings;

use std::sync::Arc;

use gpui::*;

use app_assets::{load_bundled_fonts, AppAssets};
use app_http::UreqHttpClient;
use app_storage::cache_path;
use branding::APP_NAME;
use cache::CacheStore;
use state::AppState;
use temp_source_window::{
    close_temp_source_window_if_active, install_temp_source_window_key_bindings,
    open_temp_source_window_for_selected_diff_line,
};
use views::{
    blur_review_editor, close_file_chooser, close_palette, close_review_finish_modal,
    close_review_line_action, close_waypoint_spotlight, cycle_diff_color_theme_preference,
    decrease_code_font_size_preference, execute_file_chooser_selection, execute_palette_selection,
    execute_waypoint_spotlight_selection, increase_code_font_size_preference,
    move_file_chooser_selection, move_palette_selection, move_waypoint_spotlight_selection,
    reset_code_font_size_preference, toggle_file_chooser, toggle_palette,
    toggle_waypoint_spotlight, trigger_add_waypoint_shortcut, trigger_software_update_check,
    trigger_submit_inline_comment, trigger_submit_review, trigger_submit_review_from_review_mode,
    RootView,
};
use vim::input::vim_key_from_keystroke;

fn main() {
    platform_updates::prepare_startup();
    cli_binary::repair_process_path_for_cli_tools();
    let _sentry_guard = sentry_diagnostics::init();

    let deep_link_dispatcher = deep_link::DeepLinkDispatcher::new();
    if let Err(error) =
        platform_macos::install_deep_link_url_event_handler(deep_link_dispatcher.clone())
    {
        report_sentry_error(format!("{APP_NAME} URL event handler disabled: {error}"));
    }
    deep_link_dispatcher.receive_urls(deep_link::remiss_urls_from_args(std::env::args().skip(1)));

    let deep_link_dispatcher_for_urls = deep_link_dispatcher.clone();
    let application = Application::new()
        .with_assets(AppAssets::new())
        .with_http_client(Arc::new(UreqHttpClient::new()));
    application.on_open_urls(move |urls| {
        deep_link_dispatcher_for_urls.receive_urls(urls);
    });
    application.run(move |cx: &mut App| {
        if let Err(error) = start_app(cx, deep_link_dispatcher.clone()) {
            report_sentry_error(format!("{APP_NAME} failed to start: {error}"));
        }
    });
}

fn report_sentry_error(message: String) {
    sentry_diagnostics::capture_error(&message);
    eprintln!("{message}");
}

fn start_app(
    cx: &mut App,
    deep_link_dispatcher: deep_link::DeepLinkDispatcher,
) -> Result<(), String> {
    let screenshot_config = screenshot_mode::ScreenshotConfig::from_env()?;
    let startup_wizard_options = onboarding::StartupWizardOptions::from_env_and_args();
    let bundled_fonts =
        load_bundled_fonts().map_err(|error| format!("Failed to load bundled fonts: {error}"))?;
    cx.text_system()
        .add_fonts(bundled_fonts)
        .map_err(|error| format!("Failed to register bundled fonts: {error}"))?;

    if let Some(config) = screenshot_config.as_ref() {
        config.clear_ready_file()?;
    }

    let cache_path = screenshot_config
        .as_ref()
        .map(|config| config.cache_path.clone())
        .unwrap_or_else(cache_path);
    let cache = CacheStore::new(cache_path)
        .map_err(|error| format!("Failed to initialize cache: {error}"))?;
    let initial_window_size = screenshot_config
        .as_ref()
        .map(|config| config.window_size())
        .unwrap_or_else(|| window_settings::load_window_size(&cache));
    let screenshot_config_for_state = screenshot_config.clone();
    let app_state = cx.new(move |_| {
        let mut state = AppState::new(cache, startup_wizard_options);
        if let Some(config) = screenshot_config_for_state.as_ref() {
            screenshot_mode::stage_initial_state(&mut state, config);
        }
        state
    });
    let lsp_session_manager_for_quit = app_state.read(cx).lsp_session_manager.clone();
    cx.on_app_quit(move |_| {
        let lsp_session_manager = lsp_session_manager_for_quit.clone();
        async move {
            lsp_session_manager.shutdown_all();
        }
    })
    .detach();
    app_menu::install(cx);
    let app_state_for_updates = app_state.clone();
    cx.on_action(move |_: &app_menu::CheckForUpdates, cx| {
        trigger_software_update_check(&app_state_for_updates, cx);
    });
    install_temp_source_window_key_bindings(cx);
    let app_state_for_diff_vim = app_state.clone();
    cx.intercept_keystrokes(move |event, window, cx| {
        if app_state_for_diff_vim.read(cx).temp_source_window.window == cx.active_window() {
            return;
        }

        let Some(key) = vim_key_from_keystroke(&event.keystroke) else {
            return;
        };
        if views::diff_view::trigger_diff_vim_key(&app_state_for_diff_vim, key, window, cx) {
            cx.stop_propagation();
        }
    })
    .detach();
    let initial_window_appearance = cx.window_appearance();
    app_state.update(cx, |state, _| {
        state.set_window_appearance(initial_window_appearance);
    });

    let bounds = Bounds::centered(None, initial_window_size, cx);
    let app_state_for_window = app_state.clone();
    let screenshot_config_for_window = screenshot_config.clone();
    let root_window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(main_window_titlebar_options()),
                ..Default::default()
            },
            move |window, cx| {
                let app_state = app_state_for_window.clone();
                let screenshot_config = screenshot_config_for_window.clone();
                cx.new(move |cx| RootView::new(app_state, screenshot_config, window, cx))
            },
        )
        .map_err(|error| format!("Failed to open app window: {error:?}"))?;
    let async_app = cx.to_async();
    deep_link_dispatcher.install_handler({
        let app_state = app_state.clone();
        move |request| {
            let app_state = app_state.clone();
            let root_window = root_window;
            let mut async_app = async_app.clone();
            let result = root_window.update(&mut async_app, move |_, window, cx| {
                cx.activate(true);
                window.activate_window();
                views::open_deep_link_request(&app_state, request, window, cx);
            });
            if let Err(error) = result {
                report_sentry_error(format!("Failed to route Remiss URL: {error}"));
            }
        }
    });

    if screenshot_config.is_none() {
        if let Err(error) = platform_updates::start_updater() {
            report_sentry_error(format!("{APP_NAME} updater disabled: {error}"));
        }
        if let Err(error) = platform_macos::prepare_system_notifications() {
            report_sentry_error(format!("{APP_NAME} notifications disabled: {error}"));
        }
    }

    let app_state_for_keys = app_state.clone();
    cx.observe_keystrokes(move |event, window, cx| {
        let keystroke = &event.keystroke;
        let is_secondary_plain = shortcuts::secondary_plain_modifier(keystroke.modifiers);
        let is_secondary_shift = shortcuts::secondary_shift_modifier(keystroke.modifiers);

        let onboarding_wizard_open = app_state_for_keys
            .read(cx)
            .active_onboarding_wizard
            .is_some();
        if onboarding_wizard_open {
            match keystroke.key.as_str() {
                "escape" => {
                    app_state_for_keys.update(cx, |state, cx| {
                        state.complete_active_onboarding_wizard();
                        cx.notify();
                    });
                }
                "left" => {
                    app_state_for_keys.update(cx, |state, cx| {
                        state.previous_onboarding_step();
                        cx.notify();
                    });
                }
                "right" | "enter" => {
                    app_state_for_keys.update(cx, |state, cx| {
                        state.next_onboarding_step();
                        cx.notify();
                    });
                }
                _ => {}
            }
            return;
        }

        let filter_dialog_open = app_state_for_keys
            .read(cx)
            .pull_request_filter_dialog_scope
            .is_some();
        if filter_dialog_open {
            if keystroke.key == "escape" {
                app_state_for_keys.update(cx, |state, cx| {
                    state.close_pull_request_filter_dialog();
                    cx.notify();
                });
            }
            return;
        }

        if is_secondary_plain && keystroke.key == "k" {
            toggle_palette(&app_state_for_keys, cx);
            return;
        }

        if is_secondary_plain && keystroke.key == "p" {
            toggle_file_chooser(&app_state_for_keys, window, cx);
            return;
        }

        let palette_open = app_state_for_keys.read(cx).palette_open;
        if palette_open {
            match keystroke.key.as_str() {
                "escape" => close_palette(&app_state_for_keys, cx),
                "up" => move_palette_selection(&app_state_for_keys, -1, cx),
                "down" => move_palette_selection(&app_state_for_keys, 1, cx),
                "enter" => execute_palette_selection(&app_state_for_keys, window, cx),
                _ => {}
            }
            return;
        }

        let file_chooser_open = app_state_for_keys.read(cx).file_chooser_open;
        if file_chooser_open {
            match keystroke.key.as_str() {
                "escape" => close_file_chooser(&app_state_for_keys, cx),
                "up" => move_file_chooser_selection(&app_state_for_keys, -1, cx),
                "down" => move_file_chooser_selection(&app_state_for_keys, 1, cx),
                "enter" => execute_file_chooser_selection(&app_state_for_keys, window, cx),
                _ => {}
            }
            return;
        }

        if (is_secondary_plain || is_secondary_shift) && matches!(keystroke.key.as_str(), "=" | "+")
        {
            increase_code_font_size_preference(&app_state_for_keys, window, cx);
            return;
        }

        if (is_secondary_plain || is_secondary_shift) && matches!(keystroke.key.as_str(), "-" | "_")
        {
            decrease_code_font_size_preference(&app_state_for_keys, window, cx);
            return;
        }

        if is_secondary_plain && keystroke.key == "0" {
            reset_code_font_size_preference(&app_state_for_keys, window, cx);
            return;
        }

        if is_secondary_shift && keystroke.key == "t" {
            cycle_diff_color_theme_preference(&app_state_for_keys, window, cx);
            return;
        }

        if is_secondary_shift && keystroke.key == "j" {
            trigger_add_waypoint_shortcut(&app_state_for_keys, cx);
            return;
        }

        if is_secondary_plain && keystroke.key == "j" {
            toggle_waypoint_spotlight(&app_state_for_keys, cx);
            return;
        }

        if is_secondary_plain
            && keystroke.key == "o"
            && open_temp_source_window_for_selected_diff_line(&app_state_for_keys, window, cx)
        {
            return;
        }

        let waypoint_spotlight_open = app_state_for_keys.read(cx).waypoint_spotlight_open;
        if waypoint_spotlight_open {
            match keystroke.key.as_str() {
                "escape" => close_waypoint_spotlight(&app_state_for_keys, cx),
                "up" => move_waypoint_spotlight_selection(&app_state_for_keys, -1, cx),
                "down" => move_waypoint_spotlight_selection(&app_state_for_keys, 1, cx),
                "enter" => execute_waypoint_spotlight_selection(&app_state_for_keys, window, cx),
                _ => {}
            }
            return;
        }

        if keystroke.key == "escape" && close_temp_source_window_if_active(&app_state_for_keys, cx)
        {
            return;
        }

        let finish_review_open = app_state_for_keys.read(cx).review_finish_modal_open;
        if finish_review_open {
            if is_secondary_plain && keystroke.key == "enter" {
                trigger_submit_review_from_review_mode(&app_state_for_keys, window, cx);
                return;
            }

            if keystroke.key == "escape" {
                close_review_finish_modal(&app_state_for_keys, cx);
                return;
            }
            return;
        }

        let line_action_active = app_state_for_keys
            .read(cx)
            .active_review_line_action
            .is_some();
        let line_comment_mode = app_state_for_keys.read(cx).review_line_action_mode
            == state::ReviewLineActionMode::Comment;

        if line_action_active {
            if is_secondary_plain && keystroke.key == "enter" && line_comment_mode {
                trigger_submit_inline_comment(&app_state_for_keys, window, cx);
                return;
            }

            if keystroke.key == "escape" {
                close_review_line_action(&app_state_for_keys, cx);
                return;
            }
        }

        let review_editor_active = app_state_for_keys.read(cx).review_editor_active;
        let commit_timeline_navigation_enabled = {
            let state = app_state_for_keys.read(cx);
            state.active_surface == state::PullRequestSurface::Files
                && state.effective_review_center_mode()
                    == review_session::ReviewCenterMode::SemanticDiff
                && state
                    .active_detail()
                    .map(|detail| {
                        !detail.commits.is_empty()
                            && !crate::local_review::is_local_review_detail(detail)
                    })
                    .unwrap_or(false)
                && !review_editor_active
        };
        if commit_timeline_navigation_enabled {
            match keystroke.key.as_str() {
                "left" => {
                    app_state_for_keys.update(cx, |state, cx| {
                        state.move_active_commit_filter(-1);
                        cx.notify();
                    });
                    let model = app_state_for_keys.clone();
                    window
                        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
                            views::diff_view::prefetch_active_commit_diffs_flow(model, cx).await;
                        })
                        .detach();
                    return;
                }
                "right" => {
                    app_state_for_keys.update(cx, |state, cx| {
                        state.move_active_commit_filter(1);
                        cx.notify();
                    });
                    let model = app_state_for_keys.clone();
                    window
                        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
                            views::diff_view::prefetch_active_commit_diffs_flow(model, cx).await;
                        })
                        .detach();
                    return;
                }
                "home" => {
                    app_state_for_keys.update(cx, |state, cx| {
                        state.reset_active_commit_filter();
                        cx.notify();
                    });
                    return;
                }
                _ => {}
            }
        }

        if is_secondary_plain
            && keystroke.key == "v"
            && try_open_pasted_pull_request_url(&app_state_for_keys, window, cx)
        {
            return;
        }

        if !review_editor_active {
            return;
        }

        if is_secondary_plain && keystroke.key == "enter" {
            trigger_submit_review(&app_state_for_keys, window, cx);
            return;
        }

        match keystroke.key.as_str() {
            "escape" => blur_review_editor(&app_state_for_keys, cx),
            _ => {}
        }
    })
    .detach();
    Ok(())
}

fn try_open_pasted_pull_request_url(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if selectable_text::app_text_input_is_active() {
        return false;
    }

    if !allows_global_pull_request_paste(state.read(cx)) {
        return false;
    }

    let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
        return false;
    };

    let Ok(request) = deep_link::parse_github_pull_request_web_url(&text) else {
        return false;
    };

    views::open_deep_link_request(state, request, window, cx);
    cx.stop_propagation();
    true
}

fn allows_global_pull_request_paste(state: &AppState) -> bool {
    state.active_onboarding_wizard.is_none()
        && !state.palette_open
        && !state.file_chooser_open
        && state.pull_request_filter_dialog_scope.is_none()
        && !state.waypoint_spotlight_open
        && !state.review_finish_modal_open
        && state.active_review_line_action.is_none()
        && !state.review_editor_active
}

#[cfg(target_os = "macos")]
fn main_window_titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some(APP_NAME.into()),
        appears_transparent: true,
        traffic_light_position: Some(point(
            px(views::APP_TRAFFIC_LIGHT_LEFT),
            px(views::APP_TRAFFIC_LIGHT_TOP),
        )),
        ..Default::default()
    }
}

#[cfg(not(target_os = "macos"))]
fn main_window_titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some(APP_NAME.into()),
        appears_transparent: cfg!(target_os = "windows"),
        ..Default::default()
    }
}
