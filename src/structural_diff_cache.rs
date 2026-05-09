use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{
    cache::CacheStore,
    difftastic::AdaptedDifftasticDiffFile,
    github::{PullRequestDetail, PullRequestFile},
};

const STRUCTURAL_DIFF_CACHE_KEY_PREFIX: &str = "structural-diff-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum CachedStructuralDiffResult {
    Ready { diff: AdaptedDifftasticDiffFile },
    TerminalError { message: String },
}

pub fn structural_diff_cache_key(
    detail: &PullRequestDetail,
    head_oid: &str,
    file: &PullRequestFile,
    previous_path: Option<&str>,
) -> String {
    format!(
        "{STRUCTURAL_DIFF_CACHE_KEY_PREFIX}:{}:{}:{}:{}:{}:{}",
        detail.repository,
        detail.number,
        head_oid,
        file.change_type,
        previous_path.unwrap_or_default(),
        file.path
    )
}

pub fn load_cached_structural_diff(
    cache: &CacheStore,
    key: &str,
) -> Result<Option<CachedStructuralDiffResult>, String> {
    Ok(cache
        .get::<CachedStructuralDiffResult>(key)?
        .map(|document| document.value))
}

pub fn save_cached_structural_diff(
    cache: &CacheStore,
    key: &str,
    result: &CachedStructuralDiffResult,
) -> Result<(), String> {
    cache.put(key, result, now_ms())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        cache::CacheStore,
        diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine},
        difftastic::{
            AdaptedDifftasticDiffFile, AdaptedDifftasticSideBySideHunk,
            AdaptedDifftasticSideBySideLineMap,
        },
        github::{PullRequestDetail, PullRequestFile},
    };

    use super::{
        load_cached_structural_diff, save_cached_structural_diff, structural_diff_cache_key,
        CachedStructuralDiffResult,
    };

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_cache() -> CacheStore {
        let unique = format!(
            "remiss-structural-diff-cache-test-{}-{}.sqlite3",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        CacheStore::new(path).expect("cache")
    }

    fn sample_detail(updated_at: &str) -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_kw123".to_string(),
            repository: "acme/widgets".to_string(),
            number: 42,
            title: "Improve widget".to_string(),
            body: String::new(),
            url: "https://github.com/acme/widgets/pull/42".to_string(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: Some("base-oid".to_string()),
            head_ref_oid: Some("head-oid".to_string()),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-05-09T00:00:00Z".to_string(),
            updated_at: updated_at.to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: Default::default(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: Vec::new(),
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: Default::default(),
        }
    }

    fn sample_file() -> PullRequestFile {
        PullRequestFile {
            path: "src/lib.rs".to_string(),
            additions: 1,
            deletions: 1,
            change_type: "MODIFIED".to_string(),
        }
    }

    fn sample_diff() -> AdaptedDifftasticDiffFile {
        AdaptedDifftasticDiffFile {
            parsed_file: ParsedDiffFile {
                path: "src/lib.rs".to_string(),
                previous_path: None,
                hunks: vec![ParsedDiffHunk {
                    header: "@@ -1 +1 @@ structural".to_string(),
                    lines: vec![ParsedDiffLine {
                        kind: DiffLineKind::Addition,
                        prefix: "+".to_string(),
                        left_line_number: None,
                        right_line_number: Some(1),
                        content: "pub fn value() -> i32 { 2 }".to_string(),
                    }],
                }],
                is_binary: false,
            },
            emphasis_hunks: vec![vec![Vec::new()]],
            side_by_side_hunks: vec![AdaptedDifftasticSideBySideHunk { rows: Vec::new() }],
            side_by_side_line_map: vec![vec![Some(AdaptedDifftasticSideBySideLineMap {
                row_index: 0,
                primary: true,
            })]],
        }
    }

    #[test]
    fn structural_diff_cache_key_uses_checkout_head_not_detail_updated_at() {
        let detail = sample_detail("2026-05-09T10:00:00Z");
        let changed_metadata_detail = sample_detail("2026-05-09T11:00:00Z");
        let file = sample_file();

        let head_a = structural_diff_cache_key(&detail, "head-a", &file, None);
        let head_a_after_metadata_change =
            structural_diff_cache_key(&changed_metadata_detail, "head-a", &file, None);
        let head_b = structural_diff_cache_key(&detail, "head-b", &file, None);

        assert_eq!(head_a, head_a_after_metadata_change);
        assert_ne!(head_a, head_b);
    }

    #[test]
    fn structural_diff_cache_roundtrips_ready_and_terminal_error_documents() {
        let cache = temp_cache();
        let ready_key = "structural-diff-test:ready";
        let error_key = "structural-diff-test:error";

        let ready = CachedStructuralDiffResult::Ready {
            diff: sample_diff(),
        };
        let terminal = CachedStructuralDiffResult::TerminalError {
            message: "Structural diff is not available for binary file image.png.".to_string(),
        };

        save_cached_structural_diff(&cache, ready_key, &ready).expect("save ready");
        save_cached_structural_diff(&cache, error_key, &terminal).expect("save error");

        match load_cached_structural_diff(&cache, ready_key).expect("load ready") {
            Some(CachedStructuralDiffResult::Ready { diff }) => {
                assert_eq!(diff.parsed_file.path, "src/lib.rs");
                assert_eq!(
                    diff.side_by_side_line_map[0][0].as_ref().unwrap().row_index,
                    0
                );
            }
            other => panic!("unexpected ready cache value: {other:?}"),
        }

        match load_cached_structural_diff(&cache, error_key).expect("load error") {
            Some(CachedStructuralDiffResult::TerminalError { message }) => {
                assert!(message.contains("binary file"));
            }
            other => panic!("unexpected error cache value: {other:?}"),
        }

        let _ = fs::remove_file(cache.path());
    }
}
