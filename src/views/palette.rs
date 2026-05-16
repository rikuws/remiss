use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

use gpui::prelude::*;
use gpui::*;

use crate::app_assets::APP_LOGO_ASSET;
use crate::branding::APP_NAME;
use crate::icons::{lucide_icon, LucideIcon};
use crate::review_session::ReviewCenterMode;
use crate::selectable_text::{AppTextFieldKind, AppTextInput};
use crate::shortcuts;
use crate::state::*;
use crate::theme::*;

use super::diff_view::{
    ensure_active_review_focus_loaded, enter_files_surface, enter_stack_review_mode,
    switch_review_code_mode,
};
use super::sections::{badge, open_pull_request, panel_state_text};
use super::settings::{
    decrease_code_font_size_preference, increase_code_font_size_preference, prepare_settings_view,
    reset_code_font_size_preference, save_diff_color_theme_preference,
    trigger_software_update_check, update_code_font_size_preference,
};
use super::workspace_sync::trigger_sync_workspace;

const PALETTE_ANIMATION_MS: u64 = 160;
const PALETTE_SCROLL_ANIMATION_MS: u64 = 120;
const PALETTE_SCROLL_ANIMATION_STEPS: u64 = 10;
const PALETTE_SCROLL_REPEAT_WINDOW_MS: u64 = 140;
const PALETTE_SCROLL_EDGE_COMFORT_ROWS: f32 = 1.35;
const CODE_THEME_COMMAND_LABEL: &str = "Change code theme";
const PALETTE_RESULTS_HEADER_ITEMS: usize = 1;

pub fn render_palette(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let palette_open = s.palette_open;
    let query = s.palette_query.clone();
    let filtered = filtered_command_items(&s);
    let selected_index = selected_command_index(&s, &filtered);
    let saved_code_theme = s
        .palette_code_theme_preview_original
        .unwrap_or(s.diff_color_theme_preference);
    let previewed_code_theme = s.palette_code_theme_preview;

    let state_for_backdrop = state.clone();

    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .justify_center()
        .pt(px(72.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(palette_backdrop())
                .on_mouse_move({
                    let state = state_for_backdrop.clone();
                    move |_, _, cx| {
                        revert_code_theme_preview(&state, cx);
                    }
                })
                .on_mouse_down(MouseButton::Left, {
                    let state = state_for_backdrop.clone();
                    move |_, _, cx| {
                        close_palette(&state, cx);
                    }
                }),
        )
        .child(
            div()
                .w(px(720.0))
                .max_h(px(640.0))
                .bg(bg_overlay())
                .rounded(radius_lg())
                .border_1()
                .border_color(border_default())
                .occlude()
                .shadow_sm()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .on_mouse_move({
                            let state = state_for_backdrop.clone();
                            move |_, _, cx| {
                                revert_code_theme_preview(&state, cx);
                            }
                        })
                        .px(px(24.0))
                        .py(px(20.0))
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .child(
                                            img(APP_LOGO_ASSET)
                                                .size(px(24.0))
                                                .object_fit(ObjectFit::Contain),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child(format!("{APP_NAME} command")),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .child(badge(&shortcuts::secondary_key_label("k")))
                                        .child(badge("esc")),
                                ),
                        )
                        .child(
                            div()
                                .px(px(16.0))
                                .py(px(14.0))
                                .rounded(radius())
                                .border_1()
                                .border_color(focus_border())
                                .bg(bg_surface())
                                .text_size(px(15.0))
                                .text_color(if query.is_empty() {
                                    fg_subtle()
                                } else {
                                    fg_emphasis()
                                })
                                .child(
                                    AppTextInput::new(
                                        "palette-query-input",
                                        state.clone(),
                                        AppTextFieldKind::PaletteQuery,
                                        "Type to filter commands, sections, or open pull requests",
                                    )
                                    .autofocus(palette_open),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(format!("{} matches", filtered.len())),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .text_size(px(11.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child("↑↓ move")
                                        .child("•")
                                        .child("enter open"),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .id("palette-scroll")
                        .overflow_y_scroll()
                        .track_scroll(&s.palette_scroll_handle)
                        .max_h(px(452.0))
                        .child(
                            div()
                                .px(px(24.0))
                                .py(px(10.0))
                                .text_size(px(11.0))
                                .text_color(fg_subtle())
                                .font_weight(FontWeight::MEDIUM)
                                .child("Commands"),
                        )
                        .when(filtered.is_empty(), |el| {
                            el.child(
                                div().px(px(20.0)).pb(px(18.0)).child(panel_state_text(
                                    "No commands matched the current query.",
                                )),
                            )
                        })
                        .children(filtered.into_iter().enumerate().map(|(ix, item)| {
                            let state = state_for_backdrop.clone();
                            palette_item(
                                item,
                                ix == selected_index,
                                saved_code_theme,
                                previewed_code_theme,
                                state,
                            )
                        })),
                ),
        )
        .with_animation(
            ("command-palette-overlay", usize::from(palette_open)),
            Animation::new(Duration::from_millis(PALETTE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = palette_reveal_progress(palette_open, delta);
                el.opacity(progress.clamp(0.0, 1.0))
                    .pt(lerp_px(82.0, 72.0, progress))
            },
        )
}

pub fn open_palette(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.palette_open = true;
        s.palette_closing = false;
        s.palette_close_generation = s.palette_close_generation.wrapping_add(1);
        s.palette_query.clear();
        s.palette_selected_index = 0;
        reset_palette_scroll(s);
        s.palette_code_theme_expanded = false;
        clear_code_theme_preview(s);
        cx.notify();
    });
}

pub fn toggle_palette(state: &Entity<AppState>, cx: &mut App) {
    let is_open = state.read(cx).palette_open;
    if is_open {
        close_palette(state, cx);
    } else {
        open_palette(state, cx);
    }
}

pub fn close_palette(state: &Entity<AppState>, cx: &mut App) {
    let mut schedule_close = false;
    let mut close_generation = 0;
    state.update(cx, |s, cx| {
        if !s.palette_open && !s.palette_closing {
            return;
        }
        revert_code_theme_preview_in_state(s);
        schedule_close = s.palette_open;
        s.palette_open = false;
        s.palette_closing = true;
        s.palette_close_generation = s.palette_close_generation.wrapping_add(1);
        close_generation = s.palette_close_generation;
        cx.notify();
    });
    if schedule_close {
        finish_palette_close_after_animation(state.clone(), close_generation, cx);
    }
}

pub fn move_palette_selection(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let mut scroll_animation = None;
    state.update(cx, |s, cx| {
        if !s.palette_open {
            return;
        }
        let filtered = filtered_command_items(s);
        let item_count = filtered.len();
        if item_count == 0 {
            s.palette_selected_index = 0;
            revert_code_theme_preview_in_state(s);
            cx.notify();
            return;
        }

        let current_index = selected_command_index(s, &filtered);
        let max_index = item_count.saturating_sub(1) as isize;
        let next = (current_index as isize + delta).clamp(0, max_index) as usize;
        if next != s.palette_selected_index {
            s.palette_selected_index = next;
        }
        scroll_animation = prepare_palette_selection_scroll(s, next);
        apply_selection_preview(s, filtered.get(next));
        cx.notify();
    });
    if let Some(animation) = scroll_animation {
        animate_palette_scroll(state.clone(), animation, cx);
    }
}

pub fn execute_palette_selection(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let item = {
        let selected = state.read(cx);
        if !selected.palette_open {
            return;
        }
        let filtered = filtered_command_items(&selected);
        let selected_index = selected_command_index(&selected, &filtered);
        filtered.get(selected_index).cloned()
    };
    let Some(item) = item else {
        return;
    };
    apply_command_action(item.action, state, window, cx);
}

fn palette_item(
    item: CommandItem,
    selected: bool,
    saved_code_theme: DiffColorThemePreference,
    previewed_code_theme: Option<DiffColorThemePreference>,
    state: Entity<AppState>,
) -> impl IntoElement {
    let label = item.label.clone();
    let animation_id = command_item_animation_id(&item);
    let row_role = item.role;
    let is_code_theme_option = matches!(row_role, CommandItemRole::CodeThemeOption(_));
    let is_previewed = match row_role {
        CommandItemRole::CodeThemeOption(theme) => previewed_code_theme == Some(theme),
        _ => false,
    };
    let left_padding = if is_code_theme_option {
        px(34.0)
    } else {
        px(16.0)
    };
    let row_bg = if selected {
        bg_emphasis()
    } else if is_previewed {
        bg_selected()
    } else {
        bg_overlay()
    };
    let hover_action = item.action.clone();
    let click_action = item.action.clone();

    div()
        .mx(px(8.0))
        .mb(px(7.0))
        .pl(left_padding)
        .pr(px(16.0))
        .py(px(12.0))
        .rounded(radius_sm())
        .text_size(px(13.0))
        .border_1()
        .border_color(transparent())
        .bg(row_bg)
        .text_color(if selected {
            fg_emphasis()
        } else {
            fg_default()
        })
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(if selected {
                    bg_emphasis()
                } else {
                    bg_selected()
                })
                .text_color(fg_emphasis())
        })
        .on_mouse_move({
            let state = state.clone();
            move |_, _, cx| {
                apply_hover_preview(&state, &hover_action, cx);
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            apply_command_action(click_action.clone(), &state, window, cx);
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(label),
                )
                .child(command_item_accessory(row_role, selected, saved_code_theme))
                .when(
                    selected && !matches!(row_role, CommandItemRole::CodeThemeParent { .. }),
                    |el| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .font_family(mono_font_family())
                                .text_color(fg_subtle())
                                .child("enter"),
                        )
                    },
                ),
        )
        .with_animation(
            ("palette-row-reveal", animation_id),
            Animation::new(Duration::from_millis(PALETTE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                if is_code_theme_option {
                    let progress = delta.clamp(0.0, 1.0);
                    el.opacity(progress).mt(lerp_px(-5.0, 0.0, progress))
                } else {
                    el
                }
            },
        )
}

fn command_item_animation_id(item: &CommandItem) -> u64 {
    let mut hasher = DefaultHasher::new();
    item.label.hash(&mut hasher);
    match item.role {
        CommandItemRole::Normal => 0_u8.hash(&mut hasher),
        CommandItemRole::CodeThemeParent { expanded } => {
            1_u8.hash(&mut hasher);
            expanded.hash(&mut hasher);
        }
        CommandItemRole::CodeThemeOption(theme) => {
            2_u8.hash(&mut hasher);
            (theme as u8).hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn command_item_accessory(
    role: CommandItemRole,
    selected: bool,
    saved_code_theme: DiffColorThemePreference,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .when_some(code_theme_check_icon(role, saved_code_theme), |el, icon| {
            el.child(icon)
        })
        .when_some(code_theme_parent_icon(role), |el, icon| el.child(icon))
        .when(
            selected && matches!(role, CommandItemRole::CodeThemeParent { .. }),
            |el| {
                el.child(
                    div()
                        .text_size(px(11.0))
                        .font_family(mono_font_family())
                        .text_color(fg_subtle())
                        .child("enter"),
                )
            },
        )
}

fn code_theme_check_icon(
    role: CommandItemRole,
    saved_code_theme: DiffColorThemePreference,
) -> Option<impl IntoElement> {
    match role {
        CommandItemRole::CodeThemeOption(theme) if theme == saved_code_theme => {
            Some(lucide_icon(LucideIcon::Check, 13.0, fg_emphasis()))
        }
        _ => None,
    }
}

fn code_theme_parent_icon(role: CommandItemRole) -> Option<impl IntoElement> {
    match role {
        CommandItemRole::CodeThemeParent { expanded } => Some(lucide_icon(
            if expanded {
                LucideIcon::ChevronDown
            } else {
                LucideIcon::ChevronRight
            },
            14.0,
            fg_subtle(),
        )),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum CommandItemRole {
    Normal,
    CodeThemeParent { expanded: bool },
    CodeThemeOption(DiffColorThemePreference),
}

#[derive(Clone)]
struct CommandItem {
    label: String,
    search_text: String,
    action: CommandAction,
    role: CommandItemRole,
}

impl CommandItem {
    fn normal(label: impl Into<String>, action: CommandAction) -> Self {
        Self::normal_with_keywords(label, action, &[])
    }

    fn normal_with_keywords(
        label: impl Into<String>,
        action: CommandAction,
        keywords: &[&str],
    ) -> Self {
        let label = label.into();
        Self {
            search_text: command_search_text(&label, keywords),
            label,
            action,
            role: CommandItemRole::Normal,
        }
    }

    fn code_theme_parent(expanded: bool) -> Self {
        Self {
            label: CODE_THEME_COMMAND_LABEL.to_string(),
            search_text: command_search_text(
                CODE_THEME_COMMAND_LABEL,
                &["color", "syntax", "highlight", "diff"],
            ),
            action: CommandAction::ToggleCodeThemeSubmenu,
            role: CommandItemRole::CodeThemeParent { expanded },
        }
    }

    fn code_theme_option(theme: DiffColorThemePreference) -> Self {
        let label = theme.label().to_string();
        Self {
            search_text: command_search_text(&label, &["color", "syntax", "highlight"]),
            label,
            action: CommandAction::CommitCodeTheme(theme),
            role: CommandItemRole::CodeThemeOption(theme),
        }
    }
}

#[derive(Clone)]
enum CommandAction {
    GoToSection(SectionId),
    OpenPullRequest(crate::github::PullRequestSummary),
    ShowPullRequestSurface(PullRequestSurface),
    EnterCodeReview,
    EnterCodeLens(ReviewCenterMode),
    JumpToNextReviewComment,
    EnterAiTour,
    EnterStack,
    SyncWorkspace,
    CheckForUpdates,
    IncreaseCodeFontSize,
    DecreaseCodeFontSize,
    ResetCodeFontSize,
    SetCodeFontSize(CodeFontSizePreference),
    ToggleCodeThemeSubmenu,
    CommitCodeTheme(DiffColorThemePreference),
}

fn build_command_items(state: &AppState) -> Vec<CommandItem> {
    let query = normalized_palette_query(state);
    let mut items = Vec::new();

    for section in SectionId::all()
        .iter()
        .filter(|section| **section != SectionId::Issues)
    {
        push_command(
            &mut items,
            format!("Go to {}", section.label()),
            CommandAction::GoToSection(*section),
        );
    }

    push_review_navigation_items(&mut items, state);

    push_command(&mut items, "Sync workspace", CommandAction::SyncWorkspace);
    items.push(CommandItem::normal_with_keywords(
        "Check for Updates",
        CommandAction::CheckForUpdates,
        &["software", "sparkle", "release", "upgrade"],
    ));

    push_command(
        &mut items,
        "Increase code font size",
        CommandAction::IncreaseCodeFontSize,
    );
    push_command(
        &mut items,
        "Decrease code font size",
        CommandAction::DecreaseCodeFontSize,
    );
    push_command(
        &mut items,
        "Reset code font size",
        CommandAction::ResetCodeFontSize,
    );
    for size in CodeFontSizePreference::all() {
        push_command(
            &mut items,
            format!("Code font size: {}", size.label()),
            CommandAction::SetCodeFontSize(*size),
        );
    }
    push_code_theme_items(&mut items, state, &query);

    for tab in &state.open_tabs {
        push_command(
            &mut items,
            format!("Open {} #{}", tab.repository, tab.number),
            CommandAction::OpenPullRequest(tab.clone()),
        );
    }

    if let Some(workspace) = &state.workspace {
        for queue in &workspace.queues {
            for item in queue.items.iter().take(5) {
                push_command(
                    &mut items,
                    format!("#{} {}", item.number, item.title),
                    CommandAction::OpenPullRequest(item.clone()),
                );
            }
        }
    }

    items
}

fn filtered_command_items(state: &AppState) -> Vec<CommandItem> {
    let items = build_command_items(state);
    let query = normalized_palette_query(state);
    let query_chars = fuzzy_query_chars(&query);
    if query_chars.is_empty() {
        return items;
    }

    ranked_command_items(items, &query_chars)
}

fn normalized_palette_query(state: &AppState) -> String {
    state.palette_query.trim().to_lowercase()
}

fn push_command(items: &mut Vec<CommandItem>, label: impl Into<String>, action: CommandAction) {
    items.push(CommandItem::normal(label, action));
}

fn push_review_navigation_items(items: &mut Vec<CommandItem>, state: &AppState) {
    if state.active_detail().is_none() {
        return;
    }

    if !state.active_is_local_review() {
        items.push(CommandItem::normal_with_keywords(
            "Show PR briefing",
            CommandAction::ShowPullRequestSurface(PullRequestSurface::Overview),
            &["overview", "summary", "pull request"],
        ));
    }

    items.push(CommandItem::normal_with_keywords(
        "Open review files",
        CommandAction::ShowPullRequestSurface(PullRequestSurface::Files),
        &["review", "files", "changed files", "code"],
    ));
    items.push(CommandItem::normal_with_keywords(
        "Switch to Code",
        CommandAction::EnterCodeReview,
        &["review", "files", "diff", "source", "structural"],
    ));
    items.push(CommandItem::normal_with_keywords(
        "Switch to Diff",
        CommandAction::EnterCodeLens(ReviewCenterMode::SemanticDiff),
        &["semantic", "normal", "unified", "code", "review"],
    ));
    items.push(CommandItem::normal_with_keywords(
        "Switch to Structural Diff",
        CommandAction::EnterCodeLens(ReviewCenterMode::StructuralDiff),
        &["struct", "difftastic", "syntax", "ast", "code", "review"],
    ));
    if state.next_review_comment_location().is_some() {
        items.push(CommandItem::normal_with_keywords(
            "Jump to next review comment",
            CommandAction::JumpToNextReviewComment,
            &["next", "comment", "thread", "review", "diff"],
        ));
    }
    items.push(CommandItem::normal_with_keywords(
        "Switch to Source",
        CommandAction::EnterCodeLens(ReviewCenterMode::SourceBrowser),
        &["source browser", "repository", "full tree", "files", "code"],
    ));
    items.push(CommandItem::normal_with_keywords(
        "Switch to Guided Review",
        CommandAction::EnterStack,
        &[
            "ai",
            "tour",
            "guide",
            "generated review",
            "ai stack",
            "virtual stack",
            "layers",
            "review plan",
        ],
    ));
}

fn push_code_theme_items(items: &mut Vec<CommandItem>, state: &AppState, query: &str) {
    let expanded = state.palette_code_theme_expanded || !query.is_empty();
    items.push(CommandItem::code_theme_parent(expanded));

    if expanded {
        items.extend(
            DiffColorThemePreference::all()
                .iter()
                .copied()
                .map(CommandItem::code_theme_option),
        );
    }
}

fn command_search_text(label: &str, keywords: &[&str]) -> String {
    let mut search_text = String::with_capacity(
        label.len()
            + keywords
                .iter()
                .map(|keyword| keyword.len() + 1)
                .sum::<usize>(),
    );
    push_lowercase(&mut search_text, label);
    for keyword in keywords {
        search_text.push(' ');
        push_lowercase(&mut search_text, keyword);
    }
    search_text
}

fn push_lowercase(output: &mut String, text: &str) {
    for ch in text.chars() {
        output.extend(ch.to_lowercase());
    }
}

fn fuzzy_query_chars(query: &str) -> Vec<char> {
    query
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn ranked_command_items(items: Vec<CommandItem>, query_chars: &[char]) -> Vec<CommandItem> {
    let mut ranked = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            fuzzy_match_score(&item.search_text, query_chars).map(|score| (score, index, item))
        })
        .collect::<Vec<_>>();

    ranked.sort_by(
        |(left_score, left_index, _), (right_score, right_index, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_index.cmp(right_index))
        },
    );

    ranked.into_iter().map(|(_, _, item)| item).collect()
}

fn fuzzy_match_score(search_text: &str, query_chars: &[char]) -> Option<i64> {
    if query_chars.is_empty() {
        return Some(0);
    }

    let mut score = 0_i64;
    let mut query_ix = 0;
    let mut first_match_ix = 0_usize;
    let mut last_match_ix = None;
    let mut at_word_start = true;

    for (char_ix, ch) in search_text.chars().enumerate() {
        if query_ix >= query_chars.len() {
            break;
        }

        if ch == query_chars[query_ix] {
            if query_ix == 0 {
                first_match_ix = char_ix;
            }

            score += 12;
            if at_word_start {
                score += 10;
            }

            if let Some(last_ix) = last_match_ix {
                let gap = char_ix.saturating_sub(last_ix + 1);
                if gap == 0 {
                    score += 16;
                } else {
                    score -= gap.min(12) as i64;
                }
            }

            last_match_ix = Some(char_ix);
            query_ix += 1;
        }

        at_word_start = is_palette_word_separator(ch);
    }

    if query_ix != query_chars.len() {
        return None;
    }

    let compact_query = query_chars.iter().collect::<String>();
    if search_text.contains(compact_query.as_str()) {
        score += 64 + (compact_query.chars().count().min(16) as i64 * 4);
    }
    if search_text
        .split(is_palette_word_separator)
        .any(|word| word.starts_with(compact_query.as_str()))
    {
        score += 32;
    }

    score -= first_match_ix.min(48) as i64;
    score -= search_text.chars().count().min(160) as i64 / 40;
    Some(score)
}

fn is_palette_word_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '/' | '-' | '_' | ':' | '#' | '.' | '(' | ')')
}

fn apply_command_action(
    action: CommandAction,
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    match action {
        CommandAction::GoToSection(section) => {
            if section == SectionId::Settings {
                prepare_settings_view(state, window, cx);
            }
            close_palette(state, cx);
            state.update(cx, |s, cx| {
                s.set_active_section(section);
                s.active_pr_key = None;
                cx.notify();
            });
        }
        CommandAction::OpenPullRequest(pr) => {
            close_palette(state, cx);
            open_pull_request(state, pr, window, cx);
        }
        CommandAction::ShowPullRequestSurface(surface) => {
            close_palette(state, cx);
            match surface {
                PullRequestSurface::Overview => {
                    state.update(cx, |s, cx| {
                        if s.active_detail().is_none() || s.active_is_local_review() {
                            return;
                        }
                        s.active_surface = PullRequestSurface::Overview;
                        s.pr_header_compact = false;
                        s.persist_active_review_session();
                        cx.notify();
                    });
                }
                PullRequestSurface::Files => {
                    enter_files_surface(state, window, cx);
                }
            }
        }
        CommandAction::EnterCodeReview => {
            close_palette(state, cx);
            state.update(cx, |s, cx| {
                if s.active_detail().is_none() {
                    return;
                }
                s.active_surface = PullRequestSurface::Files;
                s.pr_header_compact = false;
                s.enter_code_review_mode();
                s.persist_active_review_session();
                cx.notify();
            });
            ensure_active_review_focus_loaded(state, window, cx);
        }
        CommandAction::EnterCodeLens(mode) => {
            close_palette(state, cx);
            state.update(cx, |s, cx| {
                if s.active_detail().is_none() {
                    return;
                }
                s.active_surface = PullRequestSurface::Files;
                s.pr_header_compact = false;
                cx.notify();
            });
            switch_review_code_mode(state, mode, window, cx);
        }
        CommandAction::JumpToNextReviewComment => {
            let location = state.read(cx).next_review_comment_location();
            close_palette(state, cx);
            if let Some(location) = location {
                state.update(cx, |s, cx| {
                    s.active_surface = PullRequestSurface::Files;
                    s.pr_header_compact = false;
                    s.set_review_file_collapsed(&location.file_path, false);
                    s.navigate_to_review_location(location, true);
                    s.persist_active_review_session();
                    cx.notify();
                });
                ensure_active_review_focus_loaded(state, window, cx);
            }
        }
        CommandAction::EnterAiTour => {
            close_palette(state, cx);
            enter_stack_review_mode(state, window, cx);
        }
        CommandAction::EnterStack => {
            close_palette(state, cx);
            enter_stack_review_mode(state, window, cx);
        }
        CommandAction::SyncWorkspace => {
            trigger_sync_workspace(state, window, cx);
            close_palette(state, cx);
        }
        CommandAction::CheckForUpdates => {
            trigger_software_update_check(state, cx);
            close_palette(state, cx);
        }
        CommandAction::IncreaseCodeFontSize => {
            increase_code_font_size_preference(state, window, cx);
            close_palette(state, cx);
        }
        CommandAction::DecreaseCodeFontSize => {
            decrease_code_font_size_preference(state, window, cx);
            close_palette(state, cx);
        }
        CommandAction::ResetCodeFontSize => {
            reset_code_font_size_preference(state, window, cx);
            close_palette(state, cx);
        }
        CommandAction::SetCodeFontSize(size) => {
            update_code_font_size_preference(state, size, window, cx);
            close_palette(state, cx);
        }
        CommandAction::ToggleCodeThemeSubmenu => {
            toggle_code_theme_submenu(state, cx);
        }
        CommandAction::CommitCodeTheme(theme) => {
            state.update(cx, |s, _| {
                clear_code_theme_preview(s);
            });
            save_diff_color_theme_preference(state, theme, window, cx);
            close_palette(state, cx);
        }
    }
}

fn toggle_code_theme_submenu(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if s.palette_code_theme_expanded {
            s.palette_code_theme_expanded = false;
            revert_code_theme_preview_in_state(s);
        } else {
            s.palette_code_theme_expanded = true;
        }
        cx.notify();
    });
}

fn selected_command_index(state: &AppState, items: &[CommandItem]) -> usize {
    if items.is_empty() {
        return 0;
    }

    let clamped = state
        .palette_selected_index
        .min(items.len().saturating_sub(1));
    if state.palette_selected_index != 0 {
        return clamped;
    }

    let query = normalized_palette_query(state);
    let query_chars = fuzzy_query_chars(&query);
    if query_chars.is_empty()
        || fuzzy_match_score(
            &command_search_text(CODE_THEME_COMMAND_LABEL, &[]),
            &query_chars,
        )
        .is_some()
    {
        return clamped;
    }

    items
        .iter()
        .position(|item| matches!(item.role, CommandItemRole::CodeThemeOption(_)))
        .unwrap_or(clamped)
}

fn apply_selection_preview(state: &mut AppState, item: Option<&CommandItem>) {
    match item.map(|item| item.role) {
        Some(CommandItemRole::CodeThemeOption(theme)) => {
            preview_code_theme_in_state(state, theme);
        }
        _ => {
            revert_code_theme_preview_in_state(state);
        }
    }
}

fn apply_hover_preview(state: &Entity<AppState>, action: &CommandAction, cx: &mut App) {
    state.update(cx, |s, cx| {
        let changed = match action {
            CommandAction::CommitCodeTheme(theme) => preview_code_theme_in_state(s, *theme),
            _ => revert_code_theme_preview_in_state(s),
        };
        if changed {
            cx.notify();
        }
    });
}

fn preview_code_theme_in_state(state: &mut AppState, theme: DiffColorThemePreference) -> bool {
    let mut changed = false;
    if state.palette_code_theme_preview_original.is_none() {
        state.palette_code_theme_preview_original = Some(state.diff_color_theme_preference);
        changed = true;
    }
    if state.palette_code_theme_preview != Some(theme) {
        state.palette_code_theme_preview = Some(theme);
        changed = true;
    }
    if state.diff_color_theme_preference != theme {
        state.set_diff_color_theme_preference(theme);
        changed = true;
    }
    changed
}

fn revert_code_theme_preview(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if revert_code_theme_preview_in_state(s) {
            cx.notify();
        }
    });
}

fn revert_code_theme_preview_in_state(state: &mut AppState) -> bool {
    let Some(original) = state.palette_code_theme_preview_original else {
        state.palette_code_theme_preview = None;
        return false;
    };
    state.set_diff_color_theme_preference(original);
    clear_code_theme_preview(state);
    true
}

fn clear_code_theme_preview(state: &mut AppState) {
    state.palette_code_theme_preview_original = None;
    state.palette_code_theme_preview = None;
}

fn finish_palette_close_after_animation(
    state: Entity<AppState>,
    close_generation: u64,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(PALETTE_ANIMATION_MS))
            .await;
        state
            .update(cx, |s, cx| {
                if !s.palette_open && s.palette_close_generation == close_generation {
                    s.palette_closing = false;
                    s.palette_query.clear();
                    s.palette_selected_index = 0;
                    reset_palette_scroll(s);
                    s.palette_code_theme_expanded = false;
                    clear_code_theme_preview(s);
                    cx.notify();
                }
            })
            .ok();
    })
    .detach();
}

fn palette_reveal_progress(open: bool, delta: f32) -> f32 {
    if open {
        delta
    } else {
        1.0 - delta
    }
}

fn lerp_px(from: f32, to: f32, progress: f32) -> Pixels {
    px(from + (to - from) * progress)
}

struct PaletteScrollAnimation {
    generation: u64,
    start: Point<Pixels>,
    target: Point<Pixels>,
}

fn reset_palette_scroll(state: &mut AppState) {
    state.palette_scroll_animation_generation =
        state.palette_scroll_animation_generation.wrapping_add(1);
    state.palette_scroll_animation_active = false;
    state.palette_last_scroll_navigation_at = None;
    state
        .palette_scroll_handle
        .set_offset(point(px(0.0), px(0.0)));
}

fn prepare_palette_selection_scroll(
    state: &mut AppState,
    selected_index: usize,
) -> Option<PaletteScrollAnimation> {
    let item_ix = PALETTE_RESULTS_HEADER_ITEMS + selected_index;
    let scroll_handle = &state.palette_scroll_handle;
    let current_offset = scroll_handle.offset();
    let now = Instant::now();
    let rapid_repeat = state
        .palette_last_scroll_navigation_at
        .map(|last| {
            now.duration_since(last) <= Duration::from_millis(PALETTE_SCROLL_REPEAT_WINDOW_MS)
        })
        .unwrap_or(false);
    state.palette_last_scroll_navigation_at = Some(now);

    let Some(target_y) = palette_scroll_target_y(scroll_handle, item_ix) else {
        state.palette_scroll_animation_generation =
            state.palette_scroll_animation_generation.wrapping_add(1);
        state.palette_scroll_animation_active = false;
        scroll_handle.scroll_to_item(item_ix);
        return None;
    };

    let scroll_delta = (f32::from(target_y) - f32::from(current_offset.y)).abs();
    if scroll_delta < 0.5 {
        if rapid_repeat && state.palette_scroll_animation_active {
            state.palette_scroll_animation_generation =
                state.palette_scroll_animation_generation.wrapping_add(1);
            state.palette_scroll_animation_active = false;
        }
        return None;
    }

    state.palette_scroll_animation_generation =
        state.palette_scroll_animation_generation.wrapping_add(1);
    if rapid_repeat || state.palette_scroll_animation_active {
        state.palette_scroll_animation_active = false;
        scroll_handle.set_offset(point(current_offset.x, target_y));
        return None;
    }

    state.palette_scroll_animation_active = true;
    Some(PaletteScrollAnimation {
        generation: state.palette_scroll_animation_generation,
        start: current_offset,
        target: point(current_offset.x, target_y),
    })
}

fn palette_scroll_target_y(scroll_handle: &ScrollHandle, item_ix: usize) -> Option<Pixels> {
    let item_bounds = scroll_handle.bounds_for_item(item_ix)?;
    let viewport_bounds = scroll_handle.bounds();
    let current_offset = scroll_handle.offset();
    let edge_comfort = palette_scroll_edge_comfort(item_bounds, viewport_bounds);
    let top_edge = viewport_bounds.top() + edge_comfort;
    let bottom_edge = viewport_bounds.bottom() - edge_comfort;
    let mut target_y = current_offset.y;

    if item_bounds.top() + current_offset.y < top_edge {
        target_y = top_edge - item_bounds.top();
    } else if item_bounds.bottom() + current_offset.y > bottom_edge {
        target_y = bottom_edge - item_bounds.bottom();
    }

    let max_offset = f32::from(scroll_handle.max_offset().height);
    Some(px(f32::from(target_y).clamp(-max_offset, 0.0)))
}

fn palette_scroll_edge_comfort(
    item_bounds: Bounds<Pixels>,
    viewport_bounds: Bounds<Pixels>,
) -> Pixels {
    let item_height = f32::from(item_bounds.size.height);
    let viewport_height = f32::from(viewport_bounds.size.height);
    px((item_height * PALETTE_SCROLL_EDGE_COMFORT_ROWS).min(viewport_height * 0.24))
}

fn animate_palette_scroll(
    state: Entity<AppState>,
    animation: PaletteScrollAnimation,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let frame_ms = PALETTE_SCROLL_ANIMATION_MS / PALETTE_SCROLL_ANIMATION_STEPS;
        for step in 1..=PALETTE_SCROLL_ANIMATION_STEPS {
            cx.background_executor()
                .timer(Duration::from_millis(frame_ms))
                .await;

            let progress = step as f32 / PALETTE_SCROLL_ANIMATION_STEPS as f32;
            let eased = ease_out_sine(progress);
            let next_y = lerp_f32(
                f32::from(animation.start.y),
                f32::from(animation.target.y),
                eased,
            );
            state
                .update(cx, |s, cx| {
                    if !s.palette_open
                        || s.palette_scroll_animation_generation != animation.generation
                    {
                        return;
                    }
                    s.palette_scroll_handle
                        .set_offset(point(animation.target.x, px(next_y)));
                    if step == PALETTE_SCROLL_ANIMATION_STEPS {
                        s.palette_scroll_animation_active = false;
                    }
                    cx.notify();
                })
                .ok();
        }
    })
    .detach();
}

fn lerp_f32(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

fn ease_out_sine(progress: f32) -> f32 {
    (progress.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin()
}

#[cfg(test)]
mod tests {
    use super::{
        fuzzy_match_score, fuzzy_query_chars, ranked_command_items, CommandAction, CommandItem,
    };

    #[test]
    fn fuzzy_match_accepts_abbreviated_navigation_queries() {
        assert!(fuzzy_match_score(
            "switch to structural diff struct difftastic syntax",
            &fuzzy_query_chars("str")
        )
        .is_some());
        assert!(fuzzy_match_score("switch to ai tour guide", &fuzzy_query_chars("ai")).is_some());
        assert!(
            fuzzy_match_score("switch to source source browser", &fuzzy_query_chars("src"))
                .is_some()
        );
    }

    #[test]
    fn ranked_command_items_prefers_tighter_fuzzy_matches() {
        let items = vec![
            CommandItem::normal_with_keywords(
                "Switch to Source",
                CommandAction::SyncWorkspace,
                &["source browser"],
            ),
            CommandItem::normal_with_keywords(
                "Switch to Structural Diff",
                CommandAction::SyncWorkspace,
                &["struct difftastic"],
            ),
            CommandItem::normal_with_keywords(
                "Switch to Guided Review",
                CommandAction::SyncWorkspace,
                &["ai guide"],
            ),
        ];

        let ranked = ranked_command_items(items, &fuzzy_query_chars("str"));

        assert_eq!(ranked[0].label, "Switch to Structural Diff");
    }

    #[test]
    fn fuzzy_match_uses_command_keywords() {
        let items = vec![CommandItem::normal_with_keywords(
            "Switch to Guided Review",
            CommandAction::SyncWorkspace,
            &["ai stack virtual layers"],
        )];

        let ranked = ranked_command_items(items, &fuzzy_query_chars("ai"));

        assert_eq!(ranked[0].label, "Switch to Guided Review");
    }
}
