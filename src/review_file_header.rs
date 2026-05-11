use gpui::prelude::*;
use gpui::*;

use crate::github::PullRequestFile;
use crate::icons::{lucide_icon, LucideIcon};
use crate::theme::*;

#[derive(Clone, Debug)]
pub struct ReviewFileHeaderProps {
    pub path: String,
    pub previous_path: Option<String>,
    pub change_type: Option<String>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub binary: bool,
    pub active: bool,
    pub context: Option<String>,
}

impl ReviewFileHeaderProps {
    pub fn from_pull_request_file(file: &PullRequestFile) -> Self {
        Self {
            path: file.path.clone(),
            previous_path: None,
            change_type: Some(file.change_type.clone()),
            additions: Some(file.additions),
            deletions: Some(file.deletions),
            binary: false,
            active: false,
            context: None,
        }
    }

    pub fn from_path(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            previous_path: None,
            change_type: None,
            additions: None,
            deletions: None,
            binary: false,
            active: false,
            context: None,
        }
    }
}

pub fn render_review_file_header(props: ReviewFileHeaderProps) -> AnyElement {
    render_review_file_header_with_action(props, None)
}

pub fn render_review_file_header_with_action(
    props: ReviewFileHeaderProps,
    action: Option<AnyElement>,
) -> AnyElement {
    let display_path = review_file_display_path(&props);

    div()
        .w_full()
        .min_w_0()
        .h(px(48.0))
        .pl(px(20.0))
        .pr(px(16.0))
        .bg(diff_annotation_bg())
        .border_1()
        .border_color(diff_annotation_border())
        .rounded(radius_sm())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(13.0))
                .child(lucide_icon(LucideIcon::ChevronDown, 14.0, fg_muted()))
                .child(
                    div()
                        .min_w_0()
                        .font_family(mono_font_family())
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(display_path),
                )
                .when_some(props.context.clone(), |el, context| {
                    el.child(
                        div()
                            .flex_shrink_0()
                            .font_family(mono_font_family())
                            .text_size(px(10.0))
                            .text_color(fg_muted())
                            .child(context),
                    )
                }),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(lucide_icon(LucideIcon::Copy, 13.0, fg_muted()))
                .when_some(
                    props.additions.zip(props.deletions),
                    |el, (additions, deletions)| {
                        el.when(additions != 0, |el| {
                            el.child(
                                div()
                                    .font_family(mono_font_family())
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(success())
                                    .child(format!("+{additions}")),
                            )
                        })
                        .when(deletions != 0, |el| {
                            el.child(
                                div()
                                    .font_family(mono_font_family())
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(danger())
                                    .child(format!("-{deletions}")),
                            )
                        })
                    },
                )
                .when(props.binary, |el| {
                    el.child(
                        div()
                            .font_family(mono_font_family())
                            .text_size(px(10.0))
                            .text_color(fg_muted())
                            .child("binary"),
                    )
                })
                .when(
                    props.additions.zip(props.deletions) == Some((0, 0))
                        && !props.binary
                        && props.change_type.is_some(),
                    |el| {
                        el.child(
                            div()
                                .font_family(mono_font_family())
                                .text_size(px(10.0))
                                .text_color(fg_muted())
                                .child(
                                    props
                                        .change_type
                                        .as_deref()
                                        .map(review_file_change_type_label)
                                        .unwrap_or("modified"),
                                ),
                        )
                    },
                )
                .when_some(action, |el, action| el.child(action)),
        )
        .into_any_element()
}

fn review_file_display_path(props: &ReviewFileHeaderProps) -> String {
    if let Some(previous_path) = props
        .previous_path
        .as_ref()
        .filter(|path| *path != &props.path)
    {
        format!("{previous_path} -> {}", props.path)
    } else {
        props.path.clone()
    }
}

fn review_file_change_type_label(change_type: &str) -> &'static str {
    match change_type {
        "ADDED" => "added",
        "DELETED" => "deleted",
        "RENAMED" => "renamed",
        "COPIED" => "copied",
        _ => "modified",
    }
}
