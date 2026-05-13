use crate::state::DiffInlineRange;

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
}
