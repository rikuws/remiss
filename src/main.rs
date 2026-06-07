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
mod env_flags;
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
    register_bundled_fonts(cx)?;

    if let Some(config) = screenshot_config.as_ref() {
        config.clear_ready_file()?;
    }

    let (app_state, initial_window_size) =
        build_startup_state(cx, screenshot_config.as_ref(), startup_wizard_options)?;
    install_app_lifecycle(cx, &app_state);

    let initial_window_appearance = cx.window_appearance();
    app_state.update(cx, |state, _| {
        state.set_window_appearance(initial_window_appearance);
    });

    let root_window = open_root_window(
        cx,
        &app_state,
        screenshot_config.clone(),
        initial_window_size,
    )?;
    install_deep_link_handler(deep_link_dispatcher, &app_state, root_window, cx);
    start_platform_services(screenshot_config.as_ref());
    install_global_keystroke_observer(cx, &app_state);
    Ok(())
}

fn register_bundled_fonts(cx: &mut App) -> Result<(), String> {
    let bundled_fonts =
        load_bundled_fonts().map_err(|error| format!("Failed to load bundled fonts: {error}"))?;
    cx.text_system()
        .add_fonts(bundled_fonts)
        .map_err(|error| format!("Failed to register bundled fonts: {error}"))
}

fn build_startup_state(
    cx: &mut App,
    screenshot_config: Option<&screenshot_mode::ScreenshotConfig>,
    startup_wizard_options: onboarding::StartupWizardOptions,
) -> Result<(Entity<AppState>, Size<Pixels>), String> {
    let cache_path = screenshot_config
        .map(|config| config.cache_path.clone())
        .unwrap_or_else(cache_path);
    let cache = CacheStore::new(cache_path)
        .map_err(|error| format!("Failed to initialize cache: {error}"))?;
    let initial_window_size = screenshot_config
        .map(|config| config.window_size())
        .unwrap_or_else(|| window_settings::load_window_size(&cache));
    let screenshot_config_for_state = screenshot_config.cloned();
    let app_state = cx.new(move |_| {
        let mut state = AppState::new(cache, startup_wizard_options);
        if let Some(config) = screenshot_config_for_state.as_ref() {
            screenshot_mode::stage_initial_state(&mut state, config);
        }
        state
    });

    Ok((app_state, initial_window_size))
}

fn install_app_lifecycle(cx: &mut App, app_state: &Entity<AppState>) {
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
    install_diff_vim_key_interceptor(cx, app_state);
}

fn install_diff_vim_key_interceptor(cx: &mut App, app_state: &Entity<AppState>) {
    let app_state_for_diff_vim = app_state.clone();
    cx.intercept_keystrokes(move |event, window, cx| {
        handle_diff_vim_key_event(&app_state_for_diff_vim, event, window, cx);
    })
    .detach();
}

fn handle_diff_vim_key_event(
    app_state: &Entity<AppState>,
    event: &KeystrokeEvent,
    window: &mut Window,
    cx: &mut App,
) {
    if app_state.read(cx).temp_source_window.window == cx.active_window() {
        return;
    }

    let Some(key) = vim_key_from_keystroke(&event.keystroke) else {
        return;
    };
    if views::diff_view::trigger_diff_vim_key(app_state, key, window, cx) {
        cx.stop_propagation();
    }
}

fn open_root_window(
    cx: &mut App,
    app_state: &Entity<AppState>,
    screenshot_config: Option<screenshot_mode::ScreenshotConfig>,
    initial_window_size: Size<Pixels>,
) -> Result<WindowHandle<RootView>, String> {
    let bounds = Bounds::centered(None, initial_window_size, cx);
    let app_state_for_window = app_state.clone();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(main_window_titlebar_options()),
            ..Default::default()
        },
        move |window, cx| {
            let app_state = app_state_for_window.clone();
            let screenshot_config = screenshot_config.clone();
            cx.new(move |cx| RootView::new(app_state, screenshot_config, window, cx))
        },
    )
    .map_err(|error| format!("Failed to open app window: {error:?}"))
}

fn install_deep_link_handler(
    deep_link_dispatcher: deep_link::DeepLinkDispatcher,
    app_state: &Entity<AppState>,
    root_window: WindowHandle<RootView>,
    cx: &mut App,
) {
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
}

fn start_platform_services(screenshot_config: Option<&screenshot_mode::ScreenshotConfig>) {
    if screenshot_config.is_some() {
        return;
    }

    if let Err(error) = platform_updates::start_updater() {
        report_sentry_error(format!("{APP_NAME} updater disabled: {error}"));
    }
    if let Err(error) = platform_macos::prepare_system_notifications() {
        report_sentry_error(format!("{APP_NAME} notifications disabled: {error}"));
    }
}

fn install_global_keystroke_observer(cx: &mut App, app_state: &Entity<AppState>) {
    let app_state_for_keys = app_state.clone();
    cx.observe_keystrokes(move |event, window, cx| {
        handle_global_keystroke_event(&app_state_for_keys, event, window, cx);
    })
    .detach();
}

fn handle_global_keystroke_event(
    app_state: &Entity<AppState>,
    event: &KeystrokeEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let keystroke = &event.keystroke;
    let is_secondary_plain = shortcuts::secondary_plain_modifier(keystroke.modifiers);
    let is_secondary_shift = shortcuts::secondary_shift_modifier(keystroke.modifiers);

    if handle_onboarding_wizard_key(app_state, keystroke, cx)
        || handle_filter_dialog_key(app_state, keystroke, cx)
        || handle_global_surface_shortcut_key(app_state, keystroke, is_secondary_plain, window, cx)
        || handle_palette_key(app_state, keystroke, window, cx)
        || handle_file_chooser_key(app_state, keystroke, window, cx)
        || handle_review_surface_shortcut_key(
            app_state,
            keystroke,
            is_secondary_plain,
            is_secondary_shift,
            window,
            cx,
        )
        || handle_waypoint_spotlight_key(app_state, keystroke, window, cx)
        || handle_temp_source_window_key(app_state, keystroke, cx)
        || handle_finish_review_key(app_state, keystroke, is_secondary_plain, window, cx)
        || handle_line_action_key(app_state, keystroke, is_secondary_plain, window, cx)
    {
        return;
    }

    let review_editor_active = app_state.read(cx).review_editor_active;
    if handle_commit_timeline_navigation_key(app_state, keystroke, review_editor_active, window, cx)
        || handle_global_pull_request_paste_key(
            app_state,
            keystroke,
            is_secondary_plain,
            window,
            cx,
        )
    {
        return;
    }

    handle_review_editor_key(
        app_state,
        keystroke,
        is_secondary_plain,
        review_editor_active,
        window,
        cx,
    );
}

fn handle_onboarding_wizard_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    cx: &mut App,
) -> bool {
    if app_state.read(cx).active_onboarding_wizard.is_none() {
        return false;
    }

    match keystroke.key.as_str() {
        "escape" => app_state.update(cx, |state, cx| {
            state.complete_active_onboarding_wizard();
            cx.notify();
        }),
        "left" => app_state.update(cx, |state, cx| {
            state.previous_onboarding_step();
            cx.notify();
        }),
        "right" | "enter" => app_state.update(cx, |state, cx| {
            state.next_onboarding_step();
            cx.notify();
        }),
        _ => {}
    }
    true
}

fn handle_filter_dialog_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    cx: &mut App,
) -> bool {
    if app_state
        .read(cx)
        .pull_request_filter_dialog_scope
        .is_none()
    {
        return false;
    }

    if keystroke.key == "escape" {
        app_state.update(cx, |state, cx| {
            state.close_pull_request_filter_dialog();
            cx.notify();
        });
    }
    true
}

fn handle_global_surface_shortcut_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if is_secondary_plain && keystroke.key == "k" {
        toggle_palette(app_state, cx);
        return true;
    }
    if is_secondary_plain && keystroke.key == "p" {
        toggle_file_chooser(app_state, window, cx);
        return true;
    }
    false
}

fn handle_review_surface_shortcut_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    is_secondary_shift: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if handle_code_appearance_shortcut_key(
        app_state,
        keystroke,
        is_secondary_plain,
        is_secondary_shift,
        window,
        cx,
    ) {
        return true;
    }
    handle_waypoint_or_source_shortcut_key(
        app_state,
        keystroke,
        is_secondary_plain,
        is_secondary_shift,
        window,
        cx,
    )
}

fn handle_palette_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !app_state.read(cx).palette_open {
        return false;
    }

    match keystroke.key.as_str() {
        "escape" => close_palette(app_state, cx),
        "up" => move_palette_selection(app_state, -1, cx),
        "down" => move_palette_selection(app_state, 1, cx),
        "enter" => execute_palette_selection(app_state, window, cx),
        _ => {}
    }
    true
}

fn handle_file_chooser_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !app_state.read(cx).file_chooser_open {
        return false;
    }

    match keystroke.key.as_str() {
        "escape" => close_file_chooser(app_state, cx),
        "up" => move_file_chooser_selection(app_state, -1, cx),
        "down" => move_file_chooser_selection(app_state, 1, cx),
        "enter" => execute_file_chooser_selection(app_state, window, cx),
        _ => {}
    }
    true
}

fn handle_code_appearance_shortcut_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    is_secondary_shift: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if (is_secondary_plain || is_secondary_shift) && matches!(keystroke.key.as_str(), "=" | "+") {
        increase_code_font_size_preference(app_state, window, cx);
        return true;
    }
    if (is_secondary_plain || is_secondary_shift) && matches!(keystroke.key.as_str(), "-" | "_") {
        decrease_code_font_size_preference(app_state, window, cx);
        return true;
    }
    if is_secondary_plain && keystroke.key == "0" {
        reset_code_font_size_preference(app_state, window, cx);
        return true;
    }
    if is_secondary_shift && keystroke.key == "t" {
        cycle_diff_color_theme_preference(app_state, window, cx);
        return true;
    }
    false
}

fn handle_waypoint_or_source_shortcut_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    is_secondary_shift: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if is_secondary_shift && keystroke.key == "j" {
        trigger_add_waypoint_shortcut(app_state, cx);
        return true;
    }
    if is_secondary_plain && keystroke.key == "j" {
        toggle_waypoint_spotlight(app_state, cx);
        return true;
    }
    is_secondary_plain
        && keystroke.key == "o"
        && open_temp_source_window_for_selected_diff_line(app_state, window, cx)
}

fn handle_waypoint_spotlight_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !app_state.read(cx).waypoint_spotlight_open {
        return false;
    }

    match keystroke.key.as_str() {
        "escape" => close_waypoint_spotlight(app_state, cx),
        "up" => move_waypoint_spotlight_selection(app_state, -1, cx),
        "down" => move_waypoint_spotlight_selection(app_state, 1, cx),
        "enter" => execute_waypoint_spotlight_selection(app_state, window, cx),
        _ => {}
    }
    true
}

fn handle_temp_source_window_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    cx: &mut App,
) -> bool {
    keystroke.key == "escape" && close_temp_source_window_if_active(app_state, cx)
}

fn handle_finish_review_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !app_state.read(cx).review_finish_modal_open {
        return false;
    }

    if is_secondary_plain && keystroke.key == "enter" {
        trigger_submit_review_from_review_mode(app_state, window, cx);
        return true;
    }
    if keystroke.key == "escape" {
        close_review_finish_modal(app_state, cx);
    }
    true
}

fn handle_line_action_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if app_state.read(cx).active_review_line_action.is_none() {
        return false;
    }

    let line_comment_mode =
        app_state.read(cx).review_line_action_mode == state::ReviewLineActionMode::Comment;
    if is_secondary_plain && keystroke.key == "enter" && line_comment_mode {
        trigger_submit_inline_comment(app_state, window, cx);
        return true;
    }
    if keystroke.key == "escape" {
        close_review_line_action(app_state, cx);
        return true;
    }
    false
}

fn handle_commit_timeline_navigation_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    review_editor_active: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let navigation_enabled = {
        let state = app_state.read(cx);
        commit_timeline_navigation_enabled(state, review_editor_active)
    };
    if !navigation_enabled {
        return false;
    }

    match keystroke.key.as_str() {
        "left" => {
            app_state.update(cx, |state, cx| {
                state.move_active_commit_filter(-1);
                cx.notify();
            });
            prefetch_active_commit_diffs(app_state, window, cx);
            true
        }
        "right" => {
            app_state.update(cx, |state, cx| {
                state.move_active_commit_filter(1);
                cx.notify();
            });
            prefetch_active_commit_diffs(app_state, window, cx);
            true
        }
        "home" => {
            app_state.update(cx, |state, cx| {
                state.reset_active_commit_filter();
                cx.notify();
            });
            true
        }
        _ => false,
    }
}

fn commit_timeline_navigation_enabled(state: &AppState, review_editor_active: bool) -> bool {
    state.active_surface == state::PullRequestSurface::Files
        && state.effective_review_center_mode() == review_session::ReviewCenterMode::SemanticDiff
        && state
            .active_detail()
            .map(|detail| {
                !detail.commits.is_empty() && !crate::local_review::is_local_review_detail(detail)
            })
            .unwrap_or(false)
        && !review_editor_active
}

fn prefetch_active_commit_diffs(app_state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let model = app_state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            views::diff_view::prefetch_active_commit_diffs_flow(model, cx).await;
        })
        .detach();
}

fn handle_global_pull_request_paste_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    is_secondary_plain
        && keystroke.key == "v"
        && try_open_pasted_pull_request_url(app_state, window, cx)
}

fn handle_review_editor_key(
    app_state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_secondary_plain: bool,
    review_editor_active: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if !review_editor_active {
        return;
    }
    if is_secondary_plain && keystroke.key == "enter" {
        trigger_submit_review(app_state, window, cx);
        return;
    }
    if keystroke.key == "escape" {
        blur_review_editor(app_state, cx);
    }
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
