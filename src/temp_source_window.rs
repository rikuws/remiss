use gpui::prelude::*;
use gpui::*;
use std::{fs, path::PathBuf, rc::Rc};

use crate::code_display::{
    build_prepared_file_lsp_context,
    render_virtualized_prepared_file_with_line_numbers_and_focus_flush,
    render_virtualized_prepared_file_with_line_numbers_diffs_and_focus_flush,
};
use crate::diff::{DiffLineKind, ParsedDiffFile, ParsedDiffLine};
use crate::github::PullRequestDetail;
use crate::review_ai::DiffAnchor;
use crate::source_browser::build_full_file_diff_lines;
use crate::state::{AppState, ReviewLineActionTarget, TempSourceSide, TempSourceTarget};
use crate::theme::*;
use crate::views::diff_view::load_temp_source_file_content_flow;

actions!(
    temp_source_window,
    [
        CloseTempSourceWindow,
        MoveTempSourceLineUp,
        MoveTempSourceLineDown,
        MoveTempSourceHalfPageUp,
        MoveTempSourceHalfPageDown,
        MoveTempSourcePageUp,
        MoveTempSourcePageDown,
        MoveTempSourceStart,
        MoveTempSourceEnd
    ]
);

const TEMP_SOURCE_WINDOW_WIDTH: f32 = 920.0;
const TEMP_SOURCE_WINDOW_HEIGHT: f32 = 720.0;
const TEMP_SOURCE_WINDOW_KEY_CONTEXT: &str = "temp_source_window";
const TEMP_SOURCE_CODE_MARGIN: f32 = 6.0;
const TEMP_SOURCE_CODE_LINE_HEIGHT: f32 = 21.0;

pub struct TempSourceWindow {
    state: Entity<AppState>,
    focus_handle: FocusHandle,
    list_state: ListState,
    last_scrolled_focus_key: Option<String>,
}

impl TempSourceWindow {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        cx.observe_window_bounds(window, {
            let state = state.clone();
            move |_, window, cx| {
                if window.is_fullscreen() || window.is_maximized() {
                    return;
                }

                let cache = state.read(cx).cache.clone();
                let _ = crate::window_settings::save_temp_source_window_bounds(
                    cache.as_ref(),
                    window.bounds(),
                );
            }
        })
        .detach();

        let state_for_release = state.clone();
        cx.on_release(move |_, cx| {
            state_for_release.update(cx, |state, cx| {
                state.temp_source_window.window = None;
                cx.notify();
            });
        })
        .detach();

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);

        Self {
            state,
            focus_handle,
            list_state: ListState::new(0, ListAlignment::Top, px(520.0)),
            last_scrolled_focus_key: None,
        }
    }

    fn move_line_up(
        &mut self,
        _: &MoveTempSourceLineUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focus_line(-1, window, cx);
    }

    fn move_line_down(
        &mut self,
        _: &MoveTempSourceLineDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focus_line(1, window, cx);
    }

    fn move_focus_line(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_line) = self.current_focus_line(cx) else {
            cx.stop_propagation();
            return;
        };

        let line_count = self.loaded_line_count(cx).unwrap_or(0);
        let next_line = temp_source_line_after_delta(current_line, delta, line_count);
        self.set_focus_line(next_line, window, cx);
    }

    fn move_half_page_up(
        &mut self,
        _: &MoveTempSourceHalfPageUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focus_line(-(temp_source_half_page_rows(window) as isize), window, cx);
    }

    fn move_half_page_down(
        &mut self,
        _: &MoveTempSourceHalfPageDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focus_line(temp_source_half_page_rows(window) as isize, window, cx);
    }

    fn move_page_up(
        &mut self,
        _: &MoveTempSourcePageUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focus_line(-(temp_source_page_rows(window) as isize), window, cx);
    }

    fn move_page_down(
        &mut self,
        _: &MoveTempSourcePageDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focus_line(temp_source_page_rows(window) as isize, window, cx);
    }

    fn move_to_start(
        &mut self,
        _: &MoveTempSourceStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_focus_line(1, window, cx);
    }

    fn move_to_end(&mut self, _: &MoveTempSourceEnd, window: &mut Window, cx: &mut Context<Self>) {
        let Some(line_count) = self.loaded_line_count(cx) else {
            cx.stop_propagation();
            return;
        };

        self.set_focus_line(line_count, window, cx);
    }

    fn current_focus_line(&self, cx: &App) -> Option<usize> {
        self.state
            .read(cx)
            .temp_source_window
            .target
            .as_ref()
            .map(|target| target.line)
    }

    fn loaded_line_count(&self, cx: &App) -> Option<usize> {
        self.state
            .read(cx)
            .temp_source_window
            .prepared
            .as_ref()
            .map(|prepared| prepared.lines.len())
    }

    fn set_focus_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let movement = self.state.update(cx, |state, cx| {
            let line_count = state
                .temp_source_window
                .prepared
                .as_ref()
                .map(|prepared| prepared.lines.len())?;
            let target = state.temp_source_window.target.as_mut()?;
            let next_line = temp_source_line_after_delta(line, 0, line_count);

            if next_line == target.line {
                return None;
            }

            target.line = next_line;
            cx.notify();
            Some((target.focus_key(), next_line.saturating_sub(1)))
        });

        if let Some((focus_key, item_ix)) = movement {
            self.last_scrolled_focus_key = Some(focus_key);
            self.list_state.scroll_to_reveal_item(item_ix);
            window.refresh();
        }

        cx.stop_propagation();
    }
}

impl Render for TempSourceWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = {
            let app_state = self.state.read(cx);
            app_state.temp_source_window.clone()
        };
        let state_for_close = self.state.clone();

        let target = snapshot.target.clone();

        if let Some(prepared) = snapshot.prepared.as_ref() {
            if self.list_state.item_count() != prepared.lines.len() {
                self.list_state.reset(prepared.lines.len());
            }

            if let Some(target) = target.as_ref() {
                let focus_key = target.focus_key();
                if self.last_scrolled_focus_key.as_deref() != Some(focus_key.as_str()) {
                    let visible_rows = centered_source_visible_rows(window);
                    self.list_state.scroll_to(ListOffset {
                        item_ix: centered_source_item_ix(target.line, visible_rows),
                        offset_in_item: px(0.0),
                    });
                    self.last_scrolled_focus_key = Some(focus_key);
                }
            }
        }

        div()
            .key_context(TEMP_SOURCE_WINDOW_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(move |_: &CloseTempSourceWindow, window, cx| {
                close_temp_source_window(&state_for_close, window, cx);
                cx.stop_propagation();
            })
            .on_action(cx.listener(Self::move_line_up))
            .on_action(cx.listener(Self::move_line_down))
            .on_action(cx.listener(Self::move_half_page_up))
            .on_action(cx.listener(Self::move_half_page_down))
            .on_action(cx.listener(Self::move_page_up))
            .on_action(cx.listener(Self::move_page_down))
            .on_action(cx.listener(Self::move_to_start))
            .on_action(cx.listener(Self::move_to_end))
            .size_full()
            .min_w(px(560.0))
            .min_h(px(360.0))
            .bg(bg_overlay())
            .text_color(fg_default())
            .flex()
            .flex_col()
            .child(match target.as_ref() {
                None => render_temp_source_state("No source target selected.").into_any_element(),
                Some(target) if snapshot.loading && snapshot.prepared.is_none() => {
                    render_temp_source_state(format!(
                        "Loading {} at {}...",
                        target.path, target.reference
                    ))
                    .into_any_element()
                }
                Some(target) if snapshot.error.is_some() && snapshot.prepared.is_none() => {
                    render_temp_source_error(
                        self.state.clone(),
                        target.clone(),
                        snapshot.error.as_deref().unwrap_or_default(),
                    )
                    .into_any_element()
                }
                Some(target) => snapshot
                    .prepared
                    .as_ref()
                    .map(|prepared| {
                        let lsp_context = (target.side == TempSourceSide::Head)
                            .then(|| {
                                build_prepared_file_lsp_context(
                                    &self.state,
                                    target.path.as_str(),
                                    Some(prepared),
                                    cx,
                                )
                            })
                            .flatten();
                        let parsed = self.state.read(cx).active_detail().and_then(|detail| {
                            crate::diff::find_parsed_diff_file(&detail.parsed_diff, &target.path)
                        });

                        div()
                            .flex()
                            .flex_col()
                            .flex_grow()
                            .min_h_0()
                            .min_w_0()
                            .p(px(TEMP_SOURCE_CODE_MARGIN))
                            .child(render_temp_source_code_surface(if target.side
                                == TempSourceSide::Head
                            {
                                if let Some(parsed) = parsed {
                                    render_virtualized_prepared_file_with_line_numbers_diffs_and_focus_flush(
                                        prepared,
                                        lsp_context.as_ref(),
                                        build_full_file_diff_lines(parsed),
                                        self.list_state.clone(),
                                        Some(target.line),
                                    )
                                    .into_any_element()
                                } else {
                                    render_virtualized_prepared_file_with_line_numbers_and_focus_flush(
                                        prepared,
                                        lsp_context.as_ref(),
                                        self.list_state.clone(),
                                        Some(target.line),
                                    )
                                    .into_any_element()
                                }
                            } else {
                                render_virtualized_prepared_file_with_line_numbers_and_focus_flush(
                                    prepared,
                                    lsp_context.as_ref(),
                                    self.list_state.clone(),
                                    Some(target.line),
                                )
                                .into_any_element()
                            }))
                            .into_any_element()
                    })
                    .unwrap_or_else(|| {
                        render_temp_source_state(format!(
                            "Loading {} at {}...",
                            target.path, target.reference
                        ))
                        .into_any_element()
                    }),
            })
    }
}

fn render_temp_source_code_surface(code: AnyElement) -> impl IntoElement {
    let radius = radius_lg();
    let mask_color = bg_overlay();

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .rounded(radius)
        .bg(bg_inset())
        .overflow_hidden()
        .child(code)
        .child(render_temp_source_code_corner_mask(radius, mask_color))
}

fn render_temp_source_code_corner_mask(radius: Pixels, mask_color: Rgba) -> impl IntoElement {
    canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            paint_temp_source_code_corner_mask(window, bounds, radius, mask_color);
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn paint_temp_source_code_corner_mask(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    radius: Pixels,
    color: Rgba,
) {
    let radius = f32::from(radius)
        .min(f32::from(bounds.size.width) / 2.0)
        .min(f32::from(bounds.size.height) / 2.0);
    if radius <= 0.0 {
        return;
    }

    let radius = px(radius);
    let control = px(f32::from(radius) * 0.552_284_8);
    let left = bounds.left();
    let right = bounds.right();
    let top = bounds.top();
    let bottom = bounds.bottom();

    let mut top_left = PathBuilder::fill();
    top_left.move_to(point(left, top));
    top_left.line_to(point(left + radius, top));
    top_left.cubic_bezier_to(
        point(left, top + radius),
        point(left + radius - control, top),
        point(left, top + radius - control),
    );
    top_left.line_to(point(left, top));
    top_left.close();
    paint_corner_mask_path(window, top_left, color);

    let mut top_right = PathBuilder::fill();
    top_right.move_to(point(right, top));
    top_right.line_to(point(right - radius, top));
    top_right.cubic_bezier_to(
        point(right, top + radius),
        point(right - radius + control, top),
        point(right, top + radius - control),
    );
    top_right.line_to(point(right, top));
    top_right.close();
    paint_corner_mask_path(window, top_right, color);

    let mut bottom_right = PathBuilder::fill();
    bottom_right.move_to(point(right, bottom));
    bottom_right.line_to(point(right, bottom - radius));
    bottom_right.cubic_bezier_to(
        point(right - radius, bottom),
        point(right, bottom - radius + control),
        point(right - radius + control, bottom),
    );
    bottom_right.line_to(point(right, bottom));
    bottom_right.close();
    paint_corner_mask_path(window, bottom_right, color);

    let mut bottom_left = PathBuilder::fill();
    bottom_left.move_to(point(left, bottom));
    bottom_left.line_to(point(left, bottom - radius));
    bottom_left.cubic_bezier_to(
        point(left + radius, bottom),
        point(left, bottom - radius + control),
        point(left + radius - control, bottom),
    );
    bottom_left.line_to(point(left, bottom));
    bottom_left.close();
    paint_corner_mask_path(window, bottom_left, color);
}

fn paint_corner_mask_path(window: &mut Window, builder: PathBuilder, color: Rgba) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

pub fn install_temp_source_window_key_bindings(cx: &mut App) {
    let mut bindings = vec![
        KeyBinding::new(
            "escape",
            CloseTempSourceWindow,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "up",
            MoveTempSourceLineUp,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "k",
            MoveTempSourceLineUp,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "down",
            MoveTempSourceLineDown,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "j",
            MoveTempSourceLineDown,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-u",
            MoveTempSourceHalfPageUp,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-d",
            MoveTempSourceHalfPageDown,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "pageup",
            MoveTempSourcePageUp,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-b",
            MoveTempSourcePageUp,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "pagedown",
            MoveTempSourcePageDown,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-f",
            MoveTempSourcePageDown,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "home",
            MoveTempSourceStart,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "g g",
            MoveTempSourceStart,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "end",
            MoveTempSourceEnd,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "shift-g",
            MoveTempSourceEnd,
            Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
        ),
    ];
    bindings.extend(load_temp_source_vim_config_key_bindings());
    cx.bind_keys(bindings);
}

pub fn open_temp_source_window_for_selected_diff_line(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let target = {
        let app_state = state.read(cx);
        app_state.active_detail().and_then(|detail| {
            temp_source_target_for_current_diff_selection(
                detail,
                app_state.active_review_line_action.as_ref(),
                app_state.hovered_temp_source_target.as_ref(),
                app_state.selected_diff_anchor.as_ref(),
            )
        })
    };

    if let Some(target) = target {
        open_temp_source_window_for_diff_target(state, target, window, cx);
        true
    } else {
        false
    }
}

pub fn open_temp_source_window_for_diff_target(
    state: &Entity<AppState>,
    target: TempSourceTarget,
    window: &mut Window,
    cx: &mut App,
) {
    let request_key = {
        let app_state = state.read(cx);
        let Some(detail) = app_state.active_detail() else {
            return;
        };
        temp_source_request_key(detail, &target)
    };

    state.update(cx, |state, cx| {
        let already_loaded = state.temp_source_window.request_key.as_deref()
            == Some(request_key.as_str())
            && state.temp_source_window.prepared.is_some()
            && state.temp_source_window.error.is_none();

        state.temp_source_window.target = Some(target.clone());
        state.temp_source_window.request_key = Some(request_key.clone());
        if !already_loaded {
            state.temp_source_window.document = None;
            state.temp_source_window.prepared = None;
            state.temp_source_window.loading = true;
            state.temp_source_window.error = None;
        }
        cx.notify();
    });

    let existing_window = state.read(cx).temp_source_window.window;
    let updated_title = temp_source_title(&target);
    let reused = existing_window
        .map(|handle| {
            handle
                .update(cx, |_, window, _| {
                    window.set_window_title(&updated_title);
                    window.activate_window();
                })
                .is_ok()
        })
        .unwrap_or(false);

    if !reused {
        state.update(cx, |state, _| {
            state.temp_source_window.window = None;
        });

        let title = temp_source_title(&target);
        let fallback_bounds = Bounds::centered(
            None,
            size(px(TEMP_SOURCE_WINDOW_WIDTH), px(TEMP_SOURCE_WINDOW_HEIGHT)),
            cx,
        );
        let bounds = {
            let cache = state.read(cx).cache.clone();
            crate::window_settings::load_temp_source_window_bounds(cache.as_ref(), fallback_bounds)
        };
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.activate(false);
                cx.new(|cx| TempSourceWindow::new(state.clone(), window, cx))
            },
        ) {
            Ok(handle) => {
                let any_handle = handle.into();
                state.update(cx, |state, cx| {
                    state.temp_source_window.window = Some(any_handle);
                    cx.notify();
                });
                let _ = handle.update(cx, |_, window, _| {
                    window.activate_window();
                });
            }
            Err(error) => {
                state.update(cx, |state, cx| {
                    state.temp_source_window.loading = false;
                    state.temp_source_window.error =
                        Some(format!("Failed to open source window: {error:?}"));
                    cx.notify();
                });
            }
        }
    }

    retry_temp_source_window_target(state, target, window, cx);
}

pub fn close_temp_source_window(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    window.remove_window();
    state.update(cx, |state, cx| {
        state.temp_source_window.window = None;
        cx.notify();
    });
    true
}

pub fn close_temp_source_window_if_active(state: &Entity<AppState>, cx: &mut App) -> bool {
    let handle = state.read(cx).temp_source_window.window;
    let active = cx.active_window();

    let Some(handle) = handle else {
        return false;
    };
    if active != Some(handle) {
        return false;
    }

    if handle
        .update(cx, |_, window, _| {
            window.remove_window();
        })
        .is_ok()
    {
        state.update(cx, |state, cx| {
            state.temp_source_window.window = None;
            cx.notify();
        });
        true
    } else {
        state.update(cx, |state, _| {
            state.temp_source_window.window = None;
        });
        false
    }
}

pub fn temp_source_target_for_diff_line(
    detail: &PullRequestDetail,
    parsed: &ParsedDiffFile,
    line: &ParsedDiffLine,
) -> Option<TempSourceTarget> {
    let base_reference = base_reference(detail)?;
    let head_reference = head_reference(detail)?;
    temp_source_target_for_diff_line_with_refs(parsed, line, &base_reference, &head_reference)
}

pub fn temp_source_target_for_diff_side(
    detail: &PullRequestDetail,
    parsed: &ParsedDiffFile,
    line: &ParsedDiffLine,
    side: TempSourceSide,
) -> Option<TempSourceTarget> {
    let base_reference = base_reference(detail)?;
    let head_reference = head_reference(detail)?;
    temp_source_target_for_diff_side_with_refs(parsed, line, side, &base_reference, &head_reference)
}

pub fn temp_source_target_for_anchor(
    detail: &PullRequestDetail,
    anchor: &DiffAnchor,
) -> Option<TempSourceTarget> {
    let parsed = crate::diff::find_parsed_diff_file(&detail.parsed_diff, &anchor.file_path)?;
    let side = anchor.side.as_deref()?;
    let target_side = match side {
        "LEFT" => TempSourceSide::Base,
        "RIGHT" => TempSourceSide::Head,
        _ => return None,
    };
    let line_number = anchor.line?;
    let line = parsed
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .find(|line| match side {
            "LEFT" => line.left_line_number == Some(line_number),
            "RIGHT" => line.right_line_number == Some(line_number),
            _ => false,
        })?;

    temp_source_target_for_diff_side(detail, parsed, line, target_side)
}

pub fn temp_source_request_key(detail: &PullRequestDetail, target: &TempSourceTarget) -> String {
    format!(
        "{}:{}:{}:temp-source",
        detail.updated_at,
        detail.repository,
        target.content_key()
    )
}

pub(crate) fn temp_source_target_for_current_diff_selection(
    detail: &PullRequestDetail,
    active_line_action: Option<&ReviewLineActionTarget>,
    hovered_source_target: Option<&TempSourceTarget>,
    selected_anchor: Option<&DiffAnchor>,
) -> Option<TempSourceTarget> {
    active_line_action
        .and_then(|target| temp_source_target_for_anchor(detail, &target.anchor))
        .or_else(|| hovered_source_target.cloned())
        .or_else(|| {
            selected_anchor.and_then(|anchor| temp_source_target_for_anchor(detail, anchor))
        })
}

fn centered_source_visible_rows(window: &Window) -> usize {
    let source_height = window.viewport_size().height - px(TEMP_SOURCE_CODE_MARGIN * 2.0);
    (source_height / px(TEMP_SOURCE_CODE_LINE_HEIGHT))
        .floor()
        .max(1.0) as usize
}

fn temp_source_half_page_rows(window: &Window) -> usize {
    (centered_source_visible_rows(window) / 2).max(1)
}

fn temp_source_page_rows(window: &Window) -> usize {
    centered_source_visible_rows(window)
        .saturating_sub(2)
        .max(1)
}

fn centered_source_item_ix(target_line: usize, visible_rows: usize) -> usize {
    target_line
        .saturating_sub(1)
        .saturating_sub(visible_rows / 2)
}

fn temp_source_line_after_delta(current_line: usize, delta: isize, line_count: usize) -> usize {
    if line_count == 0 {
        return 0;
    }

    let next = if delta < 0 {
        current_line.saturating_sub((-delta) as usize)
    } else {
        current_line.saturating_add(delta as usize)
    };

    next.clamp(1, line_count)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TempSourceMovement {
    LineUp,
    LineDown,
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    Start,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TempSourceVimRemap {
    keystrokes: String,
    movement: TempSourceMovement,
}

fn load_temp_source_vim_config_key_bindings() -> Vec<KeyBinding> {
    let mut leader = "\\".to_string();
    let mut remaps = Vec::new();

    for path in temp_source_vim_config_paths() {
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        let parsed = temp_source_vim_remaps_from_config(&contents, leader.as_str());
        leader = parsed.leader;
        remaps.extend(parsed.remaps);
    }

    remaps
        .into_iter()
        .filter_map(|remap| temp_source_key_binding_for_movement(&remap.keystrokes, remap.movement))
        .collect()
}

fn temp_source_vim_config_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    vec![
        home.join(".vimrc"),
        home.join(".config").join("nvim").join("init.vim"),
        home.join(".config").join("nvim").join("init.lua"),
    ]
}

fn temp_source_key_binding_for_movement(
    keystrokes: &str,
    movement: TempSourceMovement,
) -> Option<KeyBinding> {
    let context: Rc<KeyBindingContextPredicate> =
        KeyBindingContextPredicate::parse(TEMP_SOURCE_WINDOW_KEY_CONTEXT)
            .ok()?
            .into();
    let action: Box<dyn Action> = match movement {
        TempSourceMovement::LineUp => Box::new(MoveTempSourceLineUp),
        TempSourceMovement::LineDown => Box::new(MoveTempSourceLineDown),
        TempSourceMovement::HalfPageUp => Box::new(MoveTempSourceHalfPageUp),
        TempSourceMovement::HalfPageDown => Box::new(MoveTempSourceHalfPageDown),
        TempSourceMovement::PageUp => Box::new(MoveTempSourcePageUp),
        TempSourceMovement::PageDown => Box::new(MoveTempSourcePageDown),
        TempSourceMovement::Start => Box::new(MoveTempSourceStart),
        TempSourceMovement::End => Box::new(MoveTempSourceEnd),
    };

    KeyBinding::load(
        keystrokes,
        action,
        Some(context),
        false,
        None,
        &DummyKeyboardMapper,
    )
    .ok()
}

#[derive(Clone, Debug)]
struct ParsedTempSourceVimConfig {
    leader: String,
    remaps: Vec<TempSourceVimRemap>,
}

fn temp_source_vim_remaps_from_config(
    contents: &str,
    initial_leader: &str,
) -> ParsedTempSourceVimConfig {
    let mut leader = initial_leader.to_string();
    let mut remaps = Vec::new();

    for line in contents.lines() {
        if let Some(next_leader) = temp_source_vimscript_leader_assignment(line)
            .or_else(|| temp_source_lua_leader_assignment(line))
        {
            leader = next_leader;
            continue;
        }

        if let Some(remap) = temp_source_vimscript_remap(line, leader.as_str())
            .or_else(|| temp_source_lua_remap(line, leader.as_str()))
        {
            remaps.push(remap);
        }
    }

    ParsedTempSourceVimConfig { leader, remaps }
}

fn temp_source_vimscript_leader_assignment(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('"') || !trimmed.starts_with("let ") {
        return None;
    }

    let (_, value) = trimmed.split_once('=')?;
    let name = trimmed
        .strip_prefix("let ")?
        .split('=')
        .next()?
        .trim()
        .trim_start_matches("g:");
    if name != "mapleader" {
        return None;
    }

    temp_source_first_quoted_string(value)
}

fn temp_source_lua_leader_assignment(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("--") || !trimmed.contains("mapleader") {
        return None;
    }

    let (name, value) = trimmed.split_once('=')?;
    if !name.contains("vim.g.mapleader") {
        return None;
    }

    temp_source_first_quoted_string(value)
}

fn temp_source_vimscript_remap(line: &str, leader: &str) -> Option<TempSourceVimRemap> {
    let trimmed = line.trim_start().trim_start_matches(':');
    if trimmed.starts_with('"') {
        return None;
    }

    let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
    let command = *tokens.first()?;
    if !matches!(command, "nmap" | "nnoremap" | "noremap" | "map") {
        return None;
    }

    let mut index = 1;
    while tokens
        .get(index)
        .is_some_and(|token| temp_source_vim_map_option(token))
    {
        if tokens[index].eq_ignore_ascii_case("<expr>") {
            return None;
        }
        index += 1;
    }

    let lhs = *tokens.get(index)?;
    let rhs = *tokens.get(index + 1)?;
    temp_source_vim_remap_from_parts(lhs, rhs, leader)
}

fn temp_source_lua_remap(line: &str, leader: &str) -> Option<TempSourceVimRemap> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("--")
        || !(trimmed.contains("keymap.set")
            || trimmed.contains("nvim_set_keymap")
            || trimmed.starts_with("keymap("))
        || trimmed.contains("expr = true")
        || trimmed.contains("expr=true")
    {
        return None;
    }

    let quoted = temp_source_quoted_strings(trimmed);
    let mode = quoted.first()?;
    if !mode.contains('n') {
        return None;
    }

    temp_source_vim_remap_from_parts(quoted.get(1)?, quoted.get(2)?, leader)
}

fn temp_source_vim_remap_from_parts(
    lhs: &str,
    rhs: &str,
    leader: &str,
) -> Option<TempSourceVimRemap> {
    let movement = temp_source_movement_from_vim_rhs(rhs)?;
    let keystrokes = temp_source_vim_key_sequence_to_gpui(lhs, leader)?;

    Some(TempSourceVimRemap {
        keystrokes,
        movement,
    })
}

fn temp_source_movement_from_vim_rhs(rhs: &str) -> Option<TempSourceMovement> {
    let normalized = temp_source_vim_key_sequence_to_gpui(rhs, "\\")?;
    match normalized.as_str() {
        "up" | "k" => Some(TempSourceMovement::LineUp),
        "down" | "j" => Some(TempSourceMovement::LineDown),
        "ctrl-u" => Some(TempSourceMovement::HalfPageUp),
        "ctrl-d" => Some(TempSourceMovement::HalfPageDown),
        "pageup" | "ctrl-b" => Some(TempSourceMovement::PageUp),
        "pagedown" | "ctrl-f" => Some(TempSourceMovement::PageDown),
        "home" | "g g" => Some(TempSourceMovement::Start),
        "end" | "shift-g" => Some(TempSourceMovement::End),
        _ => None,
    }
}

fn temp_source_vim_key_sequence_to_gpui(sequence: &str, leader: &str) -> Option<String> {
    let mut keys = Vec::new();
    let characters = sequence.chars().collect::<Vec<_>>();
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];
        if character == '<' {
            let end = characters[index + 1..]
                .iter()
                .position(|candidate| *candidate == '>')?
                + index
                + 1;
            let token = characters[index + 1..end].iter().collect::<String>();
            keys.extend(temp_source_vim_special_key_to_gpui(&token, leader)?);
            index = end + 1;
        } else {
            keys.push(temp_source_vim_literal_key(&character.to_string()));
            index += 1;
        }
    }

    (!keys.is_empty()).then(|| keys.join(" "))
}

fn temp_source_vim_special_key_to_gpui(token: &str, leader: &str) -> Option<Vec<String>> {
    let lower = token.to_ascii_lowercase();
    if lower == "leader" {
        return temp_source_vim_key_sequence_to_gpui(leader, "\\")
            .map(|sequence| sequence.split_whitespace().map(str::to_string).collect());
    }

    if let Some(key) = temp_source_vim_named_key_to_gpui(lower.as_str()) {
        return Some(vec![key]);
    }

    let mut modifiers = Vec::new();
    let mut parts = token.split('-').collect::<Vec<_>>();
    let key = parts.pop()?;
    for modifier in parts {
        match modifier.to_ascii_lowercase().as_str() {
            "c" | "ctrl" | "control" => modifiers.push("ctrl"),
            "a" | "m" | "alt" | "meta" => modifiers.push("alt"),
            "s" | "shift" => modifiers.push("shift"),
            "d" | "cmd" | "command" => modifiers.push("cmd"),
            _ => return None,
        }
    }

    let key = temp_source_vim_named_key_to_gpui(&key.to_ascii_lowercase())
        .unwrap_or_else(|| temp_source_vim_literal_key(key));
    modifiers.push(key.as_str());
    Some(vec![modifiers.join("-")])
}

fn temp_source_vim_named_key_to_gpui(key: &str) -> Option<String> {
    Some(
        match key {
            "space" => "space",
            "cr" | "enter" | "return" => "enter",
            "esc" | "escape" => "escape",
            "tab" => "tab",
            "bs" | "backspace" => "backspace",
            "del" | "delete" => "delete",
            "up" => "up",
            "down" => "down",
            "left" => "left",
            "right" => "right",
            "pageup" | "page-up" => "pageup",
            "pagedown" | "page-down" => "pagedown",
            "home" => "home",
            "end" => "end",
            _ => return None,
        }
        .to_string(),
    )
}

fn temp_source_vim_literal_key(value: &str) -> String {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(' '), None) => "space".to_string(),
        (Some(character), None) if character.is_ascii_uppercase() => {
            format!("shift-{}", character.to_ascii_lowercase())
        }
        _ => value.to_string(),
    }
}

fn temp_source_vim_map_option(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "<silent>" | "<buffer>" | "<nowait>" | "<unique>" | "<script>" | "<special>" | "<expr>"
    )
}

fn temp_source_first_quoted_string(value: &str) -> Option<String> {
    temp_source_quoted_strings(value).into_iter().next()
}

fn temp_source_quoted_strings(value: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut chars = value.char_indices().peekable();

    while let Some((_, character)) = chars.next() {
        if character != '"' && character != '\'' {
            continue;
        }

        let quote = character;
        let mut text = String::new();
        let mut escaped = false;
        for (_, next) in chars.by_ref() {
            if escaped {
                text.push(next);
                escaped = false;
            } else if next == '\\' {
                escaped = true;
            } else if next == quote {
                break;
            } else {
                text.push(next);
            }
        }
        strings.push(text);
    }

    strings
}

pub(crate) fn temp_source_target_for_diff_line_with_refs(
    parsed: &ParsedDiffFile,
    line: &ParsedDiffLine,
    base_reference: &str,
    head_reference: &str,
) -> Option<TempSourceTarget> {
    let side = match line.kind {
        DiffLineKind::Deletion => TempSourceSide::Base,
        DiffLineKind::Addition | DiffLineKind::Context => TempSourceSide::Head,
        DiffLineKind::Meta => return None,
    };

    temp_source_target_for_diff_side_with_refs(parsed, line, side, base_reference, head_reference)
}

fn temp_source_target_for_diff_side_with_refs(
    parsed: &ParsedDiffFile,
    line: &ParsedDiffLine,
    side: TempSourceSide,
    base_reference: &str,
    head_reference: &str,
) -> Option<TempSourceTarget> {
    if line.kind == DiffLineKind::Meta {
        return None;
    }

    let (path, line_number, reference) = match side {
        TempSourceSide::Base => (
            parsed
                .previous_path
                .as_deref()
                .filter(|path| !path.trim().is_empty())
                .unwrap_or(parsed.path.as_str()),
            line.left_line_number,
            base_reference,
        ),
        TempSourceSide::Head => (parsed.path.as_str(), line.right_line_number, head_reference),
    };

    let line = line_number
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0)?;
    let reference = reference.trim();
    if path.trim().is_empty() || reference.is_empty() {
        return None;
    }

    Some(TempSourceTarget {
        path: path.to_string(),
        side,
        line,
        reference: reference.to_string(),
    })
}

pub(crate) fn temp_source_diff_lines_for_target(
    target: &TempSourceTarget,
    parsed: &ParsedDiffFile,
) -> Option<crate::code_display::PreparedFileLineDiffs> {
    (target.side == TempSourceSide::Head).then(|| build_full_file_diff_lines(parsed))
}

fn retry_temp_source_window_target(
    state: &Entity<AppState>,
    target: TempSourceTarget,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        state.temp_source_window.target = Some(target.clone());
        state.temp_source_window.loading = true;
        state.temp_source_window.error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            load_temp_source_file_content_flow(model, target, cx).await;
        })
        .detach();
}

fn render_temp_source_state(message: impl Into<String>) -> impl IntoElement {
    div()
        .flex_grow()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .p(px(20.0))
        .bg(bg_surface())
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(message.into()),
        )
}

fn render_temp_source_error(
    state: Entity<AppState>,
    target: TempSourceTarget,
    error: &str,
) -> impl IntoElement {
    div()
        .flex_grow()
        .min_h_0()
        .bg(bg_surface())
        .p(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .max_w(px(720.0))
                .rounded(radius())
                .border_1()
                .border_color(transparent())
                .bg(bg_overlay())
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Could not load source file"),
                )
                .child(
                    div()
                        .font_family(mono_font_family())
                        .text_size(px(11.0))
                        .text_color(fg_muted())
                        .child(format!(
                            "{}@{}:{}",
                            target.reference, target.path, target.line
                        )),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(danger())
                        .child(error.to_string()),
                )
                .child(
                    div()
                        .w(px(72.0))
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(radius_sm())
                        .border_1()
                        .border_color(transparent())
                        .bg(bg_surface())
                        .text_size(px(12.0))
                        .text_color(fg_emphasis())
                        .hover(|style| style.bg(hover_bg()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            retry_temp_source_window_target(&state, target.clone(), window, cx);
                        })
                        .child("Retry"),
                ),
        )
}

fn temp_source_title(target: &TempSourceTarget) -> String {
    format!("{}:{} ({})", target.path, target.line, target.side.label())
}

fn base_reference(detail: &PullRequestDetail) -> Option<String> {
    detail
        .base_ref_oid
        .clone()
        .or_else(|| Some(detail.base_ref_name.clone()))
        .map(|reference| reference.trim().to_string())
        .filter(|reference| !reference.is_empty())
}

fn head_reference(detail: &PullRequestDetail) -> Option<String> {
    detail
        .head_ref_oid
        .clone()
        .or_else(|| Some(detail.head_ref_name.clone()))
        .map(|reference| reference.trim().to_string())
        .filter(|reference| !reference.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        centered_source_item_ix, temp_source_diff_lines_for_target, temp_source_line_after_delta,
        temp_source_target_for_current_diff_selection, temp_source_target_for_diff_line_with_refs,
        temp_source_vim_key_sequence_to_gpui, temp_source_vim_remaps_from_config,
        TempSourceMovement, TempSourceVimRemap,
    };
    use crate::diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine};
    use crate::github::{PullRequestDataCompleteness, PullRequestDetail};
    use crate::review_ai::DiffAnchor;
    use crate::state::{ReviewLineActionTarget, TempSourceSide, TempSourceTarget};
    use std::collections::BTreeMap;

    fn parsed_file() -> ParsedDiffFile {
        ParsedDiffFile {
            path: "src/new.rs".to_string(),
            previous_path: Some("src/old.rs".to_string()),
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -10,2 +10,2 @@".to_string(),
                lines: vec![],
            }],
        }
    }

    fn line(
        kind: DiffLineKind,
        left_line_number: Option<i64>,
        right_line_number: Option<i64>,
    ) -> ParsedDiffLine {
        ParsedDiffLine {
            kind,
            prefix: String::new(),
            left_line_number,
            right_line_number,
            content: "let value = 1;".to_string(),
        }
    }

    fn detail_with_parsed(parsed_diff: Vec<ParsedDiffFile>) -> PullRequestDetail {
        PullRequestDetail {
            id: "pr1".to_string(),
            repository: "acme/api".to_string(),
            number: 42,
            title: "Test PR".to_string(),
            body: String::new(),
            url: "https://example.com/pr/42".to_string(),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature/test".to_string(),
            base_ref_oid: Some("base-ref".to_string()),
            head_ref_oid: Some("head-ref".to_string()),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            commits: Vec::new(),
            created_at: "2026-04-17T00:00:00Z".to_string(),
            updated_at: "2026-04-18T00:00:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            viewer_pending_review: None,
            files: Vec::new(),
            raw_diff: String::new(),
            parsed_diff,
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }

    fn anchor(file_path: &str, side: &str, line: i64) -> DiffAnchor {
        DiffAnchor {
            file_path: file_path.to_string(),
            hunk_header: None,
            line: Some(line),
            side: Some(side.to_string()),
            thread_id: None,
        }
    }

    #[test]
    fn addition_opens_head_right_line() {
        let target = temp_source_target_for_diff_line_with_refs(
            &parsed_file(),
            &line(DiffLineKind::Addition, None, Some(12)),
            "base-ref",
            "head-ref",
        )
        .expect("addition should resolve");

        assert_eq!(target.side, TempSourceSide::Head);
        assert_eq!(target.path, "src/new.rs");
        assert_eq!(target.line, 12);
        assert_eq!(target.reference, "head-ref");
    }

    #[test]
    fn context_opens_head_right_line() {
        let target = temp_source_target_for_diff_line_with_refs(
            &parsed_file(),
            &line(DiffLineKind::Context, Some(11), Some(13)),
            "base-ref",
            "head-ref",
        )
        .expect("context should resolve");

        assert_eq!(target.side, TempSourceSide::Head);
        assert_eq!(target.path, "src/new.rs");
        assert_eq!(target.line, 13);
    }

    #[test]
    fn deletion_opens_base_left_line() {
        let target = temp_source_target_for_diff_line_with_refs(
            &parsed_file(),
            &line(DiffLineKind::Deletion, Some(14), None),
            "base-ref",
            "head-ref",
        )
        .expect("deletion should resolve");

        assert_eq!(target.side, TempSourceSide::Base);
        assert_eq!(target.path, "src/old.rs");
        assert_eq!(target.line, 14);
        assert_eq!(target.reference, "base-ref");
    }

    #[test]
    fn renamed_deletion_uses_previous_path() {
        let target = temp_source_target_for_diff_line_with_refs(
            &parsed_file(),
            &line(DiffLineKind::Deletion, Some(7), None),
            "base-ref",
            "head-ref",
        )
        .expect("renamed deletion should resolve");

        assert_eq!(target.path, "src/old.rs");
    }

    #[test]
    fn meta_rows_do_not_open_source_targets() {
        let target = temp_source_target_for_diff_line_with_refs(
            &parsed_file(),
            &line(DiffLineKind::Meta, None, None),
            "base-ref",
            "head-ref",
        );

        assert!(target.is_none());
    }

    #[test]
    fn current_diff_selection_prefers_active_line_action() {
        let mut parsed = parsed_file();
        parsed.hunks[0].lines = vec![
            line(DiffLineKind::Addition, None, Some(12)),
            line(DiffLineKind::Deletion, Some(14), None),
        ];
        let detail = detail_with_parsed(vec![parsed]);
        let active_line_action = ReviewLineActionTarget {
            anchor: anchor("src/new.rs", "LEFT", 14),
            start_line: None,
            start_side: None,
            label: "src/new.rs:14".to_string(),
        };
        let selected_anchor = anchor("src/new.rs", "RIGHT", 12);

        let target = temp_source_target_for_current_diff_selection(
            &detail,
            Some(&active_line_action),
            None,
            Some(&selected_anchor),
        )
        .expect("active line action should resolve");

        assert_eq!(target.side, TempSourceSide::Base);
        assert_eq!(target.path, "src/old.rs");
        assert_eq!(target.line, 14);
    }

    #[test]
    fn current_diff_selection_uses_hovered_source_target_before_selected_anchor() {
        let mut parsed = parsed_file();
        parsed.hunks[0].lines = vec![line(DiffLineKind::Addition, None, Some(12))];
        let detail = detail_with_parsed(vec![parsed]);
        let hovered_target = TempSourceTarget {
            path: "src/hovered.rs".to_string(),
            side: TempSourceSide::Head,
            line: 22,
            reference: "head-ref".to_string(),
        };
        let selected_anchor = anchor("src/new.rs", "RIGHT", 12);

        let target = temp_source_target_for_current_diff_selection(
            &detail,
            None,
            Some(&hovered_target),
            Some(&selected_anchor),
        )
        .expect("hovered source target should resolve");

        assert_eq!(target, hovered_target);
    }

    #[test]
    fn current_diff_selection_falls_back_to_selected_anchor() {
        let mut parsed = parsed_file();
        parsed.hunks[0].lines = vec![line(DiffLineKind::Addition, None, Some(12))];
        let detail = detail_with_parsed(vec![parsed]);
        let selected_anchor = anchor("src/new.rs", "RIGHT", 12);

        let target = temp_source_target_for_current_diff_selection(
            &detail,
            None,
            None,
            Some(&selected_anchor),
        )
        .expect("selected anchor should resolve");

        assert_eq!(target.side, TempSourceSide::Head);
        assert_eq!(target.path, "src/new.rs");
        assert_eq!(target.line, 12);
    }

    #[test]
    fn centered_source_scroll_places_target_near_middle() {
        assert_eq!(centered_source_item_ix(1, 20), 0);
        assert_eq!(centered_source_item_ix(30, 20), 19);
    }

    #[test]
    fn temp_source_line_movement_clamps_to_loaded_file_bounds() {
        assert_eq!(temp_source_line_after_delta(1, -1, 20), 1);
        assert_eq!(temp_source_line_after_delta(10, -1, 20), 9);
        assert_eq!(temp_source_line_after_delta(10, 1, 20), 11);
        assert_eq!(temp_source_line_after_delta(20, 1, 20), 20);
        assert_eq!(temp_source_line_after_delta(1, 1, 0), 0);
    }

    #[test]
    fn temp_source_vim_key_sequence_translates_common_notation() {
        assert_eq!(
            temp_source_vim_key_sequence_to_gpui("<C-d>", "\\").as_deref(),
            Some("ctrl-d")
        );
        assert_eq!(
            temp_source_vim_key_sequence_to_gpui("gg", "\\").as_deref(),
            Some("g g")
        );
        assert_eq!(
            temp_source_vim_key_sequence_to_gpui("G", "\\").as_deref(),
            Some("shift-g")
        );
        assert_eq!(
            temp_source_vim_key_sequence_to_gpui("<leader>u", " ").as_deref(),
            Some("space u")
        );
    }

    #[test]
    fn temp_source_vim_config_imports_simple_normal_mode_movement_remaps() {
        let parsed = temp_source_vim_remaps_from_config(
            r#"
let mapleader = ","
nnoremap <silent> <leader>u <C-u>
nnoremap K k
vim.keymap.set("n", "<leader>d", "<C-d>")
vim.api.nvim_set_keymap('n', 'J', '<PageDown>', {})
nnoremap <leader>e G
nnoremap <expr> Y k
inoremap xx <C-d>
"#,
            "\\",
        );

        assert_eq!(parsed.leader, ",");
        assert_eq!(
            parsed.remaps,
            vec![
                TempSourceVimRemap {
                    keystrokes: ", u".to_string(),
                    movement: TempSourceMovement::HalfPageUp,
                },
                TempSourceVimRemap {
                    keystrokes: "shift-k".to_string(),
                    movement: TempSourceMovement::LineUp,
                },
                TempSourceVimRemap {
                    keystrokes: ", d".to_string(),
                    movement: TempSourceMovement::HalfPageDown,
                },
                TempSourceVimRemap {
                    keystrokes: "shift-j".to_string(),
                    movement: TempSourceMovement::PageDown,
                },
                TempSourceVimRemap {
                    keystrokes: ", e".to_string(),
                    movement: TempSourceMovement::End,
                },
            ]
        );

        let parsed = temp_source_vim_remaps_from_config(
            r#"
let mapleader = " "
nnoremap <leader>u <C-u>
"#,
            "\\",
        );
        assert_eq!(parsed.leader, " ");
        assert_eq!(
            parsed.remaps,
            vec![TempSourceVimRemap {
                keystrokes: "space u".to_string(),
                movement: TempSourceMovement::HalfPageUp,
            }]
        );
    }

    #[test]
    fn source_diff_highlighting_is_head_only() {
        let mut parsed = parsed_file();
        parsed.hunks[0].lines = vec![
            line(DiffLineKind::Addition, None, Some(3)),
            line(DiffLineKind::Deletion, Some(4), None),
        ];
        let head_target = temp_source_target_for_diff_line_with_refs(
            &parsed,
            &line(DiffLineKind::Addition, None, Some(3)),
            "base-ref",
            "head-ref",
        )
        .expect("head target");
        let base_target = temp_source_target_for_diff_line_with_refs(
            &parsed,
            &line(DiffLineKind::Deletion, Some(4), None),
            "base-ref",
            "head-ref",
        )
        .expect("base target");

        assert!(temp_source_diff_lines_for_target(&head_target, &parsed).is_some());
        assert!(temp_source_diff_lines_for_target(&base_target, &parsed).is_none());
    }
}
