use gpui::prelude::*;
use gpui::*;

use crate::code_display::{
    build_prepared_file_lsp_context,
    render_virtualized_prepared_file_with_line_numbers_and_focus_flush,
    render_virtualized_prepared_file_with_line_numbers_diffs_and_focus_flush,
};
use crate::code_tour::DiffAnchor;
use crate::diff::{DiffLineKind, ParsedDiffFile, ParsedDiffLine};
use crate::github::PullRequestDetail;
use crate::source_browser::build_full_file_diff_lines;
use crate::state::{AppState, ReviewLineActionTarget, TempSourceSide, TempSourceTarget};
use crate::theme::*;
use crate::views::diff_view::load_temp_source_file_content_flow;

actions!(temp_source_window, [CloseTempSourceWindow]);

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
    cx.bind_keys([KeyBinding::new(
        "escape",
        CloseTempSourceWindow,
        Some(TEMP_SOURCE_WINDOW_KEY_CONTEXT),
    )]);
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

fn centered_source_item_ix(target_line: usize, visible_rows: usize) -> usize {
    target_line
        .saturating_sub(1)
        .saturating_sub(visible_rows / 2)
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
                .border_color(danger())
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
                        .border_color(border_default())
                        .bg(bg_surface())
                        .text_size(px(12.0))
                        .text_color(fg_emphasis())
                        .cursor_pointer()
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
        centered_source_item_ix, temp_source_diff_lines_for_target,
        temp_source_target_for_current_diff_selection, temp_source_target_for_diff_line_with_refs,
    };
    use crate::code_tour::DiffAnchor;
    use crate::diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine};
    use crate::github::{PullRequestDataCompleteness, PullRequestDetail};
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
            created_at: "2026-04-17T00:00:00Z".to_string(),
            updated_at: "2026-04-18T00:00:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
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
