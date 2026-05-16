use std::{
    collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
};

use crate::{
    github::{PullRequestDetail, PullRequestFile},
    state::ReviewFileTreeRow,
};

pub(crate) fn review_file_tree_cache_scope(visible_paths: Option<&BTreeSet<String>>) -> String {
    match visible_paths {
        None => "review-file-tree-rows:all".to_string(),
        Some(paths) => {
            let mut hasher = DefaultHasher::new();
            paths.hash(&mut hasher);
            format!(
                "review-file-tree-rows:stack-filter:{}:{:x}",
                paths.len(),
                hasher.finish()
            )
        }
    }
}

pub(crate) fn review_file_tree_totals(
    detail: &PullRequestDetail,
    visible_paths: Option<&BTreeSet<String>>,
) -> (usize, i64, i64) {
    detail
        .files
        .iter()
        .filter(|file| {
            visible_paths
                .map(|paths| paths.contains(&file.path))
                .unwrap_or(true)
        })
        .fold(
            (0usize, 0i64, 0i64),
            |(count, additions, deletions), file| {
                (
                    count + 1,
                    additions + file.additions,
                    deletions + file.deletions,
                )
            },
        )
}

pub(crate) fn build_review_file_tree_rows(
    detail: &PullRequestDetail,
    visible_paths: Option<&BTreeSet<String>>,
) -> Vec<ReviewFileTreeRow> {
    let entries = detail
        .files
        .iter()
        .filter(|file| {
            visible_paths
                .map(|paths| paths.contains(&file.path))
                .unwrap_or(true)
        })
        .map(|file| ReviewFileTreeEntry {
            path: file.path.clone(),
            additions: file.additions,
            deletions: file.deletions,
        })
        .collect::<Vec<_>>();

    build_file_tree_rows(entries)
}

pub(crate) fn build_repository_file_tree_rows(
    paths: &[String],
    changed_files: &[PullRequestFile],
) -> Vec<ReviewFileTreeRow> {
    let changed_metrics = changed_files
        .iter()
        .map(|file| (file.path.as_str(), (file.additions, file.deletions)))
        .collect::<BTreeMap<_, _>>();
    let entries = paths
        .iter()
        .map(|path| {
            let (additions, deletions) = changed_metrics
                .get(path.as_str())
                .copied()
                .unwrap_or((0, 0));
            ReviewFileTreeEntry {
                path: path.clone(),
                additions,
                deletions,
            }
        })
        .collect::<Vec<_>>();

    build_file_tree_rows(entries)
}

pub(crate) fn ordered_review_files_from_tree_rows<'a>(
    detail: &'a PullRequestDetail,
    tree_rows: &[ReviewFileTreeRow],
    visible_paths: Option<&BTreeSet<String>>,
) -> Vec<&'a PullRequestFile> {
    let mut files = tree_rows
        .iter()
        .filter_map(|row| match row {
            ReviewFileTreeRow::File { path, .. } => {
                detail.files.iter().find(|file| file.path == *path)
            }
            ReviewFileTreeRow::Directory { .. } => None,
        })
        .collect::<Vec<_>>();

    for file in detail.files.iter().filter(|file| {
        visible_paths
            .map(|paths| paths.contains(&file.path))
            .unwrap_or(true)
    }) {
        if !files
            .iter()
            .any(|ordered_file| ordered_file.path == file.path)
        {
            files.push(file);
        }
    }

    files
}

#[derive(Clone)]
struct ReviewFileTreeEntry {
    path: String,
    additions: i64,
    deletions: i64,
}

#[derive(Default)]
struct ReviewFileTreeNode {
    name: String,
    children: BTreeMap<String, ReviewFileTreeNode>,
    entries: Vec<ReviewFileTreeNodeEntry>,
}

enum ReviewFileTreeNodeEntry {
    Directory(String),
    File(ReviewFileTreeEntry),
}

fn build_file_tree_rows(entries: Vec<ReviewFileTreeEntry>) -> Vec<ReviewFileTreeRow> {
    let mut root = ReviewFileTreeNode::default();
    for file in entries {
        let mut cursor = &mut root;
        let mut segments = file.path.split('/').peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_some() {
                if !cursor.children.contains_key(segment) {
                    cursor.children.insert(
                        segment.to_string(),
                        ReviewFileTreeNode {
                            name: segment.to_string(),
                            ..ReviewFileTreeNode::default()
                        },
                    );
                    cursor
                        .entries
                        .push(ReviewFileTreeNodeEntry::Directory(segment.to_string()));
                }
                cursor = cursor
                    .children
                    .get_mut(segment)
                    .expect("inserted directory should be present");
            } else {
                cursor
                    .entries
                    .push(ReviewFileTreeNodeEntry::File(file.clone()));
            }
        }
    }

    let mut rows = Vec::new();
    flatten_review_file_tree(&root, 0, &mut rows);
    rows
}

fn push_review_file_tree_file(
    file: &ReviewFileTreeEntry,
    file_depth: usize,
    rows: &mut Vec<ReviewFileTreeRow>,
) {
    let name = file
        .path
        .rsplit('/')
        .next()
        .unwrap_or(file.path.as_str())
        .to_string();
    rows.push(ReviewFileTreeRow::File {
        path: file.path.clone(),
        name,
        depth: file_depth,
        additions: file.additions,
        deletions: file.deletions,
    });
}

fn review_file_tree_single_directory_child(
    node: &ReviewFileTreeNode,
) -> Option<&ReviewFileTreeNode> {
    if node.entries.len() != 1 {
        return None;
    }

    let ReviewFileTreeNodeEntry::Directory(child_name) = &node.entries[0] else {
        return None;
    };

    node.children.get(child_name)
}

fn review_file_tree_child<'a>(
    node: &'a ReviewFileTreeNode,
    child_name: &str,
) -> Option<&'a ReviewFileTreeNode> {
    node.children.get(child_name)
}

fn flatten_review_file_tree(
    node: &ReviewFileTreeNode,
    depth: usize,
    rows: &mut Vec<ReviewFileTreeRow>,
) {
    if depth == 0 {
        for entry in &node.entries {
            match entry {
                ReviewFileTreeNodeEntry::Directory(child_name) => {
                    if let Some(child) = review_file_tree_child(node, child_name) {
                        flatten_review_file_tree_directory(child, 1, rows);
                    }
                }
                ReviewFileTreeNodeEntry::File(file) => push_review_file_tree_file(file, 0, rows),
            }
        }
        return;
    }

    flatten_review_file_tree_directory(node, depth, rows);
}

fn flatten_review_file_tree_directory(
    node: &ReviewFileTreeNode,
    depth: usize,
    rows: &mut Vec<ReviewFileTreeRow>,
) {
    let (name, node) = compact_review_file_tree_directory(node);
    rows.push(ReviewFileTreeRow::Directory { name, depth });

    for entry in &node.entries {
        match entry {
            ReviewFileTreeNodeEntry::Directory(child_name) => {
                if let Some(child) = review_file_tree_child(node, child_name) {
                    flatten_review_file_tree_directory(child, depth + 1, rows);
                }
            }
            ReviewFileTreeNodeEntry::File(file) => {
                push_review_file_tree_file(file, depth + 1, rows);
            }
        }
    }
}

fn compact_review_file_tree_directory(
    mut node: &ReviewFileTreeNode,
) -> (String, &ReviewFileTreeNode) {
    let mut name = node.name.clone();
    while let Some(child) = review_file_tree_single_directory_child(node) {
        name.push('/');
        name.push_str(&child.name);
        node = child;
    }

    (name, node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tree_compacts_single_child_directory_chains() {
        let rows = build_file_tree_rows(vec![ReviewFileTreeEntry {
            path: "app/src/main/kotlin/com/acme/product/Feature.kt".to_string(),
            additions: 3,
            deletions: 1,
        }]);

        assert_eq!(rows.len(), 2);
        assert_directory_row(&rows[0], "app/src/main/kotlin/com/acme/product", 1);
        assert_file_row(
            &rows[1],
            "app/src/main/kotlin/com/acme/product/Feature.kt",
            "Feature.kt",
            2,
            3,
            1,
        );
    }

    #[test]
    fn file_tree_preserves_branch_points_inside_compact_folders() {
        let rows = build_file_tree_rows(vec![
            ReviewFileTreeEntry {
                path: "app/src/main/kotlin/com/acme/product/Feature.kt".to_string(),
                additions: 2,
                deletions: 0,
            },
            ReviewFileTreeEntry {
                path: "app/src/test/kotlin/com/acme/product/FeatureTest.kt".to_string(),
                additions: 4,
                deletions: 1,
            },
            ReviewFileTreeEntry {
                path: "README.md".to_string(),
                additions: 1,
                deletions: 0,
            },
        ]);

        assert_eq!(rows.len(), 6);
        assert_directory_row(&rows[0], "app/src", 1);
        assert_directory_row(&rows[1], "main/kotlin/com/acme/product", 2);
        assert_file_row(
            &rows[2],
            "app/src/main/kotlin/com/acme/product/Feature.kt",
            "Feature.kt",
            3,
            2,
            0,
        );
        assert_directory_row(&rows[3], "test/kotlin/com/acme/product", 2);
        assert_file_row(
            &rows[4],
            "app/src/test/kotlin/com/acme/product/FeatureTest.kt",
            "FeatureTest.kt",
            3,
            4,
            1,
        );
        assert_file_row(&rows[5], "README.md", "README.md", 0, 1, 0);
    }

    #[test]
    fn file_tree_keeps_root_files_in_diff_order() {
        let rows = build_file_tree_rows(vec![
            ReviewFileTreeEntry {
                path: "README.md".to_string(),
                additions: 1,
                deletions: 0,
            },
            ReviewFileTreeEntry {
                path: "src/lib.rs".to_string(),
                additions: 2,
                deletions: 1,
            },
        ]);

        assert_eq!(rows.len(), 3);
        assert_file_row(&rows[0], "README.md", "README.md", 0, 1, 0);
        assert_directory_row(&rows[1], "src", 1);
        assert_file_row(&rows[2], "src/lib.rs", "lib.rs", 2, 2, 1);
    }

    fn assert_directory_row(row: &ReviewFileTreeRow, expected_name: &str, expected_depth: usize) {
        match row {
            ReviewFileTreeRow::Directory { name, depth } => {
                assert_eq!(name, expected_name);
                assert_eq!(*depth, expected_depth);
            }
            other => panic!("expected directory row, got {other:?}"),
        }
    }

    fn assert_file_row(
        row: &ReviewFileTreeRow,
        expected_path: &str,
        expected_name: &str,
        expected_depth: usize,
        expected_additions: i64,
        expected_deletions: i64,
    ) {
        match row {
            ReviewFileTreeRow::File {
                path,
                name,
                depth,
                additions,
                deletions,
            } => {
                assert_eq!(path, expected_path);
                assert_eq!(name, expected_name);
                assert_eq!(*depth, expected_depth);
                assert_eq!(*additions, expected_additions);
                assert_eq!(*deletions, expected_deletions);
            }
            other => panic!("expected file row, got {other:?}"),
        }
    }
}
