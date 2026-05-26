use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::{
    app_storage,
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffLine},
    github::{PullRequestDetail, PullRequestReviewComment, PullRequestReviewThread},
    review_ai::DiffAnchor,
};

pub const FEEDBACK_SCHEMA: &str = "remiss.feedback.v1";
const INDEX_SCHEMA: &str = "remiss.feedback.index.v1";
const CURRENT_FILE: &str = "current.json";
const ARCHIVE_DIR: &str = "archive";
const CONTEXT_RADIUS: usize = 3;

#[derive(Clone, Debug)]
pub struct LocalFeedbackStore {
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalFeedbackTarget {
    pub anchor: DiffAnchor,
    pub start_line: Option<i64>,
    pub start_side: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LocalFeedbackThreads {
    pub threads: Vec<PullRequestReviewThread>,
    pub pending_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalFeedbackIndex {
    pub schema: String,
    #[serde(default)]
    pub repos: Vec<LocalFeedbackIndexEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalFeedbackIndexEntry {
    pub repo_root: String,
    pub repo_root_hash: String,
    pub repository: String,
    pub current_path: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalFeedbackStatus {
    Pending,
    Resolved,
    Stale,
}

impl Default for LocalFeedbackStatus {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalFeedbackInstructions {
    pub mode: String,
    pub summary_required: bool,
    pub tests_required_if_relevant: bool,
}

impl Default for LocalFeedbackInstructions {
    fn default() -> Self {
        Self {
            mode: "apply_minimal_changes".to_string(),
            summary_required: true,
            tests_required_if_relevant: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalFeedbackDocument {
    pub schema: String,
    pub feedback_id: String,
    pub repo_root: String,
    pub repo_root_hash: String,
    pub repository: String,
    pub local_review_key: String,
    pub base_ref: String,
    pub head_ref: String,
    #[serde(default)]
    pub base_oid: Option<String>,
    #[serde(default)]
    pub head_oid: Option<String>,
    #[serde(default)]
    pub worktree_identity: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub status: LocalFeedbackStatus,
    pub instructions: LocalFeedbackInstructions,
    #[serde(default)]
    pub comments: Vec<LocalFeedbackComment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalFeedbackComment {
    pub id: String,
    pub thread_id: String,
    pub file: String,
    #[serde(default)]
    pub old_line_start: Option<i64>,
    #[serde(default)]
    pub old_line_end: Option<i64>,
    #[serde(default)]
    pub new_line_start: Option<i64>,
    #[serde(default)]
    pub new_line_end: Option<i64>,
    #[serde(default)]
    pub side: Option<String>,
    pub severity: String,
    pub status: LocalFeedbackStatus,
    pub body: String,
    #[serde(default)]
    pub hunk_header: Option<String>,
    #[serde(default)]
    pub selected_text: Option<String>,
    #[serde(default)]
    pub before_context: Vec<String>,
    #[serde(default)]
    pub after_context: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Default)]
struct FeedbackContext {
    hunk_header: Option<String>,
    selected_text: Option<String>,
    before_context: Vec<String>,
    after_context: Vec<String>,
    old_line_start: Option<i64>,
    old_line_end: Option<i64>,
    new_line_start: Option<i64>,
    new_line_end: Option<i64>,
}

impl LocalFeedbackStore {
    pub fn new() -> Self {
        Self {
            root: app_storage::local_feedback_root(),
        }
    }

    #[cfg(test)]
    fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn sync_detail_threads(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
    ) -> Result<LocalFeedbackThreads, String> {
        let mut current = self.ensure_current_document(repo_root, detail)?;
        current.comments.sort_by(|a, b| a.id.cmp(&b.id));
        self.write_current_document(repo_root, &current)?;
        self.update_index(repo_root, detail)?;

        let mut threads = comments_to_threads(&current.comments, detail, false);
        for stale in self.load_stale_documents(repo_root, detail)? {
            threads.extend(comments_to_threads(&stale.comments, detail, true));
        }
        let pending_count = current
            .comments
            .iter()
            .filter(|comment| comment.status == LocalFeedbackStatus::Pending)
            .count();
        Ok(LocalFeedbackThreads {
            threads,
            pending_count,
        })
    }

    pub fn add_comment(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
        target: &LocalFeedbackTarget,
        body: &str,
    ) -> Result<LocalFeedbackThreads, String> {
        let mut document = self.ensure_current_document(repo_root, detail)?;
        let now = now_rfc3339();
        let context = feedback_context(detail, target);
        let comment_id = next_comment_id(repo_root, detail, body);
        document.comments.push(LocalFeedbackComment {
            thread_id: format!("local-thread-{comment_id}"),
            id: comment_id,
            file: target.anchor.file_path.clone(),
            old_line_start: context.old_line_start,
            old_line_end: context.old_line_end,
            new_line_start: context.new_line_start,
            new_line_end: context.new_line_end,
            side: target.anchor.side.clone(),
            severity: "normal".to_string(),
            status: LocalFeedbackStatus::Pending,
            body: body.trim().to_string(),
            hunk_header: context.hunk_header,
            selected_text: context.selected_text,
            before_context: context.before_context,
            after_context: context.after_context,
            created_at: now.clone(),
            updated_at: now,
        });
        self.save_document_update(repo_root, detail, document)
    }

    pub fn update_comment(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
        comment_id: &str,
        body: &str,
    ) -> Result<LocalFeedbackThreads, String> {
        let mut document = self.ensure_current_document(repo_root, detail)?;
        let Some(comment) = document
            .comments
            .iter_mut()
            .find(|comment| comment.id == comment_id)
        else {
            return Err("Local feedback comment was not found.".to_string());
        };
        comment.body = body.trim().to_string();
        comment.updated_at = now_rfc3339();
        self.save_document_update(repo_root, detail, document)
    }

    pub fn delete_comment(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
        comment_id: &str,
    ) -> Result<LocalFeedbackThreads, String> {
        let mut document = self.ensure_current_document(repo_root, detail)?;
        let original_len = document.comments.len();
        document.comments.retain(|comment| comment.id != comment_id);
        if document.comments.len() == original_len {
            return Err("Local feedback comment was not found.".to_string());
        }
        self.save_document_update(repo_root, detail, document)
    }

    pub fn set_thread_resolved(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
        thread_id: &str,
        resolved: bool,
    ) -> Result<LocalFeedbackThreads, String> {
        let mut document = self.ensure_current_document(repo_root, detail)?;
        let Some(comment) = document
            .comments
            .iter_mut()
            .find(|comment| comment.thread_id == thread_id)
        else {
            return Err("Local feedback thread was not found.".to_string());
        };
        comment.status = if resolved {
            LocalFeedbackStatus::Resolved
        } else {
            LocalFeedbackStatus::Pending
        };
        comment.updated_at = now_rfc3339();
        self.save_document_update(repo_root, detail, document)
    }

    fn save_document_update(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
        mut document: LocalFeedbackDocument,
    ) -> Result<LocalFeedbackThreads, String> {
        document.updated_at = now_rfc3339();
        document.status = if document
            .comments
            .iter()
            .any(|comment| comment.status == LocalFeedbackStatus::Pending)
        {
            LocalFeedbackStatus::Pending
        } else {
            LocalFeedbackStatus::Resolved
        };
        self.write_current_document(repo_root, &document)?;
        self.update_index(repo_root, detail)?;
        self.sync_detail_threads(repo_root, detail)
    }

    fn ensure_current_document(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
    ) -> Result<LocalFeedbackDocument, String> {
        let current = self.read_current_document(repo_root)?;
        let mut document = match current {
            Some(mut document) if document_matches_review_scope(&document, repo_root, detail) => {
                refresh_document_snapshot(&mut document, repo_root, detail);
                document
            }
            Some(document) => {
                if !document.comments.is_empty() {
                    self.archive_document(repo_root, document)?;
                }
                new_document(repo_root, detail)
            }
            None => new_document(repo_root, detail),
        };
        self.recover_same_scope_archive_comments(repo_root, detail, &mut document)?;
        Ok(document)
    }

    fn read_current_document(
        &self,
        repo_root: &Path,
    ) -> Result<Option<LocalFeedbackDocument>, String> {
        let path = self.current_document_path(repo_root);
        if !path.exists() {
            return Ok(None);
        }
        read_json_file(&path)
    }

    fn write_current_document(
        &self,
        repo_root: &Path,
        document: &LocalFeedbackDocument,
    ) -> Result<(), String> {
        write_json_file(&self.current_document_path(repo_root), document)
    }

    fn archive_document(
        &self,
        repo_root: &Path,
        mut document: LocalFeedbackDocument,
    ) -> Result<(), String> {
        for comment in &mut document.comments {
            if comment.status == LocalFeedbackStatus::Pending {
                comment.status = LocalFeedbackStatus::Stale;
            }
        }
        document.status = LocalFeedbackStatus::Stale;
        document.updated_at = now_rfc3339();
        let path = self
            .repo_dir(repo_root)
            .join(ARCHIVE_DIR)
            .join(format!("{}.json", document.feedback_id));
        write_json_file(&path, &document)
    }

    fn load_stale_documents(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
    ) -> Result<Vec<LocalFeedbackDocument>, String> {
        let archive_dir = self.repo_dir(repo_root).join(ARCHIVE_DIR);
        let Ok(entries) = fs::read_dir(&archive_dir) else {
            return Ok(Vec::new());
        };

        let mut documents = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "Failed to read local feedback archive '{}': {error}",
                    archive_dir.display()
                )
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(document) = read_json_file::<LocalFeedbackDocument>(&path)? else {
                continue;
            };
            if !document_matches_review_scope(&document, repo_root, detail)
                && document.comments.iter().any(|comment| {
                    comment.status == LocalFeedbackStatus::Pending
                        || comment.status == LocalFeedbackStatus::Stale
                })
            {
                documents.push(document);
            }
        }
        documents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        documents.truncate(5);
        Ok(documents)
    }

    fn recover_same_scope_archive_comments(
        &self,
        repo_root: &Path,
        detail: &PullRequestDetail,
        document: &mut LocalFeedbackDocument,
    ) -> Result<(), String> {
        let archive_dir = self.repo_dir(repo_root).join(ARCHIVE_DIR);
        let Ok(entries) = fs::read_dir(&archive_dir) else {
            return Ok(());
        };

        let mut seen_ids = document
            .comments
            .iter()
            .map(|comment| comment.id.clone())
            .collect::<BTreeSet<_>>();
        let mut recovered = false;
        let mut recovered_paths = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "Failed to read local feedback archive '{}': {error}",
                    archive_dir.display()
                )
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(archived) = read_json_file::<LocalFeedbackDocument>(&path)? else {
                continue;
            };
            if !document_matches_review_scope(&archived, repo_root, detail) {
                continue;
            }

            for mut comment in archived.comments {
                if !seen_ids.insert(comment.id.clone()) {
                    continue;
                }
                if comment.status == LocalFeedbackStatus::Stale {
                    comment.status = LocalFeedbackStatus::Pending;
                }
                document.comments.push(comment);
                recovered = true;
            }
            recovered_paths.push(path);
        }

        if recovered {
            document.status = if document
                .comments
                .iter()
                .any(|comment| comment.status == LocalFeedbackStatus::Pending)
            {
                LocalFeedbackStatus::Pending
            } else {
                LocalFeedbackStatus::Resolved
            };
            document.updated_at = now_rfc3339();
        }

        for path in recovered_paths {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "Failed to remove recovered local feedback archive '{}': {error}",
                    path.display()
                )
            })?;
        }

        Ok(())
    }

    fn update_index(&self, repo_root: &Path, detail: &PullRequestDetail) -> Result<(), String> {
        let index_path = self.root.join("index.json");
        let mut index = read_json_file::<LocalFeedbackIndex>(&index_path)?.unwrap_or_else(|| {
            LocalFeedbackIndex {
                schema: INDEX_SCHEMA.to_string(),
                repos: Vec::new(),
            }
        });
        index.schema = INDEX_SCHEMA.to_string();
        let root_text = repo_root.display().to_string();
        let repo_hash = repo_root_hash(repo_root);
        index
            .repos
            .retain(|entry| entry.repo_root_hash != repo_hash);
        index.repos.insert(
            0,
            LocalFeedbackIndexEntry {
                repo_root: root_text,
                repo_root_hash: repo_hash,
                repository: detail.repository.clone(),
                current_path: self.current_document_path(repo_root).display().to_string(),
                updated_at: now_rfc3339(),
            },
        );
        write_json_file(&index_path, &index)
    }

    fn repo_dir(&self, repo_root: &Path) -> PathBuf {
        self.root.join("repos").join(repo_root_hash(repo_root))
    }

    fn current_document_path(&self, repo_root: &Path) -> PathBuf {
        self.repo_dir(repo_root).join(CURRENT_FILE)
    }
}

pub fn sync_detail_threads(
    repo_root: &Path,
    detail: &PullRequestDetail,
) -> Result<LocalFeedbackThreads, String> {
    LocalFeedbackStore::new().sync_detail_threads(repo_root, detail)
}

pub fn add_comment(
    repo_root: &Path,
    detail: &PullRequestDetail,
    target: &LocalFeedbackTarget,
    body: &str,
) -> Result<LocalFeedbackThreads, String> {
    LocalFeedbackStore::new().add_comment(repo_root, detail, target, body)
}

pub fn update_comment(
    repo_root: &Path,
    detail: &PullRequestDetail,
    comment_id: &str,
    body: &str,
) -> Result<LocalFeedbackThreads, String> {
    LocalFeedbackStore::new().update_comment(repo_root, detail, comment_id, body)
}

pub fn delete_comment(
    repo_root: &Path,
    detail: &PullRequestDetail,
    comment_id: &str,
) -> Result<LocalFeedbackThreads, String> {
    LocalFeedbackStore::new().delete_comment(repo_root, detail, comment_id)
}

pub fn set_thread_resolved(
    repo_root: &Path,
    detail: &PullRequestDetail,
    thread_id: &str,
    resolved: bool,
) -> Result<LocalFeedbackThreads, String> {
    LocalFeedbackStore::new().set_thread_resolved(repo_root, detail, thread_id, resolved)
}

fn new_document(repo_root: &Path, detail: &PullRequestDetail) -> LocalFeedbackDocument {
    let now = now_rfc3339();
    LocalFeedbackDocument {
        schema: FEEDBACK_SCHEMA.to_string(),
        feedback_id: next_feedback_id(repo_root, detail),
        repo_root: repo_root.display().to_string(),
        repo_root_hash: repo_root_hash(repo_root),
        repository: detail.repository.clone(),
        local_review_key: detail.id.clone(),
        base_ref: detail.base_ref_name.clone(),
        head_ref: detail.head_ref_name.clone(),
        base_oid: detail.base_ref_oid.clone(),
        head_oid: detail.head_ref_oid.clone(),
        worktree_identity: worktree_identity(detail),
        created_at: now.clone(),
        updated_at: now,
        status: LocalFeedbackStatus::Pending,
        instructions: LocalFeedbackInstructions::default(),
        comments: Vec::new(),
    }
}

fn document_matches_review_scope(
    document: &LocalFeedbackDocument,
    repo_root: &Path,
    detail: &PullRequestDetail,
) -> bool {
    document.repo_root == repo_root.display().to_string()
        && document.repository == detail.repository
        && document.base_ref == detail.base_ref_name
        && document.head_ref == detail.head_ref_name
        && document.base_oid == detail.base_ref_oid
}

fn refresh_document_snapshot(
    document: &mut LocalFeedbackDocument,
    repo_root: &Path,
    detail: &PullRequestDetail,
) {
    document.schema = FEEDBACK_SCHEMA.to_string();
    document.repo_root = repo_root.display().to_string();
    document.repo_root_hash = repo_root_hash(repo_root);
    document.repository = detail.repository.clone();
    document.local_review_key = detail.id.clone();
    document.base_ref = detail.base_ref_name.clone();
    document.head_ref = detail.head_ref_name.clone();
    document.base_oid = detail.base_ref_oid.clone();
    document.head_oid = detail.head_ref_oid.clone();
    document.worktree_identity = worktree_identity(detail);
}

fn comments_to_threads(
    comments: &[LocalFeedbackComment],
    detail: &PullRequestDetail,
    stale: bool,
) -> Vec<PullRequestReviewThread> {
    comments
        .iter()
        .filter(|comment| comment.status != LocalFeedbackStatus::Resolved || !stale)
        .map(|comment| {
            let is_stale = stale || comment.status == LocalFeedbackStatus::Stale;
            let is_resolved = comment.status == LocalFeedbackStatus::Resolved;
            let line = comment.new_line_end;
            let original_line = comment.old_line_end;
            PullRequestReviewThread {
                id: comment.thread_id.clone(),
                path: comment.file.clone(),
                line,
                original_line,
                start_line: comment
                    .new_line_start
                    .filter(|start| Some(*start) != comment.new_line_end),
                original_start_line: comment
                    .old_line_start
                    .filter(|start| Some(*start) != comment.old_line_end),
                diff_side: comment.side.clone().unwrap_or_else(|| "RIGHT".to_string()),
                start_diff_side: comment
                    .new_line_start
                    .or(comment.old_line_start)
                    .map(|_| comment.side.clone().unwrap_or_else(|| "RIGHT".to_string())),
                is_collapsed: false,
                is_outdated: is_stale,
                is_resolved,
                subject_type: "LINE".to_string(),
                resolved_by_login: is_resolved.then(|| detail.author_login.clone()),
                viewer_can_reply: false,
                viewer_can_resolve: !is_resolved && !is_stale,
                viewer_can_unresolve: is_resolved && !is_stale,
                comments: vec![PullRequestReviewComment {
                    id: comment.id.clone(),
                    author_login: detail.author_login.clone(),
                    author_avatar_url: detail.author_avatar_url.clone(),
                    body: comment.body.clone(),
                    path: comment.file.clone(),
                    line,
                    original_line,
                    start_line: comment
                        .new_line_start
                        .filter(|start| Some(*start) != comment.new_line_end),
                    original_start_line: comment
                        .old_line_start
                        .filter(|start| Some(*start) != comment.old_line_end),
                    state: if is_stale {
                        "STALE".to_string()
                    } else if is_resolved {
                        "RESOLVED".to_string()
                    } else {
                        "PENDING".to_string()
                    },
                    created_at: comment.created_at.clone(),
                    updated_at: comment.updated_at.clone(),
                    published_at: Some(comment.updated_at.clone()),
                    reply_to_id: None,
                    viewer_can_update: !is_resolved && !is_stale,
                    viewer_can_delete: !is_stale,
                    url: String::new(),
                }],
            }
        })
        .collect()
}

fn feedback_context(detail: &PullRequestDetail, target: &LocalFeedbackTarget) -> FeedbackContext {
    let Some(line) = target.anchor.line else {
        return FeedbackContext::default();
    };
    let side = target.anchor.side.as_deref().unwrap_or("RIGHT");
    let start = target.start_line.unwrap_or(line).min(line);
    let end = target.start_line.unwrap_or(line).max(line);
    let mut context = FeedbackContext {
        hunk_header: target.anchor.hunk_header.clone(),
        ..FeedbackContext::default()
    };
    if side == "LEFT" {
        context.old_line_start = Some(start);
        context.old_line_end = Some(end);
    } else {
        context.new_line_start = Some(start);
        context.new_line_end = Some(end);
    }

    let Some(file) = detail
        .parsed_diff
        .iter()
        .find(|file| file.path == target.anchor.file_path)
    else {
        return context;
    };

    if let Some((hunk_header, selected, before, after)) =
        hunk_context_for_range(file, side, start, end)
    {
        context.hunk_header = Some(hunk_header);
        context.selected_text = (!selected.is_empty()).then(|| selected.join("\n"));
        context.before_context = before;
        context.after_context = after;
    }
    context
}

fn hunk_context_for_range(
    file: &ParsedDiffFile,
    side: &str,
    start: i64,
    end: i64,
) -> Option<(String, Vec<String>, Vec<String>, Vec<String>)> {
    for hunk in &file.hunks {
        let matched_indices = hunk
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                line_number_for_side(line, side)
                    .filter(|line_number| (start..=end).contains(line_number))
                    .map(|_| index)
            })
            .collect::<Vec<_>>();
        if matched_indices.is_empty() {
            continue;
        }

        let first = *matched_indices.first()?;
        let last = *matched_indices.last()?;
        let selected = matched_indices
            .iter()
            .map(|index| hunk.lines[*index].content.clone())
            .collect::<Vec<_>>();
        let before = hunk.lines[..first]
            .iter()
            .rev()
            .filter(|line| line.kind != DiffLineKind::Meta)
            .take(CONTEXT_RADIUS)
            .map(|line| line.content.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        let after = hunk.lines[last.saturating_add(1)..]
            .iter()
            .filter(|line| line.kind != DiffLineKind::Meta)
            .take(CONTEXT_RADIUS)
            .map(|line| line.content.clone())
            .collect::<Vec<_>>();
        return Some((hunk.header.clone(), selected, before, after));
    }
    None
}

fn line_number_for_side(line: &ParsedDiffLine, side: &str) -> Option<i64> {
    match side {
        "LEFT" => line.left_line_number,
        _ => line.right_line_number,
    }
}

fn worktree_identity(detail: &PullRequestDetail) -> Option<String> {
    let head_oid = detail.head_ref_oid.as_deref()?;
    let marker = format!("{head_oid}:");
    let suffix = detail.id.rsplit_once(&marker)?.1;
    (!suffix.is_empty() && suffix != head_oid).then(|| suffix.to_string())
}

fn next_feedback_id(repo_root: &Path, detail: &PullRequestDetail) -> String {
    format!(
        "fb_{}_{}",
        now_ms(),
        short_hash(&format!("{}:{}", repo_root.display(), detail.id))
    )
}

fn next_comment_id(repo_root: &Path, detail: &PullRequestDetail, body: &str) -> String {
    format!(
        "cmt_{}_{}",
        now_ms(),
        short_hash(&format!("{}:{}:{body}", repo_root.display(), detail.id))
    )
}

fn repo_root_hash(repo_root: &Path) -> String {
    short_hash(&repo_root.display().to_string())
}

fn short_hash(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
        .chars()
        .take(16)
        .collect()
}

fn read_json_file<T>(path: &Path) -> Result<Option<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read '{}': {error}", path.display()))?;
    let value = serde_json::from_str(&text)
        .map_err(|error| format!("Failed to parse '{}': {error}", path.display()))?;
    Ok(Some(value))
}

fn write_json_file<T>(path: &Path, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize '{}': {error}", path.display()))?;
    fs::write(path, json).map_err(|error| format!("Failed to write '{}': {error}", path.display()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn now_rfc3339() -> String {
    rfc3339_from_unix_seconds((now_ms() / 1000).max(0))
}

fn rfc3339_from_unix_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diff::parse_unified_diff,
        github::{AuthState, PullRequestDataCompleteness, PullRequestFile, PullRequestSummary},
    };
    use std::collections::BTreeMap;

    #[test]
    fn serializes_feedback_document_with_snapshot_fields() {
        let repo_root = PathBuf::from("/tmp/app");
        let detail = detail_with_diff("local:org/repo:feature:base:head:worktree-abcd");
        let document = new_document(&repo_root, &detail);
        let json = serde_json::to_value(&document).expect("serialize document");

        assert_eq!(json["schema"], FEEDBACK_SCHEMA);
        assert_eq!(json["repoRoot"], "/tmp/app");
        assert_eq!(json["repository"], "org/repo");
        assert_eq!(json["localReviewKey"], detail.id);
        assert_eq!(json["headRef"], "feature");
        assert_eq!(json["worktreeIdentity"], "worktree-abcd");
        assert_eq!(json["instructions"]["mode"], "apply_minimal_changes");
    }

    #[test]
    fn add_comment_writes_current_and_index() {
        let storage = crate::test_git::unique_test_directory("local-feedback-store");
        let repo_root = storage.join("repo");
        fs::create_dir_all(&repo_root).expect("repo dir");
        let store = LocalFeedbackStore::with_root(storage.join("feedback"));
        let detail = detail_with_diff("local:org/repo:feature:base:head");
        let target = target("src/lib.rs", "RIGHT", 1, None);

        let threads = store
            .add_comment(&repo_root, &detail, &target, "Please keep this minimal.")
            .expect("add comment");

        assert_eq!(threads.pending_count, 1);
        assert_eq!(threads.threads.len(), 1);
        let current = store
            .read_current_document(&repo_root)
            .expect("read current")
            .expect("current document");
        assert_eq!(current.comments.len(), 1);
        assert_eq!(current.comments[0].new_line_start, Some(1));
        assert_eq!(
            current.comments[0].selected_text.as_deref(),
            Some("new line")
        );
        let index: LocalFeedbackIndex = read_json_file(&store.root.join("index.json"))
            .expect("read index")
            .expect("index");
        assert_eq!(
            index.repos[0].current_path,
            store
                .current_document_path(&repo_root)
                .display()
                .to_string()
        );
    }

    #[test]
    fn worktree_snapshot_change_keeps_active_comments_in_current() {
        let storage = crate::test_git::unique_test_directory("local-feedback-current-refresh");
        let repo_root = storage.join("repo");
        fs::create_dir_all(&repo_root).expect("repo dir");
        let store = LocalFeedbackStore::with_root(storage.join("feedback"));
        let old_detail = detail_with_diff("local:org/repo:feature:base:head");
        let new_detail = detail_with_diff("local:org/repo:feature:base:newhead");

        store
            .add_comment(
                &repo_root,
                &old_detail,
                &target("src/lib.rs", "RIGHT", 2, None),
                "Fix this.",
            )
            .expect("add old comment");
        let threads = store
            .sync_detail_threads(&repo_root, &new_detail)
            .expect("sync new snapshot");

        assert_eq!(threads.pending_count, 1);
        assert_eq!(threads.threads.len(), 1);
        assert!(!threads.threads[0].is_outdated);
        let current = store
            .read_current_document(&repo_root)
            .expect("read current")
            .expect("current document");
        assert_eq!(current.comments.len(), 1);
        assert_eq!(current.head_oid.as_deref(), Some("newhead"));
        assert_eq!(current.local_review_key, new_detail.id);
    }

    #[test]
    fn base_change_archives_current_and_marks_stale_thread() {
        let storage = crate::test_git::unique_test_directory("local-feedback-stale");
        let repo_root = storage.join("repo");
        fs::create_dir_all(&repo_root).expect("repo dir");
        let store = LocalFeedbackStore::with_root(storage.join("feedback"));
        let old_detail = detail_with_diff("local:org/repo:feature:base:head");
        let new_detail = detail_with_diff("local:org/repo:feature:newbase:newhead");

        store
            .add_comment(
                &repo_root,
                &old_detail,
                &target("src/lib.rs", "RIGHT", 2, None),
                "Fix this.",
            )
            .expect("add old comment");
        let threads = store
            .sync_detail_threads(&repo_root, &new_detail)
            .expect("sync new base snapshot");

        assert_eq!(threads.pending_count, 0);
        assert_eq!(threads.threads.len(), 1);
        assert!(threads.threads[0].is_outdated);
        let current = store
            .read_current_document(&repo_root)
            .expect("read current")
            .expect("current document");
        assert!(current.comments.is_empty());
    }

    #[test]
    fn same_scope_archived_comment_is_recovered_as_pending_current() {
        let storage = crate::test_git::unique_test_directory("local-feedback-recover");
        let repo_root = storage.join("repo");
        fs::create_dir_all(&repo_root).expect("repo dir");
        let store = LocalFeedbackStore::with_root(storage.join("feedback"));
        let old_detail = detail_with_diff("local:org/repo:feature:base:head");
        let new_detail = detail_with_diff("local:org/repo:feature:base:newhead");

        store
            .add_comment(
                &repo_root,
                &old_detail,
                &target("src/lib.rs", "RIGHT", 2, None),
                "Fix this.",
            )
            .expect("add old comment");
        let old_current = store
            .read_current_document(&repo_root)
            .expect("read current")
            .expect("current document");
        store
            .archive_document(&repo_root, old_current)
            .expect("archive old current");
        store
            .write_current_document(&repo_root, &new_document(&repo_root, &new_detail))
            .expect("write empty current");

        let threads = store
            .sync_detail_threads(&repo_root, &new_detail)
            .expect("sync recovered snapshot");

        assert_eq!(threads.pending_count, 1);
        assert_eq!(threads.threads.len(), 1);
        assert!(!threads.threads[0].is_outdated);
        let current = store
            .read_current_document(&repo_root)
            .expect("read current")
            .expect("current document");
        assert_eq!(current.comments.len(), 1);
        assert_eq!(current.comments[0].status, LocalFeedbackStatus::Pending);
    }

    #[test]
    fn resolve_excludes_comment_from_pending_count_but_keeps_thread_visible() {
        let storage = crate::test_git::unique_test_directory("local-feedback-resolve");
        let repo_root = storage.join("repo");
        fs::create_dir_all(&repo_root).expect("repo dir");
        let store = LocalFeedbackStore::with_root(storage.join("feedback"));
        let detail = detail_with_diff("local:org/repo:feature:base:head");
        let added = store
            .add_comment(
                &repo_root,
                &detail,
                &target("src/lib.rs", "RIGHT", 2, None),
                "Fix this.",
            )
            .expect("add comment");
        let thread_id = added.threads[0].id.clone();

        let threads = store
            .set_thread_resolved(&repo_root, &detail, &thread_id, true)
            .expect("resolve");

        assert_eq!(threads.pending_count, 0);
        assert_eq!(threads.threads.len(), 1);
        assert!(threads.threads[0].is_resolved);
    }

    #[test]
    fn range_comment_captures_selected_text_and_context() {
        let detail = detail_with_diff("local:org/repo:feature:base:head");
        let context = feedback_context(&detail, &target("src/lib.rs", "RIGHT", 2, Some(1)));

        assert_eq!(context.new_line_start, Some(1));
        assert_eq!(context.new_line_end, Some(2));
        assert_eq!(
            context.selected_text.as_deref(),
            Some("new line\nanother line")
        );
        assert_eq!(context.before_context, vec!["old line"]);
    }

    fn target(
        file_path: &str,
        side: &str,
        line: i64,
        start_line: Option<i64>,
    ) -> LocalFeedbackTarget {
        LocalFeedbackTarget {
            anchor: DiffAnchor {
                file_path: file_path.to_string(),
                hunk_header: None,
                line: Some(line),
                side: Some(side.to_string()),
                thread_id: None,
            },
            start_line,
            start_side: start_line.map(|_| side.to_string()),
        }
    }

    fn detail_with_diff(id: &str) -> PullRequestDetail {
        let raw_diff = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
-old line
+new line
+another line
 unchanged
";
        let parsed_diff = parse_unified_diff(raw_diff);
        PullRequestDetail {
            id: id.to_string(),
            repository: "org/repo".to_string(),
            number: 0,
            title: "Local review".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "me".to_string(),
            author_avatar_url: None,
            state: "LOCAL".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: Some(if id.contains("newbase") {
                "newbase".to_string()
            } else {
                "base".to_string()
            }),
            head_ref_oid: Some(if id.contains("newhead") {
                "newhead".to_string()
            } else {
                "head".to_string()
            }),
            additions: 2,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            commits: Vec::new(),
            created_at: "created".to_string(),
            updated_at: "updated".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            viewer_pending_review: None,
            files: vec![PullRequestFile {
                path: "src/lib.rs".to_string(),
                additions: 2,
                deletions: 1,
                change_type: "MODIFIED".to_string(),
            }],
            raw_diff: raw_diff.to_string(),
            parsed_diff,
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }

    #[allow(dead_code)]
    fn _summary_for_detail(detail: &PullRequestDetail) -> PullRequestSummary {
        PullRequestSummary {
            local_key: Some(detail.id.clone()),
            repository: detail.repository.clone(),
            number: 0,
            title: detail.title.clone(),
            author_login: detail.author_login.clone(),
            author_avatar_url: None,
            is_draft: false,
            comments_count: detail.comments_count,
            additions: detail.additions,
            deletions: detail.deletions,
            changed_files: detail.changed_files,
            state: detail.state.clone(),
            author_association: "OWNER".to_string(),
            review_decision: None,
            updated_at: detail.updated_at.clone(),
            url: String::new(),
            repository_default_branch: None,
            triage_signals: Vec::new(),
        }
    }

    #[allow(dead_code)]
    fn _snapshot_for_detail(detail: PullRequestDetail) -> crate::github::PullRequestDetailSnapshot {
        crate::github::PullRequestDetailSnapshot {
            auth: AuthState {
                is_authenticated: false,
                active_login: None,
                active_hostname: None,
                message: "Local review".to_string(),
            },
            loaded_from_cache: false,
            fetched_at_ms: None,
            detail: Some(detail),
        }
    }
}
