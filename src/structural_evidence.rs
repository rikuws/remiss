use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    cache::CacheStore,
    code_tour::find_parsed_diff_file,
    diff::{DiffLineKind, ParsedDiffHunk, ParsedDiffLine},
    github::PullRequestDetail,
    stacks::model::{ChangeAtom, ChangeAtomSource, ChangeRole, LineRange},
    structural_diff::{
        build_and_cache_structural_diff, build_structural_diff_request,
        structural_result_from_cached, StructuralDiffBuildResult,
    },
    structural_diff_cache::load_cached_structural_diff,
};

pub const STRUCTURAL_EVIDENCE_VERSION: &str = "structural-evidence-v1";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StructuralEvidenceStatus {
    Full,
    Partial,
    Missing,
    Unavailable,
}

impl StructuralEvidenceStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "Structural evidence ready",
            Self::Partial => "Partial structural evidence",
            Self::Missing => "No structural changes",
            Self::Unavailable => "Structural evidence unavailable",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StructuralEvidencePack {
    pub version: String,
    pub files: Vec<StructuralEvidenceFile>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl StructuralEvidencePack {
    pub fn empty() -> Self {
        Self {
            version: STRUCTURAL_EVIDENCE_VERSION.to_string(),
            files: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn status_for_atom_ids(&self, atom_ids: &[String]) -> StructuralEvidenceStatus {
        let mut saw_any = false;
        let mut saw_ready = false;
        let mut saw_partial = false;
        let mut saw_unavailable = false;

        for file in &self.files {
            if !file
                .matched_atom_ids
                .iter()
                .any(|atom_id| atom_ids.iter().any(|candidate| candidate == atom_id))
            {
                continue;
            }
            saw_any = true;
            match file.status {
                StructuralEvidenceStatus::Full => saw_ready = true,
                StructuralEvidenceStatus::Partial => saw_partial = true,
                StructuralEvidenceStatus::Unavailable => saw_unavailable = true,
                StructuralEvidenceStatus::Missing => {}
            }
        }

        if saw_unavailable || saw_partial {
            StructuralEvidenceStatus::Partial
        } else if saw_ready {
            StructuralEvidenceStatus::Full
        } else if saw_any {
            StructuralEvidenceStatus::Missing
        } else {
            StructuralEvidenceStatus::Unavailable
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StructuralEvidenceFile {
    pub path: String,
    #[serde(default)]
    pub previous_path: Option<String>,
    pub status: StructuralEvidenceStatus,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub changes: Vec<StructuralEvidenceChange>,
    #[serde(default)]
    pub matched_atom_ids: Vec<String>,
    #[serde(default)]
    pub unmatched_hunk_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StructuralEvidenceChange {
    pub hunk_index: usize,
    pub hunk_header: String,
    #[serde(default)]
    pub old_range: Option<LineRange>,
    #[serde(default)]
    pub new_range: Option<LineRange>,
    #[serde(default)]
    pub atom_ids: Vec<String>,
    pub changed_line_count: usize,
    #[serde(default)]
    pub snippet: Option<String>,
}

pub fn build_structural_evidence_pack(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    atoms: &[ChangeAtom],
    repository: &str,
    checkout_root: &Path,
    head_oid: &str,
) -> StructuralEvidencePack {
    let mut files = Vec::new();
    let mut warnings = Vec::new();

    for file in &detail.files {
        let parsed = find_parsed_diff_file(&detail.parsed_diff, &file.path);
        let Some(request) = build_structural_diff_request(detail, file, parsed, head_oid) else {
            warnings.push(format!("No structural diff request for {}.", file.path));
            files.push(StructuralEvidenceFile {
                path: file.path.clone(),
                previous_path: parsed.and_then(|parsed| parsed.previous_path.clone()),
                status: StructuralEvidenceStatus::Unavailable,
                message: Some("Structural diff request could not be built.".to_string()),
                changes: Vec::new(),
                matched_atom_ids: atoms_for_file(atoms, &file.path),
                unmatched_hunk_count: 0,
            });
            continue;
        };

        let result = load_cached_structural_diff(cache, &request.cache_key)
            .ok()
            .flatten()
            .map(structural_result_from_cached)
            .unwrap_or_else(|| {
                build_and_cache_structural_diff(cache, repository, checkout_root, &request)
            });

        match result {
            StructuralDiffBuildResult::Ready(diff) => {
                files.push(evidence_file_from_diff(
                    &file.path,
                    atoms,
                    diff.parsed_file.hunks,
                ));
            }
            StructuralDiffBuildResult::TerminalError(message) => {
                files.push(StructuralEvidenceFile {
                    path: file.path.clone(),
                    previous_path: request.previous_path,
                    status: StructuralEvidenceStatus::Unavailable,
                    message: Some(message),
                    changes: Vec::new(),
                    matched_atom_ids: atoms_for_file(atoms, &file.path),
                    unmatched_hunk_count: 0,
                });
            }
            StructuralDiffBuildResult::TransientError(message) => {
                warnings.push(format!(
                    "Structural evidence for {} is partial: {message}",
                    file.path
                ));
                files.push(StructuralEvidenceFile {
                    path: file.path.clone(),
                    previous_path: request.previous_path,
                    status: StructuralEvidenceStatus::Unavailable,
                    message: Some(message),
                    changes: Vec::new(),
                    matched_atom_ids: atoms_for_file(atoms, &file.path),
                    unmatched_hunk_count: 0,
                });
            }
        }
    }

    StructuralEvidencePack {
        version: STRUCTURAL_EVIDENCE_VERSION.to_string(),
        files,
        warnings,
    }
}

pub fn evidence_file_from_diff(
    path: &str,
    atoms: &[ChangeAtom],
    hunks: Vec<ParsedDiffHunk>,
) -> StructuralEvidenceFile {
    let mut changes = Vec::new();
    let mut matched_atom_ids = Vec::<String>::new();
    let mut unmatched_hunk_count = 0usize;

    for (hunk_index, hunk) in hunks.iter().enumerate() {
        let old_range = line_range_for_hunk(hunk, false);
        let new_range = line_range_for_hunk(hunk, true);
        let atom_ids = atoms
            .iter()
            .filter(|atom| {
                atom_matches_structural_hunk(atom, path, old_range.as_ref(), new_range.as_ref())
            })
            .map(|atom| atom.id.clone())
            .collect::<Vec<_>>();

        if atom_ids.is_empty() {
            unmatched_hunk_count += 1;
        } else {
            for atom_id in &atom_ids {
                if !matched_atom_ids.iter().any(|existing| existing == atom_id) {
                    matched_atom_ids.push(atom_id.clone());
                }
            }
        }

        changes.push(StructuralEvidenceChange {
            hunk_index,
            hunk_header: hunk.header.clone(),
            old_range,
            new_range,
            changed_line_count: hunk
                .lines
                .iter()
                .filter(|line| matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion))
                .count(),
            snippet: structural_snippet(hunk),
            atom_ids,
        });
    }

    let status = if changes.is_empty() {
        StructuralEvidenceStatus::Missing
    } else if unmatched_hunk_count > 0 {
        StructuralEvidenceStatus::Partial
    } else {
        StructuralEvidenceStatus::Full
    };

    StructuralEvidenceFile {
        path: path.to_string(),
        previous_path: None,
        status,
        message: None,
        changes,
        matched_atom_ids,
        unmatched_hunk_count,
    }
}

fn atoms_for_file(atoms: &[ChangeAtom], path: &str) -> Vec<String> {
    atoms
        .iter()
        .filter(|atom| atom.path == path || atom.previous_path.as_deref() == Some(path))
        .map(|atom| atom.id.clone())
        .collect()
}

fn atom_matches_structural_hunk(
    atom: &ChangeAtom,
    path: &str,
    old_range: Option<&LineRange>,
    new_range: Option<&LineRange>,
) -> bool {
    if atom.path != path && atom.previous_path.as_deref() != Some(path) {
        return false;
    }

    if atom.role == ChangeRole::Generated
        || matches!(
            atom.source,
            ChangeAtomSource::GeneratedPlaceholder | ChangeAtomSource::BinaryPlaceholder
        )
    {
        return false;
    }

    ranges_overlap(atom.old_range.as_ref(), old_range)
        || ranges_overlap(atom.new_range.as_ref(), new_range)
}

fn ranges_overlap(left: Option<&LineRange>, right: Option<&LineRange>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    left.start <= right.end && right.start <= left.end
}

fn line_range_for_hunk(hunk: &ParsedDiffHunk, right_side: bool) -> Option<LineRange> {
    let numbers = hunk
        .lines
        .iter()
        .filter_map(|line| {
            if right_side {
                line.right_line_number
            } else {
                line.left_line_number
            }
        })
        .collect::<Vec<_>>();
    let start = numbers.iter().min().copied()?;
    let end = numbers.iter().max().copied()?;
    Some(LineRange { start, end })
}

fn structural_snippet(hunk: &ParsedDiffHunk) -> Option<String> {
    let lines = hunk
        .lines
        .iter()
        .filter(|line| matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion))
        .take(8)
        .map(structural_line_snippet)
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| trim_text(&lines.join("\n"), 420))
}

fn structural_line_snippet(line: &ParsedDiffLine) -> String {
    let prefix = match line.kind {
        DiffLineKind::Addition => "+",
        DiffLineKind::Deletion => "-",
        DiffLineKind::Context | DiffLineKind::Meta => " ",
    };
    format!("{prefix}{}", line.content)
}

fn trim_text(value: &str, max_length: usize) -> String {
    let normalized = value.trim();
    if normalized.chars().count() <= max_length {
        return normalized.to_string();
    }
    let truncated = normalized
        .chars()
        .take(max_length.saturating_sub(1))
        .collect::<String>();
    format!("{}…", truncated.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diff::{ParsedDiffHunk, ParsedDiffLine},
        stacks::model::{ChangeAtom, ChangeAtomSource, StackWarning},
    };

    #[test]
    fn maps_structural_hunks_to_atoms_by_line_overlap() {
        let atom = atom(
            "atom-a",
            "src/lib.rs",
            Some(LineRange { start: 10, end: 14 }),
        );
        let hunk = ParsedDiffHunk {
            header: "@@ -10,3 +10,4 @@ fn parse".to_string(),
            lines: vec![
                line(DiffLineKind::Deletion, Some(11), None, "old"),
                line(DiffLineKind::Addition, None, Some(12), "new"),
            ],
        };

        let file = evidence_file_from_diff("src/lib.rs", &[atom], vec![hunk]);

        assert_eq!(file.status, StructuralEvidenceStatus::Full);
        assert_eq!(file.changes[0].atom_ids, vec!["atom-a"]);
        assert_eq!(file.matched_atom_ids, vec!["atom-a"]);
    }

    #[test]
    fn records_partial_evidence_for_unmatched_hunks() {
        let atom = atom(
            "atom-a",
            "src/lib.rs",
            Some(LineRange { start: 30, end: 32 }),
        );
        let hunk = ParsedDiffHunk {
            header: "@@ -10,2 +10,2 @@ fn parse".to_string(),
            lines: vec![line(DiffLineKind::Addition, None, Some(10), "new")],
        };

        let file = evidence_file_from_diff("src/lib.rs", &[atom], vec![hunk]);

        assert_eq!(file.status, StructuralEvidenceStatus::Partial);
        assert_eq!(file.unmatched_hunk_count, 1);
        assert!(file.changes[0].atom_ids.is_empty());
    }

    #[test]
    fn generated_atoms_do_not_claim_structural_hunks() {
        let mut atom = atom(
            "atom-generated",
            "src/lib.rs",
            Some(LineRange { start: 10, end: 12 }),
        );
        atom.role = ChangeRole::Generated;
        atom.source = ChangeAtomSource::GeneratedPlaceholder;
        let hunk = ParsedDiffHunk {
            header: "@@ -10,2 +10,2 @@ generated".to_string(),
            lines: vec![line(DiffLineKind::Addition, None, Some(10), "new")],
        };

        let file = evidence_file_from_diff("src/lib.rs", &[atom], vec![hunk]);

        assert_eq!(file.status, StructuralEvidenceStatus::Partial);
        assert!(file.matched_atom_ids.is_empty());
    }

    fn atom(id: &str, path: &str, new_range: Option<LineRange>) -> ChangeAtom {
        ChangeAtom {
            id: id.to_string(),
            source: ChangeAtomSource::SemanticSection {
                section_id: "section".to_string(),
            },
            path: path.to_string(),
            previous_path: None,
            role: ChangeRole::CoreLogic,
            semantic_kind: Some("logic".to_string()),
            symbol_name: Some("parse".to_string()),
            defined_symbols: Vec::new(),
            referenced_symbols: Vec::new(),
            old_range: None,
            new_range,
            hunk_headers: Vec::new(),
            hunk_indices: Vec::new(),
            additions: 1,
            deletions: 1,
            patch_hash: "hash".to_string(),
            risk_score: 1,
            review_thread_ids: Vec::new(),
            warnings: Vec::<StackWarning>::new(),
        }
    }

    fn line(
        kind: DiffLineKind,
        left_line_number: Option<i64>,
        right_line_number: Option<i64>,
        content: &str,
    ) -> ParsedDiffLine {
        let prefix = match kind {
            DiffLineKind::Addition => "+".to_string(),
            DiffLineKind::Deletion => "-".to_string(),
            DiffLineKind::Context | DiffLineKind::Meta => " ".to_string(),
        };
        ParsedDiffLine {
            kind,
            prefix,
            content: content.to_string(),
            left_line_number,
            right_line_number,
        }
    }
}
