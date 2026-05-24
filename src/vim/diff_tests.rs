use crate::{
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine},
    review_ai::DiffAnchor,
    state::ReviewLineActionTarget,
};

use super::{
    diff::{
        review_line_action_target_between, review_line_action_targets_for_parsed_file,
        VimDiffOutcome, VimDiffState,
    },
    VimIntent, VimMotion,
};

fn row(file_path: &str, side: &str, line: i64) -> ReviewLineActionTarget {
    ReviewLineActionTarget {
        anchor: DiffAnchor {
            file_path: file_path.to_string(),
            hunk_header: Some("@@ -1,5 +1,5 @@".to_string()),
            line: Some(line),
            side: Some(side.to_string()),
            thread_id: None,
        },
        start_line: None,
        start_side: None,
        label: format!("{file_path}:{line}"),
    }
}

fn move_intent(motion: VimMotion, count: usize) -> VimIntent {
    VimIntent::Move { motion, count }
}

#[test]
fn moves_cursor_through_diff_rows_with_counts() {
    let rows = vec![
        row("src/lib.rs", "RIGHT", 10),
        row("src/lib.rs", "RIGHT", 11),
        row("src/lib.rs", "RIGHT", 12),
        row("src/lib.rs", "RIGHT", 13),
    ];
    let mut state = VimDiffState::new(10);

    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::LineDown, 2)),
        VimDiffOutcome::Moved {
            cursor: rows[2].clone()
        }
    );
    assert_eq!(state.cursor_index(), Some(2));

    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::LineUp, 1)),
        VimDiffOutcome::Moved {
            cursor: rows[1].clone()
        }
    );
    assert_eq!(state.cursor_index(), Some(1));

    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::LineDown, 99)),
        VimDiffOutcome::Moved {
            cursor: rows[3].clone()
        }
    );
    assert_eq!(state.cursor_index(), Some(3));
}

#[test]
fn supports_document_and_page_motion() {
    let rows = (1..=20)
        .map(|line| row("src/lib.rs", "RIGHT", line))
        .collect::<Vec<_>>();
    let mut state = VimDiffState::new(8);

    state.apply_intent(&rows, move_intent(VimMotion::LineDown, 10));
    assert_eq!(state.cursor_index(), Some(10));

    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::HalfPageUp, 1)),
        VimDiffOutcome::Moved {
            cursor: rows[6].clone()
        }
    );
    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::PageDown, 1)),
        VimDiffOutcome::Moved {
            cursor: rows[14].clone()
        }
    );
    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::DocumentStart, 1)),
        VimDiffOutcome::Moved {
            cursor: rows[0].clone()
        }
    );
    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::DocumentEnd, 1)),
        VimDiffOutcome::Moved {
            cursor: rows[19].clone()
        }
    );
}

#[test]
fn absolute_line_targets_current_file_and_side_line_number() {
    let rows = vec![
        row("src/lib.rs", "LEFT", 8),
        row("src/lib.rs", "LEFT", 11),
        row("src/lib.rs", "RIGHT", 7),
        row("src/lib.rs", "RIGHT", 40),
        row("src/main.rs", "RIGHT", 11),
    ];
    let mut state = VimDiffState::new(10);

    state.apply_intent(&rows, move_intent(VimMotion::LineDown, 3));
    assert_eq!(state.cursor_index(), Some(3));
    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::AbsoluteLine, 10)),
        VimDiffOutcome::Moved {
            cursor: rows[2].clone()
        }
    );

    state.apply_intent(&rows, move_intent(VimMotion::DocumentStart, 1));
    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::AbsoluteLine, 10)),
        VimDiffOutcome::Moved {
            cursor: rows[1].clone()
        }
    );
}

#[test]
fn visual_line_selection_builds_review_range_target() {
    let rows = vec![
        row("src/lib.rs", "RIGHT", 10),
        row("src/lib.rs", "RIGHT", 11),
        row("src/lib.rs", "RIGHT", 12),
    ];
    let mut state = VimDiffState::new(10);

    assert_eq!(
        state.apply_intent(&rows, VimIntent::StartVisualLine),
        VimDiffOutcome::VisualStarted {
            origin: rows[0].clone(),
            cursor: rows[0].clone()
        }
    );

    let mut expected = rows[2].clone();
    expected.start_line = Some(10);
    expected.start_side = Some("RIGHT".to_string());
    expected.anchor.line = Some(12);
    expected.label = "src/lib.rs:10-12".to_string();

    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::LineDown, 2)),
        VimDiffOutcome::VisualChanged {
            origin: rows[0].clone(),
            cursor: rows[2].clone(),
            comment_target: expected.clone()
        }
    );
    assert_eq!(state.comment_target(&rows), Some(expected.clone()));
    assert_eq!(
        state.apply_intent(&rows, VimIntent::ConfirmSelection),
        VimDiffOutcome::SelectionConfirmed { target: expected }
    );
    assert_eq!(state.visual_anchor_index(), None);
}

#[test]
fn visual_line_selection_falls_back_to_cursor_when_sides_differ() {
    let rows = vec![
        row("src/lib.rs", "LEFT", 10),
        row("src/lib.rs", "RIGHT", 10),
    ];
    let mut state = VimDiffState::new(10);

    state.apply_intent(&rows, VimIntent::StartVisualLine);
    assert_eq!(
        state.apply_intent(&rows, move_intent(VimMotion::LineDown, 1)),
        VimDiffOutcome::VisualChanged {
            origin: rows[0].clone(),
            cursor: rows[1].clone(),
            comment_target: rows[1].clone()
        }
    );
}

#[test]
fn range_target_requires_matching_file_and_side() {
    let right_10 = row("src/lib.rs", "RIGHT", 10);
    let right_12 = row("src/lib.rs", "RIGHT", 12);
    let other_file = row("src/main.rs", "RIGHT", 12);
    let left_12 = row("src/lib.rs", "LEFT", 12);

    assert!(review_line_action_target_between(&right_10, &right_12).is_some());
    assert_eq!(
        review_line_action_target_between(&right_10, &other_file),
        None
    );
    assert_eq!(review_line_action_target_between(&right_10, &left_12), None);
}

#[test]
fn builds_review_targets_from_parsed_diff_lines() {
    let parsed = ParsedDiffFile {
        path: "src/lib.rs".to_string(),
        previous_path: None,
        hunks: vec![ParsedDiffHunk {
            header: "@@ -3,3 +3,4 @@".to_string(),
            lines: vec![
                parsed_line(DiffLineKind::Context, Some(3), Some(3)),
                parsed_line(DiffLineKind::Deletion, Some(4), None),
                parsed_line(DiffLineKind::Addition, None, Some(4)),
                parsed_line(DiffLineKind::Meta, None, None),
            ],
        }],
        is_binary: false,
    };

    let rows = review_line_action_targets_for_parsed_file("src/lib.rs", &parsed);

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].anchor.side.as_deref(), Some("RIGHT"));
    assert_eq!(rows[0].anchor.line, Some(3));
    assert_eq!(rows[1].anchor.side.as_deref(), Some("LEFT"));
    assert_eq!(rows[1].anchor.line, Some(4));
    assert_eq!(rows[2].anchor.side.as_deref(), Some("RIGHT"));
    assert_eq!(rows[2].anchor.line, Some(4));
    assert!(rows
        .iter()
        .all(|row| row.anchor.hunk_header.as_deref() == Some("@@ -3,3 +3,4 @@")));
}

fn parsed_line(
    kind: DiffLineKind,
    left_line_number: Option<i64>,
    right_line_number: Option<i64>,
) -> ParsedDiffLine {
    ParsedDiffLine {
        kind,
        prefix: String::new(),
        left_line_number,
        right_line_number,
        content: String::new(),
    }
}
