use std::collections::HashSet;

use crate::diff::{build_diff_render_rows, find_parsed_diff_file, DiffRenderRow, ParsedDiffFile};
use crate::github::{PullRequestDetail, PullRequestReviewThread};
use crate::review_ai::DiffAnchor;
use crate::review_anchors::review_thread_anchor;
use crate::review_session::{ReviewCenterMode, ReviewLocation};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ReviewModeFocus {
    pub(super) mode: ReviewCenterMode,
    pub(super) file_path: String,
    pub(super) line: Option<usize>,
    pub(super) side: Option<String>,
    pub(super) anchor: Option<DiffAnchor>,
}

#[derive(Clone)]
pub(super) struct ReviewCommentNavigationItem {
    pub(super) thread_index: usize,
    pub(super) file_index: usize,
    pub(super) row_index: usize,
    pub(super) location: ReviewLocation,
}

pub(super) fn diff_anchor_for_line(
    parsed: &ParsedDiffFile,
    line_number: i64,
    preferred_side: Option<&str>,
) -> Option<DiffAnchor> {
    let preferred_side = preferred_side.filter(|side| *side == "LEFT" || *side == "RIGHT");
    let sides = match preferred_side {
        Some("LEFT") => ["LEFT", "RIGHT"],
        _ => ["RIGHT", "LEFT"],
    };

    for side in sides {
        for hunk in &parsed.hunks {
            if hunk.lines.iter().any(|line| match side {
                "LEFT" => line.left_line_number == Some(line_number),
                "RIGHT" => line.right_line_number == Some(line_number),
                _ => false,
            }) {
                return Some(DiffAnchor {
                    file_path: parsed.path.clone(),
                    hunk_header: Some(hunk.header.clone()),
                    line: Some(line_number),
                    side: Some(side.to_string()),
                    thread_id: None,
                });
            }
        }
    }

    None
}

pub(super) fn review_comment_navigation_items(
    detail: &PullRequestDetail,
    mode: ReviewCenterMode,
) -> Vec<ReviewCommentNavigationItem> {
    let mut seen_threads = HashSet::<usize>::new();
    let mut items = Vec::new();

    for (file_index, file) in detail.files.iter().enumerate() {
        for (row_index, row) in build_diff_render_rows(detail, &file.path)
            .into_iter()
            .enumerate()
        {
            let thread_index = match row {
                DiffRenderRow::FileCommentThread { thread_index }
                | DiffRenderRow::InlineThread { thread_index }
                | DiffRenderRow::OutdatedThread { thread_index } => thread_index,
                _ => continue,
            };
            if seen_threads.insert(thread_index) {
                push_review_comment_navigation_item(
                    &mut items,
                    detail,
                    thread_index,
                    mode,
                    file_index,
                    row_index,
                );
            }
        }
    }

    for thread_index in 0..detail.review_threads.len() {
        if seen_threads.insert(thread_index) {
            let file_index = detail
                .review_threads
                .get(thread_index)
                .and_then(|thread| review_thread_file_index(detail, thread))
                .unwrap_or(detail.files.len());
            push_review_comment_navigation_item(
                &mut items,
                detail,
                thread_index,
                mode,
                file_index,
                usize::MAX,
            );
        }
    }

    items
}

fn push_review_comment_navigation_item(
    items: &mut Vec<ReviewCommentNavigationItem>,
    detail: &PullRequestDetail,
    thread_index: usize,
    mode: ReviewCenterMode,
    file_index: usize,
    row_index: usize,
) {
    let Some(thread) = detail.review_threads.get(thread_index) else {
        return;
    };
    if thread.comments.is_empty() {
        return;
    }
    let Some(anchor) = review_thread_anchor(thread) else {
        return;
    };

    let file_path = if anchor.file_path.is_empty() {
        thread.path.clone()
    } else {
        anchor.file_path.clone()
    };
    let location = review_thread_location(mode, file_path.clone(), anchor);

    items.push(ReviewCommentNavigationItem {
        thread_index,
        file_index,
        row_index,
        location,
    });
}

fn review_thread_location(
    mode: ReviewCenterMode,
    file_path: String,
    anchor: DiffAnchor,
) -> ReviewLocation {
    match mode {
        ReviewCenterMode::StructuralDiff => {
            ReviewLocation::from_structural_diff(file_path, Some(anchor))
        }
        _ => ReviewLocation::from_diff(file_path, Some(anchor)),
    }
}

fn review_thread_file_index(
    detail: &PullRequestDetail,
    thread: &PullRequestReviewThread,
) -> Option<usize> {
    detail
        .files
        .iter()
        .position(|file| file.path == thread.path)
}

pub(super) fn first_review_comment_after_focus_index(
    detail: &PullRequestDetail,
    items: &[ReviewCommentNavigationItem],
    focus: &ReviewModeFocus,
) -> Option<usize> {
    let (focus_file_index, focus_row_index) = review_focus_position(detail, focus)?;
    items.iter().position(|item| {
        item.file_index > focus_file_index
            || (item.file_index == focus_file_index && item.row_index >= focus_row_index)
    })
}

fn review_focus_position(
    detail: &PullRequestDetail,
    focus: &ReviewModeFocus,
) -> Option<(usize, usize)> {
    let file_index = detail
        .files
        .iter()
        .position(|file| file.path == focus.file_path)?;
    let rows = build_diff_render_rows(detail, &focus.file_path);
    let row_index = focus
        .anchor
        .as_ref()
        .and_then(|anchor| review_focus_row_index(detail, &focus.file_path, &rows, anchor))
        .or_else(|| {
            review_focus_line_row_index(
                detail,
                &focus.file_path,
                &rows,
                focus.line,
                focus.side.as_deref(),
            )
        })
        .unwrap_or(0);

    Some((file_index, row_index))
}

fn review_focus_row_index(
    detail: &PullRequestDetail,
    file_path: &str,
    rows: &[DiffRenderRow],
    anchor: &DiffAnchor,
) -> Option<usize> {
    if let Some(thread_id) = anchor.thread_id.as_deref() {
        if let Some(row_index) = rows.iter().position(|row| match row {
            DiffRenderRow::FileCommentThread { thread_index }
            | DiffRenderRow::InlineThread { thread_index }
            | DiffRenderRow::OutdatedThread { thread_index } => detail
                .review_threads
                .get(*thread_index)
                .map(|thread| thread.id == thread_id)
                .unwrap_or(false),
            _ => false,
        }) {
            return Some(row_index);
        }
    }

    review_focus_line_row_index(
        detail,
        file_path,
        rows,
        anchor
            .line
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0),
        anchor.side.as_deref(),
    )
}

fn review_focus_line_row_index(
    detail: &PullRequestDetail,
    file_path: &str,
    rows: &[DiffRenderRow],
    line: Option<usize>,
    preferred_side: Option<&str>,
) -> Option<usize> {
    let line = i64::try_from(line?).ok()?;
    let parsed = find_parsed_diff_file(&detail.parsed_diff, file_path)?;
    let anchor = diff_anchor_for_line(parsed, line, preferred_side)?;

    rows.iter().position(|row| match row {
        DiffRenderRow::Line {
            hunk_index,
            line_index,
        } => parsed
            .hunks
            .get(*hunk_index)
            .and_then(|hunk| hunk.lines.get(*line_index))
            .map(|line| crate::review_anchors::line_matches_diff_anchor(line, Some(&anchor)))
            .unwrap_or(false),
        _ => false,
    })
}
