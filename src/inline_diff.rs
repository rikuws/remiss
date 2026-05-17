use crate::{
    diff::{DiffLineKind, ParsedDiffHunk},
    state::DiffInlineRange,
};

const MAX_INLINE_DIFF_LINE_CHARS: usize = 512;
const MAX_INLINE_DIFF_TOKEN_CHARS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InlineTokenKind {
    Whitespace,
    Word,
    Punctuation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InlineToken {
    pub(crate) text: String,
    pub(crate) column_start: usize,
    pub(crate) column_end: usize,
    pub(crate) kind: InlineTokenKind,
}

pub(crate) fn tokenize_inline_diff_line(content: &str) -> Vec<InlineToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_kind = None;
    let mut token_start = 1usize;
    let mut next_column = 1usize;

    for ch in content.chars() {
        let kind = classify_inline_diff_char(ch);
        if current_kind != Some(kind) && !current.is_empty() {
            tokens.push(InlineToken {
                text: std::mem::take(&mut current),
                column_start: token_start,
                column_end: next_column,
                kind: current_kind.expect("non-empty token should have a kind"),
            });
            token_start = next_column;
        }

        if current.is_empty() {
            token_start = next_column;
        }

        current_kind = Some(kind);
        current.push(ch);
        next_column += 1;
    }

    if !current.is_empty() {
        tokens.push(InlineToken {
            text: current,
            column_start: token_start,
            column_end: next_column,
            kind: current_kind.expect("non-empty token should have a kind"),
        });
    }

    tokens
}

pub(crate) fn inline_token_range(token: &InlineToken) -> DiffInlineRange {
    DiffInlineRange {
        column_start: token.column_start,
        column_end: token.column_end,
    }
}

pub(crate) fn merge_inline_ranges(mut ranges: Vec<DiffInlineRange>) -> Vec<DiffInlineRange> {
    ranges.retain(|range| range.column_start < range.column_end);

    if ranges.len() <= 1 {
        return ranges;
    }

    ranges.sort_by_key(|range| (range.column_start, range.column_end));
    let mut merged: Vec<DiffInlineRange> = Vec::with_capacity(ranges.len());

    for range in ranges {
        match merged.last_mut() {
            Some(previous) if previous.column_end >= range.column_start => {
                previous.column_end = previous.column_end.max(range.column_end);
            }
            _ => merged.push(range),
        }
    }

    merged
}

pub(crate) fn normalize_inline_emphasis_ranges(
    content: &str,
    ranges: &[DiffInlineRange],
) -> Vec<DiffInlineRange> {
    if ranges.is_empty() {
        return Vec::new();
    }

    let tokens = tokenize_inline_diff_line(content);
    let mut normalized = Vec::new();

    for range in ranges {
        if range.column_start >= range.column_end {
            continue;
        }

        normalized.extend(
            tokens
                .iter()
                .filter(|token| token.kind != InlineTokenKind::Whitespace)
                .filter(|token| {
                    token.column_start < range.column_end && token.column_end > range.column_start
                })
                .map(inline_token_range),
        );
    }

    merge_inline_ranges(normalized)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InlineDiffOp {
    Equal,
    Delete(usize),
    Add(usize),
}

fn diff_sequence_by<T, F>(left: &[T], right: &[T], eq: F) -> Vec<InlineDiffOp>
where
    F: Fn(&T, &T) -> bool + Copy,
{
    let mut lcs = vec![vec![0usize; right.len() + 1]; left.len() + 1];

    for left_ix in 0..left.len() {
        for right_ix in 0..right.len() {
            lcs[left_ix + 1][right_ix + 1] = if eq(&left[left_ix], &right[right_ix]) {
                lcs[left_ix][right_ix] + 1
            } else {
                lcs[left_ix + 1][right_ix].max(lcs[left_ix][right_ix + 1])
            };
        }
    }

    let mut left_ix = left.len();
    let mut right_ix = right.len();
    let mut ops = Vec::new();

    while left_ix > 0 || right_ix > 0 {
        if left_ix > 0 && right_ix > 0 && eq(&left[left_ix - 1], &right[right_ix - 1]) {
            ops.push(InlineDiffOp::Equal);
            left_ix -= 1;
            right_ix -= 1;
        } else if right_ix > 0
            && (left_ix == 0 || lcs[left_ix][right_ix - 1] >= lcs[left_ix - 1][right_ix])
        {
            ops.push(InlineDiffOp::Add(right_ix - 1));
            right_ix -= 1;
        } else {
            ops.push(InlineDiffOp::Delete(left_ix - 1));
            left_ix -= 1;
        }
    }

    ops.reverse();
    ops
}

fn diff_single_token_chars(
    left: &InlineToken,
    right: &InlineToken,
) -> (Vec<DiffInlineRange>, Vec<DiffInlineRange>) {
    if left.text == right.text
        || left.text.chars().count() > MAX_INLINE_DIFF_TOKEN_CHARS
        || right.text.chars().count() > MAX_INLINE_DIFF_TOKEN_CHARS
    {
        return (
            vec![inline_token_range(left)],
            vec![inline_token_range(right)],
        );
    }

    let left_chars = left.text.chars().collect::<Vec<_>>();
    let right_chars = right.text.chars().collect::<Vec<_>>();
    let ops = diff_sequence_by(&left_chars, &right_chars, |left, right| left == right);

    let mut left_ranges = Vec::new();
    let mut right_ranges = Vec::new();

    for op in ops {
        match op {
            InlineDiffOp::Equal => {}
            InlineDiffOp::Delete(left_ix) => left_ranges.push(DiffInlineRange {
                column_start: left.column_start + left_ix,
                column_end: left.column_start + left_ix + 1,
            }),
            InlineDiffOp::Add(right_ix) => right_ranges.push(DiffInlineRange {
                column_start: right.column_start + right_ix,
                column_end: right.column_start + right_ix + 1,
            }),
        }
    }

    if left_ranges.is_empty() || right_ranges.is_empty() {
        return (
            vec![inline_token_range(left)],
            vec![inline_token_range(right)],
        );
    }

    (
        vec![inline_token_range(left)],
        vec![inline_token_range(right)],
    )
}

fn apply_inline_diff_group(
    left_tokens: &[InlineToken],
    right_tokens: &[InlineToken],
    deleted_indices: &[usize],
    added_indices: &[usize],
    left_ranges: &mut Vec<DiffInlineRange>,
    right_ranges: &mut Vec<DiffInlineRange>,
) {
    let deleted = deleted_indices
        .iter()
        .filter_map(|ix| left_tokens.get(*ix))
        .filter(|token| token.kind != InlineTokenKind::Whitespace)
        .collect::<Vec<_>>();
    let added = added_indices
        .iter()
        .filter_map(|ix| right_tokens.get(*ix))
        .filter(|token| token.kind != InlineTokenKind::Whitespace)
        .collect::<Vec<_>>();

    if deleted.is_empty() && added.is_empty() {
        return;
    }

    if deleted.len() == 1 && added.len() == 1 {
        let (deleted_chars, added_chars) = diff_single_token_chars(deleted[0], added[0]);
        left_ranges.extend(deleted_chars);
        right_ranges.extend(added_chars);
        return;
    }

    left_ranges.extend(deleted.into_iter().map(inline_token_range));
    right_ranges.extend(added.into_iter().map(inline_token_range));
}

pub(crate) fn compute_inline_emphasis(
    left: &str,
    right: &str,
) -> (Vec<DiffInlineRange>, Vec<DiffInlineRange>) {
    if left == right
        || left.chars().count() > MAX_INLINE_DIFF_LINE_CHARS
        || right.chars().count() > MAX_INLINE_DIFF_LINE_CHARS
    {
        return (Vec::new(), Vec::new());
    }

    let left_tokens = tokenize_inline_diff_line(left);
    let right_tokens = tokenize_inline_diff_line(right);
    let ops = diff_sequence_by(&left_tokens, &right_tokens, |left, right| {
        left.kind == right.kind && left.text == right.text
    });

    let mut left_ranges = Vec::new();
    let mut right_ranges = Vec::new();
    let mut deleted_indices = Vec::new();
    let mut added_indices = Vec::new();

    for op in ops {
        match op {
            InlineDiffOp::Equal => {
                apply_inline_diff_group(
                    &left_tokens,
                    &right_tokens,
                    &deleted_indices,
                    &added_indices,
                    &mut left_ranges,
                    &mut right_ranges,
                );
                deleted_indices.clear();
                added_indices.clear();
            }
            InlineDiffOp::Delete(left_ix) => deleted_indices.push(left_ix),
            InlineDiffOp::Add(right_ix) => added_indices.push(right_ix),
        }
    }

    apply_inline_diff_group(
        &left_tokens,
        &right_tokens,
        &deleted_indices,
        &added_indices,
        &mut left_ranges,
        &mut right_ranges,
    );

    (
        merge_inline_ranges(left_ranges),
        merge_inline_ranges(right_ranges),
    )
}

pub(crate) fn build_hunk_inline_emphasis(hunk: &ParsedDiffHunk) -> Vec<Vec<DiffInlineRange>> {
    let mut emphasis = vec![Vec::new(); hunk.lines.len()];
    let mut line_ix = 0usize;

    while line_ix < hunk.lines.len() {
        if !matches!(
            hunk.lines[line_ix].kind,
            DiffLineKind::Addition | DiffLineKind::Deletion
        ) {
            line_ix += 1;
            continue;
        }

        let mut deletions = Vec::new();
        let mut additions = Vec::new();
        while line_ix < hunk.lines.len()
            && matches!(
                hunk.lines[line_ix].kind,
                DiffLineKind::Addition | DiffLineKind::Deletion
            )
        {
            match hunk.lines[line_ix].kind {
                DiffLineKind::Deletion => deletions.push(line_ix),
                DiffLineKind::Addition => additions.push(line_ix),
                _ => {}
            }
            line_ix += 1;
        }

        for (deleted_ix, added_ix) in deletions.into_iter().zip(additions.into_iter()) {
            let (deleted_ranges, added_ranges) = compute_inline_emphasis(
                hunk.lines[deleted_ix].content.as_str(),
                hunk.lines[added_ix].content.as_str(),
            );
            emphasis[deleted_ix].extend(deleted_ranges);
            emphasis[added_ix].extend(added_ranges);
        }
    }

    emphasis
        .into_iter()
        .zip(hunk.lines.iter())
        .map(|(ranges, line)| normalize_inline_emphasis_ranges(line.content.as_str(), &ranges))
        .collect::<Vec<_>>()
}

fn classify_inline_diff_char(ch: char) -> InlineTokenKind {
    if ch.is_whitespace() {
        InlineTokenKind::Whitespace
    } else if ch == '_' || ch.is_alphanumeric() {
        InlineTokenKind::Word
    } else {
        InlineTokenKind::Punctuation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(column_start: usize, column_end: usize) -> DiffInlineRange {
        DiffInlineRange {
            column_start,
            column_end,
        }
    }

    #[test]
    fn expands_single_character_changes_to_identifier_tokens() {
        assert_eq!(
            normalize_inline_emphasis_ranges("fooBar", &[range(4, 5)]),
            vec![range(1, 7)]
        );
        assert_eq!(
            normalize_inline_emphasis_ranges("fooBaz", &[range(6, 7)]),
            vec![range(1, 7)]
        );
    }

    #[test]
    fn expands_punctuation_changes_to_contiguous_punctuation_runs() {
        assert_eq!(
            normalize_inline_emphasis_ranges("a !== b", &[range(4, 5)]),
            vec![range(3, 6)]
        );
    }

    #[test]
    fn suppresses_whitespace_only_emphasis() {
        assert!(normalize_inline_emphasis_ranges("foo  bar", &[range(4, 6)]).is_empty());
    }

    #[test]
    fn normalizes_difftastic_style_ranges_without_changing_storage_shape() {
        assert_eq!(
            normalize_inline_emphasis_ranges("call(fooBar, baz)", &[range(7, 8), range(9, 10)]),
            vec![range(6, 12)]
        );
    }

    #[test]
    fn inline_emphasis_expands_single_token_character_diffs_to_words() {
        let (left, right) = compute_inline_emphasis("fooBar", "fooBaz");

        assert_eq!(left, vec![range(1, 7)]);
        assert_eq!(right, vec![range(1, 7)]);
    }

    #[test]
    fn inline_emphasis_suppresses_whitespace_only_changes() {
        let (left, right) = compute_inline_emphasis("foo(bar)", "foo( bar )");

        assert!(left.is_empty());
        assert!(right.is_empty());
    }
}
