use super::*;

pub(super) fn render_markdown_editor(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    id_prefix: impl Into<String>,
    placeholder: &'static str,
    preview: bool,
    min_height: f32,
    cx: &App,
) -> AnyElement {
    let id_prefix = id_prefix.into();
    let text = markdown_field_text(state.read(cx), field).to_string();
    let suggestions = current_emoji_query(&text)
        .map(|query| emoji_shortcode_suggestions(query, 8))
        .unwrap_or_default();

    div()
        .w_full()
        .min_w_0()
        .rounded(radius())
        .border_1()
        .border_color(transparent())
        .bg(bg_surface())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(render_markdown_editor_tabs(state, field, preview))
        .when(!preview, |el| {
            el.child(render_markdown_toolbar(state, field))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .min_h(px(min_height))
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(14.0))
                        .line_height(px(22.0))
                        .text_color(if text.is_empty() {
                            fg_subtle()
                        } else {
                            fg_emphasis()
                        })
                        .child(
                            AppTextInput::new(
                                format!("{id_prefix}-input"),
                                state.clone(),
                                field,
                                placeholder,
                            )
                            .autofocus(true),
                        ),
                )
                .when(!suggestions.is_empty(), |el| {
                    el.child(render_emoji_suggestions(state, field, suggestions))
                })
        })
        .when(preview, |el| {
            el.child(
                div()
                    .w_full()
                    .min_w_0()
                    .min_h(px(min_height))
                    .px(px(12.0))
                    .py(px(10.0))
                    .bg(bg_surface())
                    .child(if text.trim().is_empty() {
                        div()
                            .text_size(px(14.0))
                            .line_height(px(22.0))
                            .text_color(fg_subtle())
                            .child("Nothing to preview.")
                            .into_any_element()
                    } else {
                        render_markdown(&format!("{id_prefix}-preview"), &text).into_any_element()
                    }),
            )
        })
        .into_any_element()
}

fn render_markdown_editor_tabs(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    preview: bool,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .border_b(px(1.0))
        .border_color(border_muted())
        .bg(bg_overlay())
        .child(markdown_editor_tab("Write", !preview, {
            let state = state.clone();
            move |_, _, cx| set_markdown_preview(&state, field, false, cx)
        }))
        .child(markdown_editor_tab("Preview", preview, {
            let state = state.clone();
            move |_, _, cx| set_markdown_preview(&state, field, true, cx)
        }))
}

fn markdown_editor_tab(
    label: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(13.0))
        .py(px(9.0))
        .border_r(px(1.0))
        .border_color(border_muted())
        .bg(if active { bg_surface() } else { transparent() })
        .text_size(px(12.0))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(label)
}

fn render_markdown_toolbar(state: &Entity<AppState>, field: AppTextFieldKind) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .px(px(8.0))
        .py(px(6.0))
        .border_b(px(1.0))
        .border_color(border_muted())
        .bg(bg_overlay())
        .children([
            markdown_toolbar_button(state, field, LucideIcon::Bold, "Bold", "**bold**"),
            markdown_toolbar_button(state, field, LucideIcon::Italic, "Italic", "_italic_"),
            markdown_toolbar_button(state, field, LucideIcon::Quote, "Quote", "\n> "),
            markdown_toolbar_button(state, field, LucideIcon::Code, "Inline code", "`code`"),
            markdown_toolbar_button(state, field, LucideIcon::Link, "Link", "[text](url)"),
            markdown_toolbar_button(state, field, LucideIcon::List, "Bulleted list", "\n- "),
            markdown_toolbar_button(
                state,
                field,
                LucideIcon::ListOrdered,
                "Numbered list",
                "\n1. ",
            ),
            markdown_toolbar_button(state, field, LucideIcon::ListTodo, "Task list", "\n- [ ] "),
            markdown_toolbar_button(
                state,
                field,
                LucideIcon::MessageSquareDiff,
                "Suggestion",
                "\n```suggestion\n\n```",
            ),
            markdown_toolbar_button(state, field, LucideIcon::SmilePlus, "Emoji", ":"),
        ])
}

fn markdown_toolbar_button(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    icon: LucideIcon,
    tooltip: &'static str,
    snippet: &'static str,
) -> AnyElement {
    let state = state.clone();
    div()
        .id(tooltip)
        .w(px(26.0))
        .h(px(24.0))
        .rounded(radius_sm())
        .flex()
        .items_center()
        .justify_center()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            append_markdown_snippet(&state, field, snippet, cx);
        })
        .child(lucide_icon(icon, 14.0, fg_muted()))
        .into_any_element()
}

fn render_emoji_suggestions(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    suggestions: Vec<EmojiSuggestion>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .px(px(8.0))
        .py(px(7.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .bg(bg_overlay())
        .children(suggestions.into_iter().map(|suggestion| {
            let state = state.clone();
            let shortcode = suggestion.shortcode.clone();
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(7.0))
                .py(px(4.0))
                .rounded(radius_sm())
                .bg(bg_surface())
                .border_1()
                .border_color(transparent())
                .text_size(px(12.0))
                .text_color(fg_default())
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    replace_current_emoji_query(&state, field, &shortcode, cx);
                })
                .child(suggestion.glyph)
                .child(format!(":{}:", suggestion.shortcode))
        }))
}

fn set_markdown_preview(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    preview: bool,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        match field {
            AppTextFieldKind::ReviewBody => state.review_editor_preview = preview,
            AppTextFieldKind::InlineCommentDraft => state.inline_comment_preview = preview,
            _ => {}
        }
        cx.notify();
    });
}

fn append_markdown_snippet(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    snippet: &str,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        let current = markdown_field_text_mut(state, field);
        if !current.is_empty() && !current.ends_with('\n') && snippet.starts_with('\n') {
            current.push('\n');
            current.push_str(snippet.trim_start_matches('\n'));
        } else {
            current.push_str(snippet);
        }
        cx.notify();
    });
}

fn replace_current_emoji_query(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    shortcode: &str,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        let current = markdown_field_text_mut(state, field);
        let Some(start) = current.rfind(':') else {
            return;
        };
        if current[start + 1..].contains(':') {
            return;
        }
        current.truncate(start);
        current.push(':');
        current.push_str(shortcode);
        current.push_str(": ");
        cx.notify();
    });
}

fn markdown_field_text(state: &AppState, field: AppTextFieldKind) -> &str {
    match field {
        AppTextFieldKind::ReviewBody => state.review_body.as_str(),
        AppTextFieldKind::InlineCommentDraft => state.inline_comment_draft.as_str(),
        AppTextFieldKind::WaymarkDraft => state.waymark_draft.as_str(),
        AppTextFieldKind::PaletteQuery => state.palette_query.as_str(),
        AppTextFieldKind::FileChooserQuery => state.file_chooser_query.as_str(),
        AppTextFieldKind::PullRequestFilterName => state.pull_request_filter_preset_name.as_str(),
        AppTextFieldKind::WaypointSpotlightQuery => state.waypoint_spotlight_query.as_str(),
    }
}

fn markdown_field_text_mut(state: &mut AppState, field: AppTextFieldKind) -> &mut String {
    match field {
        AppTextFieldKind::ReviewBody => &mut state.review_body,
        AppTextFieldKind::InlineCommentDraft => &mut state.inline_comment_draft,
        AppTextFieldKind::WaymarkDraft => &mut state.waymark_draft,
        AppTextFieldKind::PaletteQuery => &mut state.palette_query,
        AppTextFieldKind::FileChooserQuery => &mut state.file_chooser_query,
        AppTextFieldKind::PullRequestFilterName => &mut state.pull_request_filter_preset_name,
        AppTextFieldKind::WaypointSpotlightQuery => &mut state.waypoint_spotlight_query,
    }
}

fn current_emoji_query(text: &str) -> Option<&str> {
    let start = text.rfind(':')?;
    let query = &text[start + 1..];
    if query.is_empty() || query.contains(':') || query.chars().any(char::is_whitespace) {
        return None;
    }
    Some(query)
}
