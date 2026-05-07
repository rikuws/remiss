use std::{collections::HashMap, sync::Arc};

#[cfg(test)]
use ::difftastic::DifftasticChunk;
use ::difftastic::{
    diff_texts, DifftasticChange, DifftasticDiff, DifftasticLine,
    DifftasticOptions as LibraryDifftasticOptions, DifftasticSide,
    DifftasticStatus as LibraryDifftasticStatus,
};

use crate::{
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine},
    state::{DiffInlineRange, DiffLineHighlight},
    syntax,
};

#[derive(Clone, Debug, Default)]
pub struct DifftasticAdaptOptions {
    pub context_lines: usize,
}

#[derive(Clone, Debug)]
pub struct AdaptedDifftasticDiffFile {
    pub parsed_file: ParsedDiffFile,
    pub emphasis_hunks: Vec<Vec<Vec<DiffInlineRange>>>,
}

pub fn run_difftastic_for_texts(
    old_name: &str,
    old_text: &str,
    new_name: &str,
    new_text: &str,
) -> Result<DifftasticDiff, String> {
    let options = LibraryDifftasticOptions {
        context_lines: 0,
        ..LibraryDifftasticOptions::default()
    };
    diff_texts(old_name, old_text, new_name, new_text, &options)
        .map_err(|error| format!("difftastic failed: {error}"))
}

pub fn adapt_difftastic_file(
    file: &DifftasticDiff,
    old_text: &str,
    new_text: &str,
    path: impl Into<String>,
    previous_path: Option<String>,
    options: &DifftasticAdaptOptions,
) -> AdaptedDifftasticDiffFile {
    let old_lines = source_lines(old_text);
    let new_lines = source_lines(new_text);
    let path = path.into();

    let (hunks, emphasis_hunks) = match file.status {
        LibraryDifftasticStatus::Unchanged => (Vec::new(), Vec::new()),
        LibraryDifftasticStatus::Created => build_created_hunk(&new_lines),
        LibraryDifftasticStatus::Deleted => build_deleted_hunk(&old_lines),
        LibraryDifftasticStatus::Changed => {
            build_changed_hunks(file, &old_lines, &new_lines, options)
        }
    };

    AdaptedDifftasticDiffFile {
        parsed_file: ParsedDiffFile {
            path,
            previous_path,
            hunks,
            is_binary: false,
        },
        emphasis_hunks,
    }
}

pub fn build_adapted_diff_highlights(
    adapted: &AdaptedDifftasticDiffFile,
) -> Arc<Vec<Vec<DiffLineHighlight>>> {
    Arc::new(
        adapted
            .parsed_file
            .hunks
            .iter()
            .enumerate()
            .map(|(hunk_ix, hunk)| {
                let syntax_lines = syntax::highlight_lines(
                    adapted.parsed_file.path.as_str(),
                    hunk.lines.iter().map(|line| line.content.as_str()),
                );
                let hunk_emphasis = adapted.emphasis_hunks.get(hunk_ix);

                hunk.lines
                    .iter()
                    .enumerate()
                    .map(|(line_ix, _)| DiffLineHighlight {
                        syntax_spans: syntax_lines.get(line_ix).cloned().unwrap_or_default(),
                        emphasis_ranges: hunk_emphasis
                            .and_then(|lines| lines.get(line_ix))
                            .cloned()
                            .unwrap_or_default(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

fn build_changed_hunks(
    file: &DifftasticDiff,
    old_lines: &[&str],
    new_lines: &[&str],
    options: &DifftasticAdaptOptions,
) -> (Vec<ParsedDiffHunk>, Vec<Vec<Vec<DiffInlineRange>>>) {
    let aligned_index = file
        .aligned_lines
        .iter()
        .copied()
        .enumerate()
        .map(|(ix, pair)| (pair, ix))
        .collect::<HashMap<_, _>>();

    file.chunks
        .iter()
        .filter_map(|chunk| {
            let rows = expanded_chunk_rows(
                chunk.lines.as_slice(),
                file.aligned_lines.as_slice(),
                &aligned_index,
                options,
            );
            build_hunk_from_rows(rows.as_slice(), old_lines, new_lines)
        })
        .unzip()
}

fn expanded_chunk_rows(
    chunk: &[DifftasticLine],
    aligned_lines: &[(Option<u32>, Option<u32>)],
    aligned_index: &HashMap<(Option<u32>, Option<u32>), usize>,
    options: &DifftasticAdaptOptions,
) -> Vec<AdaptRow> {
    let changed_rows = chunk
        .iter()
        .map(|line| AdaptRow::from_difftastic_line(line, true))
        .collect::<Vec<_>>();

    if options.context_lines == 0 || aligned_lines.is_empty() {
        return changed_rows;
    }

    let indexes = changed_row_indexes(&changed_rows, aligned_index);

    let Some(min_ix) = indexes.iter().min().copied() else {
        return changed_rows;
    };
    let Some(max_ix) = indexes.iter().max().copied() else {
        return changed_rows;
    };

    let changed_by_pair = changed_rows
        .into_iter()
        .map(|row| (row.pair(), row))
        .collect::<HashMap<_, _>>();

    let start = min_ix.saturating_sub(options.context_lines);
    let end = (max_ix + options.context_lines + 1).min(aligned_lines.len());

    aligned_lines[start..end]
        .iter()
        .copied()
        .map(|pair| {
            changed_by_pair
                .get(&pair)
                .cloned()
                .unwrap_or_else(|| AdaptRow::from_pair(pair, false))
        })
        .collect()
}

fn changed_row_indexes(
    rows: &[AdaptRow],
    aligned_index: &HashMap<(Option<u32>, Option<u32>), usize>,
) -> Vec<usize> {
    rows.iter()
        .map(AdaptRow::pair)
        .filter_map(|pair| aligned_index.get(&pair).copied())
        .collect()
}

fn build_hunk_from_rows(
    rows: &[AdaptRow],
    old_lines: &[&str],
    new_lines: &[&str],
) -> Option<(ParsedDiffHunk, Vec<Vec<DiffInlineRange>>)> {
    if rows.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    let mut emphasis = Vec::new();

    for row in rows {
        match (&row.lhs, &row.rhs) {
            (Some(lhs), Some(rhs)) if row.changed => {
                push_adapted_line(
                    &mut lines,
                    &mut emphasis,
                    DiffLineKind::Deletion,
                    "-",
                    Some(lhs.line_number),
                    None,
                    old_lines,
                    &lhs.changes,
                );
                push_adapted_line(
                    &mut lines,
                    &mut emphasis,
                    DiffLineKind::Addition,
                    "+",
                    None,
                    Some(rhs.line_number),
                    new_lines,
                    &rhs.changes,
                );
            }
            (Some(lhs), Some(rhs)) => {
                lines.push(ParsedDiffLine {
                    kind: DiffLineKind::Context,
                    prefix: " ".to_string(),
                    left_line_number: Some(ui_line_number(lhs.line_number)),
                    right_line_number: Some(ui_line_number(rhs.line_number)),
                    content: line_text(old_lines, lhs.line_number),
                });
                emphasis.push(Vec::new());
            }
            (Some(lhs), None) => {
                push_adapted_line(
                    &mut lines,
                    &mut emphasis,
                    DiffLineKind::Deletion,
                    "-",
                    Some(lhs.line_number),
                    None,
                    old_lines,
                    &lhs.changes,
                );
            }
            (None, Some(rhs)) => {
                push_adapted_line(
                    &mut lines,
                    &mut emphasis,
                    DiffLineKind::Addition,
                    "+",
                    None,
                    Some(rhs.line_number),
                    new_lines,
                    &rhs.changes,
                );
            }
            (None, None) => {}
        }
    }

    if lines.is_empty() {
        return None;
    }

    Some((
        ParsedDiffHunk {
            header: hunk_header(lines.as_slice()),
            lines,
        },
        emphasis,
    ))
}

fn build_created_hunk(new_lines: &[&str]) -> (Vec<ParsedDiffHunk>, Vec<Vec<Vec<DiffInlineRange>>>) {
    if new_lines.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let lines = new_lines
        .iter()
        .enumerate()
        .map(|(ix, line)| ParsedDiffLine {
            kind: DiffLineKind::Addition,
            prefix: "+".to_string(),
            left_line_number: None,
            right_line_number: Some((ix + 1) as i64),
            content: (*line).to_string(),
        })
        .collect::<Vec<_>>();
    let emphasis = vec![Vec::new(); lines.len()];

    (
        vec![ParsedDiffHunk {
            header: hunk_header(lines.as_slice()),
            lines,
        }],
        vec![emphasis],
    )
}

fn build_deleted_hunk(old_lines: &[&str]) -> (Vec<ParsedDiffHunk>, Vec<Vec<Vec<DiffInlineRange>>>) {
    if old_lines.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let lines = old_lines
        .iter()
        .enumerate()
        .map(|(ix, line)| ParsedDiffLine {
            kind: DiffLineKind::Deletion,
            prefix: "-".to_string(),
            left_line_number: Some((ix + 1) as i64),
            right_line_number: None,
            content: (*line).to_string(),
        })
        .collect::<Vec<_>>();
    let emphasis = vec![Vec::new(); lines.len()];

    (
        vec![ParsedDiffHunk {
            header: hunk_header(lines.as_slice()),
            lines,
        }],
        vec![emphasis],
    )
}

fn push_adapted_line(
    lines: &mut Vec<ParsedDiffLine>,
    emphasis: &mut Vec<Vec<DiffInlineRange>>,
    kind: DiffLineKind,
    prefix: &str,
    left_line_number: Option<u32>,
    right_line_number: Option<u32>,
    source_lines: &[&str],
    changes: &[DifftasticChange],
) {
    let source_line_number = left_line_number.or(right_line_number).unwrap_or(0);
    let content = line_text(source_lines, source_line_number);
    let ranges = changes_to_ranges(content.as_str(), changes);

    lines.push(ParsedDiffLine {
        kind,
        prefix: prefix.to_string(),
        left_line_number: left_line_number.map(ui_line_number),
        right_line_number: right_line_number.map(ui_line_number),
        content,
    });
    emphasis.push(ranges);
}

fn changes_to_ranges(line: &str, changes: &[DifftasticChange]) -> Vec<DiffInlineRange> {
    let mut ranges = changes
        .iter()
        .filter_map(|change| byte_offsets_to_range(line, change.start, change.end))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| (range.column_start, range.column_end));
    merge_ranges(ranges)
}

fn byte_offsets_to_range(line: &str, start: u32, end: u32) -> Option<DiffInlineRange> {
    if start >= end {
        return None;
    }

    let start = nearest_char_boundary(line, (start as usize).min(line.len()));
    let end = nearest_char_boundary(line, (end as usize).min(line.len()));
    if start >= end {
        return None;
    }

    Some(DiffInlineRange {
        column_start: line[..start].chars().count() + 1,
        column_end: line[..end].chars().count() + 1,
    })
}

fn nearest_char_boundary(line: &str, byte_ix: usize) -> usize {
    if line.is_char_boundary(byte_ix) {
        return byte_ix;
    }

    (0..=byte_ix)
        .rev()
        .find(|ix| line.is_char_boundary(*ix))
        .unwrap_or(0)
}

fn merge_ranges(ranges: Vec<DiffInlineRange>) -> Vec<DiffInlineRange> {
    let mut merged: Vec<DiffInlineRange> = Vec::new();

    for range in ranges {
        if range.column_start >= range.column_end {
            continue;
        }

        if let Some(last) = merged.last_mut() {
            if range.column_start <= last.column_end {
                last.column_end = last.column_end.max(range.column_end);
                continue;
            }
        }

        merged.push(range);
    }

    merged
}

fn hunk_header(lines: &[ParsedDiffLine]) -> String {
    let left = hunk_range(lines.iter().filter_map(|line| line.left_line_number));
    let right = hunk_range(lines.iter().filter_map(|line| line.right_line_number));
    format!("@@ -{left} +{right} @@ structural")
}

fn hunk_range(numbers: impl Iterator<Item = i64>) -> String {
    let numbers = numbers.collect::<Vec<_>>();
    let Some(start) = numbers.iter().min().copied() else {
        return "0,0".to_string();
    };
    let end = numbers.iter().max().copied().unwrap_or(start);
    let count = end.saturating_sub(start) + 1;
    format!("{start},{count}")
}

fn source_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = text.split('\n').collect::<Vec<_>>();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn line_text(lines: &[&str], line_number: u32) -> String {
    lines
        .get(line_number as usize)
        .copied()
        .unwrap_or_default()
        .to_string()
}

fn ui_line_number(line_number: u32) -> i64 {
    line_number as i64 + 1
}

#[derive(Clone, Debug)]
struct AdaptRow {
    lhs: Option<DifftasticSide>,
    rhs: Option<DifftasticSide>,
    changed: bool,
}

impl AdaptRow {
    fn from_difftastic_line(line: &DifftasticLine, changed: bool) -> Self {
        Self {
            lhs: line.lhs.clone(),
            rhs: line.rhs.clone(),
            changed,
        }
    }

    fn from_pair(pair: (Option<u32>, Option<u32>), changed: bool) -> Self {
        Self {
            lhs: pair.0.map(|line_number| DifftasticSide {
                line_number,
                changes: Vec::new(),
            }),
            rhs: pair.1.map(|line_number| DifftasticSide {
                line_number,
                changes: Vec::new(),
            }),
            changed,
        }
    }

    fn pair(&self) -> (Option<u32>, Option<u32>) {
        (
            self.lhs.as_ref().map(|side| side.line_number),
            self.rhs.as_ref().map(|side| side.line_number),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed_diff() -> DifftasticDiff {
        DifftasticDiff {
            aligned_lines: vec![(Some(0), Some(0)), (Some(1), Some(1))],
            chunks: vec![DifftasticChunk {
                lines: vec![DifftasticLine {
                    lhs: Some(DifftasticSide {
                        line_number: 0,
                        changes: vec![
                            DifftasticChange {
                                start: 5,
                                end: 9,
                                content: "gsub".to_string(),
                                highlight: Some("normal".to_string()),
                            },
                            DifftasticChange {
                                start: 23,
                                end: 24,
                                content: ",".to_string(),
                                highlight: Some("normal".to_string()),
                            },
                            DifftasticChange {
                                start: 25,
                                end: 26,
                                content: "x".to_string(),
                                highlight: Some("normal".to_string()),
                            },
                        ],
                    }),
                    rhs: Some(DifftasticSide {
                        line_number: 0,
                        changes: vec![
                            DifftasticChange {
                                start: 5,
                                end: 12,
                                content: "stringr".to_string(),
                                highlight: Some("normal".to_string()),
                            },
                            DifftasticChange {
                                start: 12,
                                end: 14,
                                content: "::".to_string(),
                                highlight: Some("keyword".to_string()),
                            },
                            DifftasticChange {
                                start: 14,
                                end: 25,
                                content: "str_replace".to_string(),
                                highlight: Some("normal".to_string()),
                            },
                            DifftasticChange {
                                start: 26,
                                end: 27,
                                content: "x".to_string(),
                                highlight: Some("normal".to_string()),
                            },
                            DifftasticChange {
                                start: 27,
                                end: 28,
                                content: ",".to_string(),
                                highlight: Some("normal".to_string()),
                            },
                        ],
                    }),
                }],
            }],
            fallback: None,
            language: Some("R".to_string()),
            path: "new.R".to_string(),
            previous_path: None,
            status: LibraryDifftasticStatus::Changed,
        }
    }

    #[test]
    fn adapts_changed_diff_to_parsed_diff_lines_and_emphasis() {
        let file = changed_diff();
        let adapted = adapt_difftastic_file(
            &file,
            "foo(gsub(\"bad\", \"good\", x))\nunchanged()\n",
            "foo(stringr::str_replace(\"bad\", \"good\", x,))\nunchanged()\n",
            "new.R",
            Some("old.R".to_string()),
            &DifftasticAdaptOptions::default(),
        );

        assert_eq!(adapted.parsed_file.path, "new.R");
        assert_eq!(adapted.parsed_file.previous_path.as_deref(), Some("old.R"));
        assert_eq!(adapted.parsed_file.hunks.len(), 1);

        let hunk = &adapted.parsed_file.hunks[0];
        assert_eq!(hunk.header, "@@ -1,1 +1,1 @@ structural");
        assert_eq!(hunk.lines.len(), 2);
        assert_eq!(hunk.lines[0].kind, DiffLineKind::Deletion);
        assert_eq!(hunk.lines[0].left_line_number, Some(1));
        assert_eq!(hunk.lines[0].content, "foo(gsub(\"bad\", \"good\", x))");
        assert_eq!(hunk.lines[1].kind, DiffLineKind::Addition);
        assert_eq!(hunk.lines[1].right_line_number, Some(1));
        assert_eq!(
            hunk.lines[1].content,
            "foo(stringr::str_replace(\"bad\", \"good\", x,))"
        );

        assert_eq!(
            adapted.emphasis_hunks[0][0],
            vec![
                DiffInlineRange {
                    column_start: 6,
                    column_end: 10
                },
                DiffInlineRange {
                    column_start: 24,
                    column_end: 25
                },
                DiffInlineRange {
                    column_start: 26,
                    column_end: 27
                }
            ]
        );
        assert_eq!(
            adapted.emphasis_hunks[0][1],
            vec![
                DiffInlineRange {
                    column_start: 6,
                    column_end: 26
                },
                DiffInlineRange {
                    column_start: 27,
                    column_end: 29
                }
            ]
        );
    }

    #[test]
    fn adapts_created_status_to_addition_hunk() {
        let file = DifftasticDiff {
            aligned_lines: Vec::new(),
            chunks: Vec::new(),
            fallback: None,
            language: Some("Rust".to_string()),
            path: "src/lib.rs".to_string(),
            previous_path: None,
            status: LibraryDifftasticStatus::Created,
        };
        let adapted = adapt_difftastic_file(
            &file,
            "",
            "fn main() {}\nprintln!();\n",
            "src/lib.rs",
            None,
            &DifftasticAdaptOptions::default(),
        );

        assert_eq!(adapted.parsed_file.hunks.len(), 1);
        assert_eq!(
            adapted.parsed_file.hunks[0].header,
            "@@ -0,0 +1,2 @@ structural"
        );
        assert!(adapted.parsed_file.hunks[0]
            .lines
            .iter()
            .all(|line| line.kind == DiffLineKind::Addition));
    }

    #[test]
    fn adapts_one_sided_chunk_lines() {
        let file = DifftasticDiff {
            aligned_lines: Vec::new(),
            chunks: vec![DifftasticChunk {
                lines: vec![DifftasticLine {
                    lhs: None,
                    rhs: Some(DifftasticSide {
                        line_number: 1,
                        changes: vec![DifftasticChange {
                            start: 0,
                            end: 7,
                            content: "created".to_string(),
                            highlight: Some("normal".to_string()),
                        }],
                    }),
                }],
            }],
            fallback: None,
            language: None,
            path: "notes.txt".to_string(),
            previous_path: None,
            status: LibraryDifftasticStatus::Changed,
        };
        let adapted = adapt_difftastic_file(
            &file,
            "stable\n",
            "stable\ncreated\n",
            "notes.txt",
            None,
            &DifftasticAdaptOptions::default(),
        );

        let hunk = &adapted.parsed_file.hunks[0];
        assert_eq!(hunk.header, "@@ -0,0 +2,1 @@ structural");
        assert_eq!(hunk.lines.len(), 1);
        assert_eq!(hunk.lines[0].kind, DiffLineKind::Addition);
        assert_eq!(hunk.lines[0].right_line_number, Some(2));
        assert_eq!(hunk.lines[0].content, "created");
        assert_eq!(
            adapted.emphasis_hunks[0][0],
            vec![DiffInlineRange {
                column_start: 1,
                column_end: 8
            }]
        );
    }

    #[test]
    fn context_lines_expand_from_aligned_lines() {
        let file = changed_diff();
        let adapted = adapt_difftastic_file(
            &file,
            "foo(gsub(\"bad\", \"good\", x))\nunchanged()\n",
            "foo(stringr::str_replace(\"bad\", \"good\", x,))\nunchanged()\n",
            "new.R",
            None,
            &DifftasticAdaptOptions { context_lines: 1 },
        );

        let hunk = &adapted.parsed_file.hunks[0];
        assert_eq!(hunk.header, "@@ -1,2 +1,2 @@ structural");
        assert_eq!(hunk.lines.len(), 3);
        assert_eq!(hunk.lines[2].kind, DiffLineKind::Context);
        assert_eq!(hunk.lines[2].left_line_number, Some(2));
        assert_eq!(hunk.lines[2].right_line_number, Some(2));
        assert_eq!(hunk.lines[2].content, "unchanged()");
        assert!(adapted.emphasis_hunks[0][2].is_empty());
    }

    #[test]
    fn runs_difftastic_library_directly() {
        let diff = run_difftastic_for_texts(
            "src/lib.rs",
            "fn value() -> i32 {\n    1\n}\n",
            "src/lib.rs",
            "fn value() -> i32 {\n    2\n}\n",
        )
        .expect("library diff succeeds");

        assert_eq!(diff.status, LibraryDifftasticStatus::Changed);
        assert!(!diff.chunks.is_empty());
    }
}
