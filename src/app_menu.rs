use gpui::{actions, App, KeyBinding, Menu, MenuItem, SystemMenuType};

use crate::{branding::APP_NAME, platform_macos};

actions!(
    remiss,
    [
        ShowAbout,
        ToggleCommandPalette,
        ShowSettings,
        CheckForUpdates,
        SyncWorkspace,
        AddLocalRepository,
        RefreshLocalRepositories,
        ShowPullRequestBriefing,
        OpenReviewFiles,
        SwitchToCode,
        SwitchToDiff,
        SwitchToStructuralDiff,
        SwitchToSource,
        SwitchToAiTour,
        SwitchToStack,
        JumpToNextReviewComment,
        IncreaseCodeFontSize,
        DecreaseCodeFontSize,
        ResetCodeFontSize,
        CycleCodeTheme,
        ToggleWaypointSpotlight,
        AddWaypoint,
        OpenSelectedLineInSource,
        SubmitReview,
        Quit
    ]
);

pub fn install(cx: &mut App) {
    bind_menu_key_equivalents(cx);
    cx.on_action(show_about);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![
        app_menu(),
        workspace_menu(),
        review_menu(),
        navigate_menu(),
        view_menu(),
    ]);
}

fn bind_menu_key_equivalents(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", ToggleCommandPalette, None),
        KeyBinding::new("cmd-,", ShowSettings, None),
        KeyBinding::new("cmd-shift-u", CheckForUpdates, None),
        KeyBinding::new("cmd-r", SyncWorkspace, None),
        KeyBinding::new("cmd-shift-o", AddLocalRepository, None),
        KeyBinding::new("cmd-enter", SubmitReview, None),
        KeyBinding::new("cmd-o", OpenSelectedLineInSource, None),
        KeyBinding::new("cmd-j", ToggleWaypointSpotlight, None),
        KeyBinding::new("cmd-shift-j", AddWaypoint, None),
        KeyBinding::new("cmd-=", IncreaseCodeFontSize, None),
        KeyBinding::new("cmd--", DecreaseCodeFontSize, None),
        KeyBinding::new("cmd-0", ResetCodeFontSize, None),
        KeyBinding::new("cmd-shift-t", CycleCodeTheme, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

fn app_menu() -> Menu {
    Menu {
        name: APP_NAME.into(),
        items: vec![
            MenuItem::action(format!("About {APP_NAME}"), ShowAbout),
            MenuItem::separator(),
            MenuItem::action("Settings...", ShowSettings),
            MenuItem::action("Check for Updates...", CheckForUpdates),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(format!("Quit {APP_NAME}"), Quit),
        ],
    }
}

fn show_about(_: &ShowAbout, _: &mut App) {
    if let Err(error) = platform_macos::show_about_panel() {
        eprintln!("{APP_NAME} about panel unavailable: {error}");
    }
}

fn workspace_menu() -> Menu {
    Menu {
        name: "Workspace".into(),
        items: vec![
            MenuItem::action("Command Palette", ToggleCommandPalette),
            MenuItem::separator(),
            MenuItem::action("Sync Workspace", SyncWorkspace),
            MenuItem::separator(),
            MenuItem::action("Add Local Repository...", AddLocalRepository),
            MenuItem::action("Refresh Local Repositories", RefreshLocalRepositories),
        ],
    }
}

fn review_menu() -> Menu {
    Menu {
        name: "Review".into(),
        items: vec![
            MenuItem::action("Show PR Briefing", ShowPullRequestBriefing),
            MenuItem::action("Open Review Files", OpenReviewFiles),
            MenuItem::separator(),
            MenuItem::action("Switch to Code", SwitchToCode),
            MenuItem::action("Switch to Diff", SwitchToDiff),
            MenuItem::action("Switch to Structural Diff", SwitchToStructuralDiff),
            MenuItem::action("Switch to Source", SwitchToSource),
            MenuItem::action("Switch to AI Tour", SwitchToAiTour),
            MenuItem::action("Switch to Stack", SwitchToStack),
            MenuItem::separator(),
            MenuItem::action("Submit Review", SubmitReview),
        ],
    }
}

fn navigate_menu() -> Menu {
    Menu {
        name: "Navigate".into(),
        items: vec![
            MenuItem::action("Find Waypoint", ToggleWaypointSpotlight),
            MenuItem::action("Add Waypoint", AddWaypoint),
            MenuItem::separator(),
            MenuItem::action("Jump to Next Review Comment", JumpToNextReviewComment),
            MenuItem::action("Open Selected Line in Source", OpenSelectedLineInSource),
        ],
    }
}

fn view_menu() -> Menu {
    Menu {
        name: "View".into(),
        items: vec![
            MenuItem::action("Increase Code Font Size", IncreaseCodeFontSize),
            MenuItem::action("Decrease Code Font Size", DecreaseCodeFontSize),
            MenuItem::action("Reset Code Font Size", ResetCodeFontSize),
            MenuItem::separator(),
            MenuItem::action("Cycle Code Theme", CycleCodeTheme),
        ],
    }
}
