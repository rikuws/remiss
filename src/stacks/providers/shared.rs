use std::collections::{BTreeMap, BTreeSet};

use super::super::model::{ChangeAtom, ChangeRole, LayerMetrics};

pub(super) fn metrics_for_atoms(atoms: &[&ChangeAtom]) -> LayerMetrics {
    let file_count = atoms
        .iter()
        .map(|atom| atom.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();

    LayerMetrics {
        file_count,
        atom_count: atoms.len(),
        additions: atoms.iter().map(|atom| atom.additions).sum(),
        deletions: atoms.iter().map(|atom| atom.deletions).sum(),
        changed_lines: atoms
            .iter()
            .map(|atom| atom.additions + atom.deletions)
            .sum(),
        unresolved_thread_count: atoms.iter().map(|atom| atom.review_thread_ids.len()).sum(),
        risk_score: atoms.iter().map(|atom| atom.risk_score).sum(),
    }
}

pub(super) fn dominant_role(atoms: &[&ChangeAtom]) -> ChangeRole {
    let mut counts = BTreeMap::<ChangeRole, usize>::new();
    for atom in atoms {
        *counts.entry(atom.role).or_default() += atom.additions + atom.deletions + 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(role, _)| role)
        .unwrap_or(ChangeRole::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stacks::model::ChangeAtomSource;

    #[test]
    fn metrics_for_atoms_sums_counts_across_distinct_files() {
        let first = atom(
            "a",
            "src/lib.rs",
            ChangeRole::CoreLogic,
            3,
            1,
            7,
            &["thread-a"],
        );
        let second = atom("b", "src/lib.rs", ChangeRole::CoreLogic, 0, 2, 2, &[]);
        let third = atom(
            "c",
            "tests/lib.rs",
            ChangeRole::Tests,
            4,
            0,
            1,
            &["thread-b", "thread-c"],
        );

        let metrics = metrics_for_atoms(&[&first, &second, &third]);

        assert_eq!(
            metrics,
            LayerMetrics {
                file_count: 2,
                atom_count: 3,
                additions: 7,
                deletions: 3,
                changed_lines: 10,
                unresolved_thread_count: 3,
                risk_score: 10,
            }
        );
    }

    #[test]
    fn dominant_role_weights_by_changed_lines_with_single_line_floor() {
        let core = atom("a", "src/lib.rs", ChangeRole::CoreLogic, 3, 0, 1, &[]);
        let tests = atom("b", "tests/lib.rs", ChangeRole::Tests, 0, 5, 1, &[]);
        let tiny_core = atom("c", "src/other.rs", ChangeRole::CoreLogic, 0, 0, 1, &[]);

        assert_eq!(
            dominant_role(&[&core, &tests, &tiny_core]),
            ChangeRole::Tests
        );
        assert_eq!(dominant_role(&[]), ChangeRole::Unknown);
    }

    fn atom(
        id: &str,
        path: &str,
        role: ChangeRole,
        additions: usize,
        deletions: usize,
        risk_score: i64,
        review_thread_ids: &[&str],
    ) -> ChangeAtom {
        ChangeAtom {
            id: id.to_string(),
            source: ChangeAtomSource::File,
            path: path.to_string(),
            previous_path: None,
            role,
            semantic_kind: None,
            symbol_name: None,
            defined_symbols: Vec::new(),
            referenced_symbols: Vec::new(),
            old_range: None,
            new_range: None,
            hunk_headers: Vec::new(),
            hunk_indices: Vec::new(),
            additions,
            deletions,
            patch_hash: String::new(),
            risk_score,
            review_thread_ids: review_thread_ids.iter().map(|id| id.to_string()).collect(),
            warnings: Vec::new(),
        }
    }
}
