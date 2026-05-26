use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{diff::ParsedDiffFile, triage::PullRequestTriageSignal};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthState {
    pub is_authenticated: bool,
    pub active_login: Option<String>,
    pub active_hostname: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewer {
    pub login: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestSummary {
    #[serde(default)]
    pub local_key: Option<String>,
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    #[serde(default)]
    pub is_draft: bool,
    pub comments_count: i64,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub state: String,
    #[serde(default = "default_author_association")]
    pub author_association: String,
    pub review_decision: Option<String>,
    pub updated_at: String,
    pub url: String,
    #[serde(default)]
    pub repository_default_branch: Option<String>,
    #[serde(default)]
    pub triage_signals: Vec<PullRequestTriageSignal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestQueue {
    pub id: String,
    pub label: String,
    pub items: Vec<PullRequestSummary>,
    pub total_count: i64,
    #[serde(default = "default_true")]
    pub is_complete: bool,
    #[serde(default)]
    pub truncated_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCachePayload {
    pub viewer: Option<Viewer>,
    pub queues: Vec<PullRequestQueue>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub auth: AuthState,
    pub loaded_from_cache: bool,
    pub fetched_at_ms: Option<i64>,
    pub viewer: Option<Viewer>,
    pub queues: Vec<PullRequestQueue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestFile {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
    #[serde(default = "default_change_type")]
    pub change_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReview {
    #[serde(default)]
    pub id: Option<String>,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPullRequestReview {
    pub id: String,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub body: String,
    pub comments: Vec<PullRequestReviewComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReviewComment {
    pub id: String,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub body: String,
    pub path: String,
    pub line: Option<i64>,
    pub original_line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_start_line: Option<i64>,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub published_at: Option<String>,
    pub reply_to_id: Option<String>,
    #[serde(default)]
    pub viewer_can_update: bool,
    #[serde(default)]
    pub viewer_can_delete: bool,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestComment {
    pub id: String,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestCommit {
    pub id: String,
    pub oid: String,
    pub abbreviated_oid: String,
    pub message_headline: String,
    pub committed_date: String,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub author_login: Option<String>,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReviewThread {
    pub id: String,
    pub path: String,
    pub line: Option<i64>,
    pub original_line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_start_line: Option<i64>,
    pub diff_side: String,
    pub start_diff_side: Option<String>,
    pub is_collapsed: bool,
    pub is_outdated: bool,
    pub is_resolved: bool,
    pub subject_type: String,
    pub resolved_by_login: Option<String>,
    pub viewer_can_reply: bool,
    pub viewer_can_resolve: bool,
    pub viewer_can_unresolve: bool,
    pub comments: Vec<PullRequestReviewComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionCompleteness {
    pub loaded_count: usize,
    pub total_count: i64,
    pub is_complete: bool,
    #[serde(default)]
    pub truncated_reason: Option<String>,
}

impl ConnectionCompleteness {
    pub(super) fn from_counts(
        loaded_count: usize,
        total_count: i64,
        truncated_reason: Option<String>,
    ) -> Self {
        Self {
            loaded_count,
            total_count,
            is_complete: truncated_reason.is_none() && loaded_count as i64 >= total_count,
            truncated_reason,
        }
    }
}

impl Default for ConnectionCompleteness {
    fn default() -> Self {
        Self {
            loaded_count: 0,
            total_count: 0,
            is_complete: true,
            truncated_reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestDataCompleteness {
    pub comments: ConnectionCompleteness,
    pub labels: ConnectionCompleteness,
    pub reviewers: ConnectionCompleteness,
    pub latest_reviews: ConnectionCompleteness,
    pub review_threads: ConnectionCompleteness,
    pub review_thread_comments: ConnectionCompleteness,
    pub files: ConnectionCompleteness,
    #[serde(default)]
    pub diff: ConnectionCompleteness,
    #[serde(default)]
    pub commits: ConnectionCompleteness,
}

impl PullRequestDataCompleteness {
    pub fn is_complete(&self) -> bool {
        self.comments.is_complete
            && self.labels.is_complete
            && self.reviewers.is_complete
            && self.latest_reviews.is_complete
            && self.review_threads.is_complete
            && self.review_thread_comments.is_complete
            && self.files.is_complete
            && self.diff.is_complete
            && self.commits.is_complete
    }

    pub fn warnings(&self) -> Vec<String> {
        [
            ("comments", &self.comments),
            ("labels", &self.labels),
            ("reviewers", &self.reviewers),
            ("reviews", &self.latest_reviews),
            ("review threads", &self.review_threads),
            ("thread comments", &self.review_thread_comments),
            ("files", &self.files),
            ("diff files", &self.diff),
            ("commits", &self.commits),
        ]
        .into_iter()
        .filter_map(|(label, completeness)| {
            if completeness.is_complete {
                return None;
            }

            Some(match completeness.truncated_reason.as_deref() {
                Some(reason) if !reason.is_empty() => format!(
                    "Loaded {} of {} {label}: {reason}",
                    completeness.loaded_count, completeness.total_count
                ),
                _ => format!(
                    "Loaded {} of {} {label}.",
                    completeness.loaded_count, completeness.total_count
                ),
            })
        })
        .collect()
    }
}

impl Default for PullRequestDataCompleteness {
    fn default() -> Self {
        Self {
            comments: ConnectionCompleteness::default(),
            labels: ConnectionCompleteness::default(),
            reviewers: ConnectionCompleteness::default(),
            latest_reviews: ConnectionCompleteness::default(),
            review_threads: ConnectionCompleteness::default(),
            review_thread_comments: ConnectionCompleteness::default(),
            files: ConnectionCompleteness::default(),
            diff: ConnectionCompleteness::default(),
            commits: ConnectionCompleteness::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestDetail {
    pub id: String,
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub state: String,
    pub is_draft: bool,
    pub review_decision: Option<String>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub base_ref_oid: Option<String>,
    pub head_ref_oid: Option<String>,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub comments_count: i64,
    pub commits_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<String>,
    pub reviewers: Vec<String>,
    #[serde(default)]
    pub reviewer_avatar_urls: BTreeMap<String, String>,
    #[serde(default)]
    pub comments: Vec<PullRequestComment>,
    #[serde(default)]
    pub commits: Vec<PullRequestCommit>,
    pub latest_reviews: Vec<PullRequestReview>,
    pub review_threads: Vec<PullRequestReviewThread>,
    #[serde(default)]
    pub viewer_pending_review: Option<PendingPullRequestReview>,
    pub files: Vec<PullRequestFile>,
    pub raw_diff: String,
    pub parsed_diff: Vec<ParsedDiffFile>,
    #[serde(default)]
    pub data_completeness: PullRequestDataCompleteness,
}

#[derive(Debug, Clone)]
pub struct PullRequestDetailSnapshot {
    pub auth: AuthState,
    pub loaded_from_cache: bool,
    pub fetched_at_ms: Option<i64>,
    pub detail: Option<PullRequestDetail>,
}

#[derive(Debug, Clone)]
pub struct ActionResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReviewAction {
    Approve,
    Comment,
    RequestChanges,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryFileContent {
    pub repository: String,
    pub reference: String,
    pub path: String,
    pub content: Option<String>,
    pub is_binary: bool,
    pub size_bytes: usize,
    #[serde(default = "default_repository_file_source")]
    pub source: String,
}

pub const REPOSITORY_FILE_SOURCE_GITHUB: &str = "github";
pub const REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT: &str = "local-checkout";

fn default_repository_file_source() -> String {
    REPOSITORY_FILE_SOURCE_GITHUB.to_string()
}

fn default_change_type() -> String {
    "MODIFIED".to_string()
}

fn default_author_association() -> String {
    "NONE".to_string()
}

fn default_true() -> bool {
    true
}
