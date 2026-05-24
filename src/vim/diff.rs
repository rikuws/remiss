use crate::{
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffLine},
    review_ai::DiffAnchor,
    state::ReviewLineActionTarget,
};

use super::{VimIntent, VimMotion};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VimDiffOutcome {
    Noop,
    Cancelled,
    Moved {
        cursor: ReviewLineActionTarget,
    },
    VisualStarted {
        origin: ReviewLineActionTarget,
        cursor: ReviewLineActionTarget,
    },
    VisualChanged {
        origin: ReviewLineActionTarget,
        cursor: ReviewLineActionTarget,
        comment_target: ReviewLineActionTarget,
    },
    SelectionConfirmed {
        target: ReviewLineActionTarget,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VimDiffState {
    cursor_index: Option<usize>,
    visual_anchor_index: Option<usize>,
    viewport_rows: usize,
}

impl Default for VimDiffState {
    fn default() -> Self {
        Self::new(24)
    }
}

impl VimDiffState {
    pub fn new(viewport_rows: usize) -> Self {
        Self {
            cursor_index: None,
            visual_anchor_index: None,
            viewport_rows: viewport_rows.max(1),
        }
    }

    pub fn cursor_index(&self) -> Option<usize> {
        self.cursor_index
    }

    pub fn visual_anchor_index(&self) -> Option<usize> {
        self.visual_anchor_index
    }

    pub fn set_cursor_for_target(
        &mut self,
        rows: &[ReviewLineActionTarget],
        target: &ReviewLineActionTarget,
    ) -> bool {
        let Some(index) = rows
            .iter()
            .position(|row| row.stable_key() == target.stable_key())
        else {
            return false;
        };
        self.cursor_index = Some(index);
        true
    }

    pub fn clear_visual_selection(&mut self) {
        self.visual_anchor_index = None;
    }

    pub fn selection_targets(
        &self,
        rows: &[ReviewLineActionTarget],
    ) -> Option<(ReviewLineActionTarget, ReviewLineActionTarget)> {
        let origin = rows.get(self.visual_anchor_index?)?.clone();
        let cursor = rows.get(self.cursor_index?)?.clone();
        Some((origin, cursor))
    }

    pub fn comment_target(
        &self,
        rows: &[ReviewLineActionTarget],
    ) -> Option<ReviewLineActionTarget> {
        let cursor = rows.get(self.cursor_index?)?.clone();
        let Some(anchor_index) = self.visual_anchor_index else {
            return Some(cursor);
        };
        let origin = rows.get(anchor_index)?;
        Some(review_line_action_target_between(origin, &cursor).unwrap_or(cursor))
    }

    pub fn apply_intent(
        &mut self,
        rows: &[ReviewLineActionTarget],
        intent: VimIntent,
    ) -> VimDiffOutcome {
        if rows.is_empty() {
            self.cursor_index = None;
            self.visual_anchor_index = None;
            return VimDiffOutcome::Noop;
        }

        match intent {
            VimIntent::Noop => VimDiffOutcome::Noop,
            VimIntent::Cancel => {
                self.visual_anchor_index = None;
                VimDiffOutcome::Cancelled
            }
            VimIntent::StartVisualLine => {
                let cursor_index = self.ensure_cursor(rows);
                self.visual_anchor_index = Some(cursor_index);
                let cursor = rows[cursor_index].clone();
                VimDiffOutcome::VisualStarted {
                    origin: cursor.clone(),
                    cursor,
                }
            }
            VimIntent::ConfirmSelection => {
                let target = self.comment_target(rows);
                self.visual_anchor_index = None;
                target
                    .map(|target| VimDiffOutcome::SelectionConfirmed { target })
                    .unwrap_or(VimDiffOutcome::Noop)
            }
            VimIntent::Move { motion, count } => {
                let current = self.ensure_cursor(rows);
                let next = self.index_after_motion(rows, current, motion, count);
                self.cursor_index = Some(next);
                let cursor = rows[next].clone();
                if let Some(anchor_index) = self.visual_anchor_index {
                    let origin = rows[anchor_index].clone();
                    let comment_target = review_line_action_target_between(&origin, &cursor)
                        .unwrap_or(cursor.clone());
                    VimDiffOutcome::VisualChanged {
                        origin,
                        cursor,
                        comment_target,
                    }
                } else if next == current {
                    VimDiffOutcome::Noop
                } else {
                    VimDiffOutcome::Moved { cursor }
                }
            }
        }
    }

    fn ensure_cursor(&mut self, rows: &[ReviewLineActionTarget]) -> usize {
        let index = self
            .cursor_index
            .unwrap_or(0)
            .min(rows.len().saturating_sub(1));
        self.cursor_index = Some(index);
        index
    }

    fn index_after_motion(
        &self,
        rows: &[ReviewLineActionTarget],
        current: usize,
        motion: VimMotion,
        count: usize,
    ) -> usize {
        let count = count.max(1);
        match motion {
            VimMotion::LineUp => current.saturating_sub(count),
            VimMotion::LineDown => current
                .saturating_add(count)
                .min(rows.len().saturating_sub(1)),
            VimMotion::HalfPageUp => {
                current.saturating_sub(self.half_page_rows().saturating_mul(count))
            }
            VimMotion::HalfPageDown => current
                .saturating_add(self.half_page_rows().saturating_mul(count))
                .min(rows.len().saturating_sub(1)),
            VimMotion::PageUp => current.saturating_sub(self.viewport_rows.saturating_mul(count)),
            VimMotion::PageDown => current
                .saturating_add(self.viewport_rows.saturating_mul(count))
                .min(rows.len().saturating_sub(1)),
            VimMotion::DocumentStart => 0,
            VimMotion::DocumentEnd => rows.len().saturating_sub(1),
            VimMotion::AbsoluteLine => self
                .index_for_absolute_line(rows, current, count)
                .unwrap_or_else(|| count.saturating_sub(1).min(rows.len().saturating_sub(1))),
            VimMotion::CharLeft
            | VimMotion::CharRight
            | VimMotion::LineStart
            | VimMotion::FirstNonBlank
            | VimMotion::LineEnd
            | VimMotion::WordForward
            | VimMotion::WordBackward
            | VimMotion::WordEnd => current,
        }
    }

    fn half_page_rows(&self) -> usize {
        (self.viewport_rows / 2).max(1)
    }

    fn index_for_absolute_line(
        &self,
        rows: &[ReviewLineActionTarget],
        current: usize,
        line: usize,
    ) -> Option<usize> {
        let current_target = rows.get(current)?;
        let side = current_target.anchor.side.as_deref();
        let file_path = current_target.anchor.file_path.as_str();
        let target_line = i64::try_from(line).ok()?;

        rows.iter()
            .enumerate()
            .filter(|(_, row)| {
                row.anchor.file_path == file_path && row.anchor.side.as_deref() == side
            })
            .min_by_key(|(_, row)| {
                row.anchor
                    .line
                    .map(|row_line| row_line.abs_diff(target_line))
                    .unwrap_or(u64::MAX)
            })
            .map(|(index, _)| index)
    }
}

pub fn review_line_action_target_between(
    origin: &ReviewLineActionTarget,
    cursor: &ReviewLineActionTarget,
) -> Option<ReviewLineActionTarget> {
    if origin.anchor.file_path != cursor.anchor.file_path
        || origin.anchor.side != cursor.anchor.side
    {
        return None;
    }

    let origin_line = origin.anchor.line?;
    let cursor_line = cursor.anchor.line?;
    if origin_line == cursor_line {
        return Some(cursor.clone());
    }

    let start = origin_line.min(cursor_line);
    let end = origin_line.max(cursor_line);
    let mut target = cursor.clone();
    target.start_line = Some(start);
    target.start_side = target.anchor.side.clone();
    target.anchor.line = Some(end);
    target.label = format!("{}:{start}-{end}", target.anchor.file_path);
    Some(target)
}

pub fn review_line_action_targets_for_parsed_file(
    file_path: &str,
    parsed: &ParsedDiffFile,
) -> Vec<ReviewLineActionTarget> {
    parsed
        .hunks
        .iter()
        .flat_map(|hunk| {
            hunk.lines.iter().filter_map(|line| {
                review_line_action_target_for_line(file_path, Some(&hunk.header), line)
            })
        })
        .collect()
}

pub fn review_line_action_target_for_line(
    file_path: &str,
    hunk_header: Option<&str>,
    line: &ParsedDiffLine,
) -> Option<ReviewLineActionTarget> {
    let side = if matches!(line.kind, DiffLineKind::Deletion) {
        Some("LEFT")
    } else if matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Context) {
        Some("RIGHT")
    } else {
        None
    }?;

    let line_number = match side {
        "LEFT" => line.left_line_number,
        _ => line.right_line_number,
    }?;
    let display_line = usize::try_from(line_number).ok().filter(|line| *line > 0)?;

    Some(ReviewLineActionTarget {
        anchor: DiffAnchor {
            file_path: file_path.to_string(),
            hunk_header: hunk_header.map(str::to_string),
            line: Some(line_number),
            side: Some(side.to_string()),
            thread_id: None,
        },
        start_line: None,
        start_side: None,
        label: format!("{file_path}:{display_line}"),
    })
}
