use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

use crate::diff::{parse_unified_diff, DiffLineKind, ParsedDiffFile};
use crate::gh;
use crate::github::{
    ConnectionCompleteness, PullRequestCommit, PullRequestDetail, PullRequestFile,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitDiffFilter {
    All,
    Commit(String),
}

impl CommitDiffFilter {
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    pub fn oid(&self) -> Option<&str> {
        match self {
            Self::All => None,
            Self::Commit(oid) => Some(oid.as_str()),
        }
    }
}

impl Default for CommitDiffFilter {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Clone, Debug, Eq)]
pub struct CommitDiffKey {
    pub repository: String,
    pub number: i64,
    pub oid: String,
    pub url: String,
}

impl PartialEq for CommitDiffKey {
    fn eq(&self, other: &Self) -> bool {
        self.repository == other.repository
            && self.number == other.number
            && self.oid == other.oid
            && self.url == other.url
    }
}

impl Hash for CommitDiffKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.repository.hash(state);
        self.number.hash(state);
        self.oid.hash(state);
        self.url.hash(state);
    }
}

#[derive(Clone, Debug)]
pub struct CommitDiffDataset {
    pub key: CommitDiffKey,
    pub raw_diff: String,
    pub parsed_diff: Vec<ParsedDiffFile>,
    pub files: Vec<PullRequestFile>,
    pub additions: i64,
    pub deletions: i64,
}

impl CommitDiffDataset {
    pub fn changed_files(&self) -> i64 {
        self.files.len() as i64
    }
}

#[derive(Clone, Debug)]
pub enum CommitDiffCacheEntry {
    Loading,
    Loaded(CommitDiffDataset),
    Error(String),
}

impl CommitDiffCacheEntry {
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub fn loaded(&self) -> Option<&CommitDiffDataset> {
        match self {
            Self::Loaded(dataset) => Some(dataset),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommitTimelineItem {
    pub filter: CommitDiffFilter,
    pub label: String,
    pub tooltip: String,
}

#[derive(Clone, Debug, Default)]
pub struct CommitDiffState {
    pub selected_filter: CommitDiffFilter,
    pub preview_filter: Option<CommitDiffFilter>,
    pub entries: HashMap<CommitDiffKey, CommitDiffCacheEntry>,
    pub prefetch_queue: VecDeque<CommitDiffKey>,
    pub in_flight: Option<CommitDiffKey>,
    initialized_identity: Option<String>,
}

impl CommitDiffState {
    pub fn sync_with_detail(&mut self, detail: &PullRequestDetail) {
        let keys = ordered_commit_keys(detail);
        let identity = commit_timeline_identity(detail, &keys);
        let key_set = keys.iter().cloned().collect::<HashSet<_>>();

        if self.initialized_identity.as_deref() != Some(identity.as_str()) {
            self.entries.retain(|key, _| key_set.contains(key));
            self.prefetch_queue = keys
                .iter()
                .filter(|key| !self.entries.contains_key(*key))
                .cloned()
                .collect();
            if self
                .in_flight
                .as_ref()
                .map(|key| !key_set.contains(key))
                .unwrap_or(false)
            {
                self.in_flight = None;
            }
            if !filter_exists_in_detail(detail, &self.selected_filter) {
                self.selected_filter = CommitDiffFilter::All;
            }
            self.preview_filter = None;
            self.initialized_identity = Some(identity);
            return;
        }

        for key in keys {
            if !self.entries.contains_key(&key) && !self.prefetch_queue.contains(&key) {
                self.prefetch_queue.push_back(key);
            }
        }
        self.prioritize_selected(detail);
    }

    pub fn select_filter(&mut self, detail: &PullRequestDetail, filter: CommitDiffFilter) {
        self.sync_with_detail(detail);
        self.selected_filter = if filter_exists_in_detail(detail, &filter) {
            filter
        } else {
            CommitDiffFilter::All
        };
        self.preview_filter = None;
        self.prioritize_selected(detail);
    }

    pub fn selected_key(&self, detail: &PullRequestDetail) -> Option<CommitDiffKey> {
        let oid = self.selected_filter.oid()?;
        ordered_commit_keys(detail)
            .into_iter()
            .find(|key| key.oid == oid)
    }

    pub fn selected_entry<'a>(
        &'a self,
        detail: &PullRequestDetail,
    ) -> Option<&'a CommitDiffCacheEntry> {
        let key = self.selected_key(detail)?;
        self.entries.get(&key)
    }

    pub fn next_fetch_key(&mut self, detail: &PullRequestDetail) -> Option<CommitDiffKey> {
        self.sync_with_detail(detail);
        if self.in_flight.is_some() {
            return None;
        }
        self.prioritize_selected(detail);

        while let Some(key) = self.prefetch_queue.pop_front() {
            if self.entries.contains_key(&key) {
                continue;
            }
            self.entries
                .insert(key.clone(), CommitDiffCacheEntry::Loading);
            self.in_flight = Some(key.clone());
            return Some(key);
        }

        None
    }

    pub fn complete_fetch(
        &mut self,
        key: CommitDiffKey,
        result: Result<CommitDiffDataset, String>,
    ) {
        if self.in_flight.as_ref() == Some(&key) {
            self.in_flight = None;
        }

        let entry = match result {
            Ok(dataset) => CommitDiffCacheEntry::Loaded(dataset),
            Err(error) => CommitDiffCacheEntry::Error(error),
        };
        self.entries.insert(key.clone(), entry);
        self.prefetch_queue.retain(|queued| queued != &key);
    }

    fn prioritize_selected(&mut self, detail: &PullRequestDetail) {
        let Some(key) = self.selected_key(detail) else {
            return;
        };
        if self.entries.contains_key(&key) {
            return;
        }
        self.prefetch_queue.retain(|queued| queued != &key);
        self.prefetch_queue.push_front(key);
    }
}

pub fn ordered_pr_commits(detail: &PullRequestDetail) -> Vec<PullRequestCommit> {
    let mut commits = detail.commits.clone();
    commits.sort_by(|left, right| {
        left.committed_date
            .cmp(&right.committed_date)
            .then_with(|| left.oid.cmp(&right.oid))
    });
    commits
}

pub fn timeline_items(detail: &PullRequestDetail) -> Vec<CommitTimelineItem> {
    let mut items = vec![CommitTimelineItem {
        filter: CommitDiffFilter::All,
        label: "All".to_string(),
        tooltip: "All commits in this pull request".to_string(),
    }];

    items.extend(ordered_pr_commits(detail).into_iter().map(|commit| {
        let short_oid = short_commit_oid(&commit);
        CommitTimelineItem {
            filter: CommitDiffFilter::Commit(commit.oid.clone()),
            label: short_oid,
            tooltip: commit_tooltip(&commit),
        }
    }));

    items
}

pub fn filter_for_timeline_index(
    detail: &PullRequestDetail,
    index: usize,
) -> Option<CommitDiffFilter> {
    timeline_items(detail)
        .get(index)
        .map(|item| item.filter.clone())
}

pub fn nearest_timeline_index(pointer_x: f32, width: f32, item_count: usize) -> Option<usize> {
    if item_count == 0 || width <= 0.0 {
        return None;
    }
    if item_count == 1 {
        return Some(0);
    }

    let item_width = width / item_count as f32;
    let index = (pointer_x / item_width).floor() as isize;
    Some(index.clamp(0, item_count.saturating_sub(1) as isize) as usize)
}

pub fn selected_file_for_commit_detail(
    detail: &PullRequestDetail,
    candidate: Option<String>,
) -> Option<String> {
    candidate
        .filter(|path| detail.files.iter().any(|file| file.path == *path))
        .or_else(|| detail.files.first().map(|file| file.path.clone()))
}

pub fn commit_diff_dataset_from_raw_diff(
    key: CommitDiffKey,
    raw_diff: String,
) -> CommitDiffDataset {
    let parsed_diff = parse_unified_diff(&raw_diff);
    let files = files_from_diff(&raw_diff, &parsed_diff);
    let (additions, deletions) = totals_from_files(&files);
    CommitDiffDataset {
        key,
        raw_diff,
        parsed_diff,
        files,
        additions,
        deletions,
    }
}

pub fn filtered_detail_for_commit(
    detail: &PullRequestDetail,
    dataset: &CommitDiffDataset,
) -> PullRequestDetail {
    let mut filtered = detail.clone();
    filtered.files = dataset.files.clone();
    filtered.raw_diff = dataset.raw_diff.clone();
    filtered.parsed_diff = dataset.parsed_diff.clone();
    filtered.additions = dataset.additions;
    filtered.deletions = dataset.deletions;
    filtered.changed_files = dataset.changed_files();
    filtered.updated_at = format!("{}:commit:{}", detail.updated_at, dataset.key.oid);
    filtered.viewer_pending_review = None;
    for thread in &mut filtered.review_threads {
        thread.viewer_can_reply = false;
        thread.viewer_can_resolve = false;
        thread.viewer_can_unresolve = false;
        for comment in &mut thread.comments {
            comment.viewer_can_update = false;
            comment.viewer_can_delete = false;
        }
    }
    filtered.data_completeness.files = complete_connection_for_count(dataset.changed_files());
    filtered.data_completeness.diff = complete_connection_for_count(dataset.changed_files());
    filtered
}

pub fn fetch_commit_diff(key: &CommitDiffKey) -> Result<CommitDiffDataset, String> {
    let endpoint = format!("repos/{}/commits/{}", key.repository, key.oid);
    let output = gh::run_owned(vec![
        "api".to_string(),
        endpoint,
        "-H".to_string(),
        "Accept: application/vnd.github.diff".to_string(),
    ])?;

    if output.exit_code == Some(0) {
        return Ok(commit_diff_dataset_from_raw_diff(
            key.clone(),
            output.stdout,
        ));
    }

    Err(if !output.stderr.is_empty() {
        format!("Failed to fetch commit diff {}: {}", key.oid, output.stderr)
    } else if !output.stdout.is_empty() {
        format!("Failed to fetch commit diff {}: {}", key.oid, output.stdout)
    } else {
        format!("Failed to fetch commit diff {}.", key.oid)
    })
}

pub fn commit_diff_key(detail: &PullRequestDetail, commit: &PullRequestCommit) -> CommitDiffKey {
    CommitDiffKey {
        repository: detail.repository.clone(),
        number: detail.number,
        oid: commit.oid.clone(),
        url: commit.url.clone(),
    }
}

fn ordered_commit_keys(detail: &PullRequestDetail) -> Vec<CommitDiffKey> {
    ordered_pr_commits(detail)
        .iter()
        .map(|commit| commit_diff_key(detail, commit))
        .collect()
}

fn filter_exists_in_detail(detail: &PullRequestDetail, filter: &CommitDiffFilter) -> bool {
    match filter {
        CommitDiffFilter::All => true,
        CommitDiffFilter::Commit(oid) => detail.commits.iter().any(|commit| commit.oid == *oid),
    }
}

fn commit_timeline_identity(detail: &PullRequestDetail, keys: &[CommitDiffKey]) -> String {
    let commits = keys
        .iter()
        .map(|key| format!("{}@{}", key.oid, key.url))
        .collect::<Vec<_>>()
        .join("|");
    format!("{}#{}:{commits}", detail.repository, detail.number)
}

fn files_from_diff(raw_diff: &str, parsed_diff: &[ParsedDiffFile]) -> Vec<PullRequestFile> {
    let metadata = diff_file_metadata(raw_diff);
    parsed_diff
        .iter()
        .map(|parsed| {
            let additions = parsed
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == DiffLineKind::Addition)
                .count() as i64;
            let deletions = parsed
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == DiffLineKind::Deletion)
                .count() as i64;
            let change_type = metadata.get(&parsed.path).cloned().unwrap_or_else(|| {
                if parsed
                    .previous_path
                    .as_deref()
                    .is_some_and(|previous| previous != parsed.path)
                {
                    "RENAMED".to_string()
                } else {
                    "MODIFIED".to_string()
                }
            });

            PullRequestFile {
                path: parsed.path.clone(),
                additions,
                deletions,
                change_type,
            }
        })
        .collect()
}

fn totals_from_files(files: &[PullRequestFile]) -> (i64, i64) {
    files.iter().fold((0, 0), |(additions, deletions), file| {
        (additions + file.additions, deletions + file.deletions)
    })
}

fn diff_file_metadata(raw_diff: &str) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    let mut current_path = None::<String>;
    let mut current_type = "MODIFIED".to_string();

    for line in raw_diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(path) = current_path.take() {
                metadata.insert(path, current_type.clone());
            }

            let mut parts = rest.split_whitespace();
            let _previous = parts.next();
            current_path = parts.next().map(normalize_diff_path);
            current_type = "MODIFIED".to_string();
            continue;
        }

        if line.starts_with("new file mode ") {
            current_type = "ADDED".to_string();
        } else if line.starts_with("deleted file mode ") {
            current_type = "DELETED".to_string();
        } else if let Some(path) = line.strip_prefix("rename to ") {
            current_path = Some(path.to_string());
            current_type = "RENAMED".to_string();
        } else if let Some(path) = line.strip_prefix("copy to ") {
            current_path = Some(path.to_string());
            current_type = "COPIED".to_string();
        }
    }

    if let Some(path) = current_path.take() {
        metadata.insert(path, current_type);
    }

    metadata
}

fn normalize_diff_path(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .strip_prefix("a/")
        .or_else(|| path.trim().trim_matches('"').strip_prefix("b/"))
        .unwrap_or_else(|| path.trim().trim_matches('"'))
        .to_string()
}

fn complete_connection_for_count(total_count: i64) -> ConnectionCompleteness {
    ConnectionCompleteness {
        loaded_count: total_count.max(0) as usize,
        total_count: total_count.max(0),
        is_complete: true,
        truncated_reason: None,
    }
}

fn short_commit_oid(commit: &PullRequestCommit) -> String {
    if !commit.abbreviated_oid.trim().is_empty() {
        return commit.abbreviated_oid.clone();
    }
    commit.oid.chars().take(7).collect()
}

fn trim_commit_headline(headline: &str, max_len: usize) -> String {
    let headline = headline.split_whitespace().collect::<Vec<_>>().join(" ");
    if headline.chars().count() <= max_len {
        return headline;
    }
    let mut trimmed = headline
        .chars()
        .take(max_len.saturating_sub(3))
        .collect::<String>();
    trimmed.push_str("...");
    trimmed
}

fn commit_tooltip(commit: &PullRequestCommit) -> String {
    let author = commit
        .author_login
        .as_deref()
        .or(commit.author_name.as_deref())
        .unwrap_or("unknown");
    let short_oid = short_commit_oid(commit);
    let headline = trim_commit_headline(&commit.message_headline, 120);
    format!(
        "{headline}\n{short_oid} by {author} on {}",
        commit.committed_date
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{PullRequestDataCompleteness, PullRequestReviewThread};

    fn commit(oid: &str, date: &str, headline: &str) -> PullRequestCommit {
        PullRequestCommit {
            id: format!("C_{oid}"),
            oid: oid.to_string(),
            abbreviated_oid: oid.chars().take(7).collect(),
            message_headline: headline.to_string(),
            committed_date: date.to_string(),
            author_name: Some("Ada".to_string()),
            author_login: Some("ada".to_string()),
            author_avatar_url: None,
            url: format!("https://github.test/commit/{oid}"),
        }
    }

    fn detail(commits: Vec<PullRequestCommit>) -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_kw".to_string(),
            repository: "acme/widgets".to_string(),
            number: 42,
            title: "Test".to_string(),
            body: String::new(),
            url: "https://github.test/acme/widgets/pull/42".to_string(),
            author_login: "ada".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "topic".to_string(),
            base_ref_oid: None,
            head_ref_oid: None,
            additions: 0,
            deletions: 0,
            changed_files: 0,
            comments_count: 0,
            commits_count: commits.len() as i64,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-02T00:00:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: Default::default(),
            comments: Vec::new(),
            commits,
            latest_reviews: Vec::new(),
            review_threads: Vec::<PullRequestReviewThread>::new(),
            viewer_pending_review: None,
            files: Vec::new(),
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }

    #[test]
    fn timeline_orders_all_then_oldest_to_newest_commits() {
        let detail = detail(vec![
            commit("bbbbbbb2", "2026-01-03T00:00:00Z", "second"),
            commit("aaaaaaa1", "2026-01-02T00:00:00Z", "first"),
        ]);

        let items = timeline_items(&detail);

        assert_eq!(items[0].filter, CommitDiffFilter::All);
        assert_eq!(
            items[1].filter,
            CommitDiffFilter::Commit("aaaaaaa1".to_string())
        );
        assert_eq!(
            items[2].filter,
            CommitDiffFilter::Commit("bbbbbbb2".to_string())
        );
    }

    #[test]
    fn nearest_timeline_index_clamps_to_rail_bounds() {
        assert_eq!(nearest_timeline_index(-10.0, 200.0, 4), Some(0));
        assert_eq!(nearest_timeline_index(52.0, 200.0, 4), Some(1));
        assert_eq!(nearest_timeline_index(210.0, 200.0, 4), Some(3));
        assert_eq!(nearest_timeline_index(0.0, 0.0, 4), None);
    }

    #[test]
    fn selected_file_falls_back_to_first_commit_file() {
        let mut detail = detail(Vec::new());
        detail.files = vec![
            PullRequestFile {
                path: "src/a.rs".to_string(),
                additions: 1,
                deletions: 0,
                change_type: "MODIFIED".to_string(),
            },
            PullRequestFile {
                path: "src/b.rs".to_string(),
                additions: 2,
                deletions: 0,
                change_type: "MODIFIED".to_string(),
            },
        ];

        assert_eq!(
            selected_file_for_commit_detail(&detail, Some("src/b.rs".to_string())).as_deref(),
            Some("src/b.rs")
        );
        assert_eq!(
            selected_file_for_commit_detail(&detail, Some("missing.rs".to_string())).as_deref(),
            Some("src/a.rs")
        );
    }

    #[test]
    fn commit_dataset_derives_files_and_totals_from_diff() {
        let detail = detail(vec![commit("aaaaaaa1", "2026-01-02T00:00:00Z", "first")]);
        let key = commit_diff_key(&detail, &detail.commits[0]);
        let raw = r#"diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 context
-old
+new
+added
 same
diff --git a/src/new.rs b/src/new.rs
new file mode 100644
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1 @@
+hello
"#;

        let dataset = commit_diff_dataset_from_raw_diff(key, raw.to_string());

        assert_eq!(dataset.files.len(), 2);
        assert_eq!(dataset.additions, 3);
        assert_eq!(dataset.deletions, 1);
        assert_eq!(dataset.files[1].change_type, "ADDED");
    }

    #[test]
    fn commit_diff_state_skips_cache_hits_and_fetches_sequentially() {
        let detail = detail(vec![
            commit("aaaaaaa1", "2026-01-02T00:00:00Z", "first"),
            commit("bbbbbbb2", "2026-01-03T00:00:00Z", "second"),
        ]);
        let mut state = CommitDiffState::default();
        state.sync_with_detail(&detail);

        let first = state.next_fetch_key(&detail).expect("first fetch");
        assert_eq!(first.oid, "aaaaaaa1");
        assert!(state.next_fetch_key(&detail).is_none());

        state.complete_fetch(
            first.clone(),
            Ok(commit_diff_dataset_from_raw_diff(first, String::new())),
        );
        let second = state.next_fetch_key(&detail).expect("second fetch");
        assert_eq!(second.oid, "bbbbbbb2");
    }

    #[test]
    fn selected_commit_is_prioritized_before_prefetch_reaches_it() {
        let detail = detail(vec![
            commit("aaaaaaa1", "2026-01-02T00:00:00Z", "first"),
            commit("bbbbbbb2", "2026-01-03T00:00:00Z", "second"),
            commit("ccccccc3", "2026-01-04T00:00:00Z", "third"),
        ]);
        let mut state = CommitDiffState::default();
        state.sync_with_detail(&detail);
        state.select_filter(&detail, CommitDiffFilter::Commit("ccccccc3".to_string()));

        let key = state.next_fetch_key(&detail).expect("selected fetch");

        assert_eq!(key.oid, "ccccccc3");
    }

    #[test]
    fn fetch_error_is_cached_and_not_retried_by_prefetch() {
        let detail = detail(vec![commit("aaaaaaa1", "2026-01-02T00:00:00Z", "first")]);
        let mut state = CommitDiffState::default();

        let key = state.next_fetch_key(&detail).expect("fetch");
        state.complete_fetch(key.clone(), Err("network failed".to_string()));

        assert!(matches!(
            state.entries.get(&key),
            Some(CommitDiffCacheEntry::Error(error)) if error == "network failed"
        ));
        assert!(state.next_fetch_key(&detail).is_none());
    }
}
