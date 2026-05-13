pub(crate) mod ai_tour;
pub(crate) mod diff_view;
mod palette;
mod pr_detail;
mod root;
mod sections;
mod settings;
mod workspace_sync;

pub use diff_view::{
    close_review_finish_modal, close_review_line_action, close_waypoint_spotlight,
    execute_waypoint_spotlight_selection, move_waypoint_spotlight_selection,
    toggle_waypoint_spotlight, trigger_add_waypoint_shortcut, trigger_submit_inline_comment,
    trigger_submit_review_from_review_mode,
};
pub use palette::{
    close_palette, execute_palette_selection, move_palette_selection, toggle_palette,
};
pub use pr_detail::{blur_review_editor, trigger_submit_review};
pub(crate) use root::{RootView, APP_CHROME_HEIGHT};
pub use settings::{
    cycle_diff_color_theme_preference, decrease_code_font_size_preference,
    increase_code_font_size_preference, reset_code_font_size_preference,
};
