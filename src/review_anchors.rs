use crate::{diff::ParsedDiffLine, github::PullRequestReviewThread, review_ai::DiffAnchor};

pub fn line_matches_diff_anchor(line: &ParsedDiffLine, anchor: Option<&DiffAnchor>) -> bool {
    let Some(anchor) = anchor else {
        return false;
    };
    let Some(side) = anchor.side.as_deref() else {
        return false;
    };
    let Some(line_number) = anchor.line else {
        return false;
    };

    match side {
        "LEFT" => line.left_line_number == Some(line_number),
        "RIGHT" => line.right_line_number == Some(line_number),
        _ => false,
    }
}

pub fn thread_matches_diff_anchor(
    thread: &PullRequestReviewThread,
    anchor: Option<&DiffAnchor>,
) -> bool {
    anchor
        .and_then(|anchor| anchor.thread_id.as_deref())
        .map(|thread_id| thread.id == thread_id)
        .unwrap_or(false)
}

pub fn review_thread_anchor(thread: &PullRequestReviewThread) -> Option<DiffAnchor> {
    if thread.subject_type == "FILE" {
        return Some(DiffAnchor {
            file_path: thread.path.clone(),
            hunk_header: None,
            line: None,
            side: None,
            thread_id: Some(thread.id.clone()),
        });
    }

    let side = if !thread.diff_side.trim().is_empty() {
        thread.diff_side.clone()
    } else {
        thread
            .start_diff_side
            .clone()
            .unwrap_or_else(|| "RIGHT".to_string())
    };

    let line = if side == "LEFT" {
        thread
            .original_line
            .or(thread.line)
            .or(thread.original_start_line)
            .or(thread.start_line)
    } else {
        thread
            .line
            .or(thread.original_line)
            .or(thread.start_line)
            .or(thread.original_start_line)
    };

    Some(DiffAnchor {
        file_path: thread.path.clone(),
        hunk_header: None,
        line,
        side: line.map(|_| side),
        thread_id: Some(thread.id.clone()),
    })
}
