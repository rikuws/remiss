use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    agents::{self, jsonrepair::parse_tolerant},
    cache::CacheStore,
    code_tour::{
        tour_code_version_key, CodeTourProvider, CodeTourReviewCommentContext,
        CodeTourReviewContext, CodeTourReviewThreadContext,
    },
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine},
    github::{PullRequestDetail, PullRequestFile, PullRequestReview, PullRequestReviewThread},
};

const REVIEW_BRIEF_CACHE_KEY_PREFIX: &str = "review-brief-v1";
const MAX_BODY_CHARS: usize = 2_500;
const MAX_RAW_DIFF_CHARS: usize = 48_000;
const MAX_FILES: usize = 80;
const MAX_PARSED_DIFF_FILES: usize = 40;
const MAX_HUNKS_PER_FILE: usize = 8;
const MAX_LINES_PER_HUNK: usize = 28;
const MAX_REVIEWS: usize = 5;
const MAX_THREADS: usize = 12;
const MAX_COMMENTS_PER_THREAD: usize = 3;
const MAX_REVIEW_BODY_CHARS: usize = 900;
const MAX_COMMENT_BODY_CHARS: usize = 500;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewBriefConfidence {
    Low,
    #[default]
    Medium,
    High,
}

impl ReviewBriefConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low confidence",
            Self::Medium => "Medium confidence",
            Self::High => "High confidence",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBrief {
    pub provider: CodeTourProvider,
    pub generated_at_ms: i64,
    pub code_version_key: String,
    pub confidence: ReviewBriefConfidence,
    pub likely_intent: String,
    pub changed_summary: Vec<String>,
    pub risks_questions: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub related_file_paths: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBriefFileContext {
    pub path: String,
    pub change_type: String,
    pub additions: i64,
    pub deletions: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateReviewBriefInput {
    pub provider: CodeTourProvider,
    pub working_directory: String,
    pub repository: String,
    pub number: i64,
    pub code_version_key: String,
    pub title: String,
    pub author_body: String,
    pub url: String,
    pub author_login: String,
    pub review_decision: Option<String>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub head_ref_oid: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<String>,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub commits_count: i64,
    pub files: Vec<ReviewBriefFileContext>,
    pub raw_diff: String,
    pub parsed_diff: Vec<ParsedDiffFile>,
    pub latest_reviews: Vec<CodeTourReviewContext>,
    pub review_threads: Vec<CodeTourReviewThreadContext>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewBriefResponse {
    confidence: ReviewBriefConfidence,
    likely_intent: String,
    changed_summary: Vec<String>,
    risks_questions: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    related_file_paths: Vec<String>,
}

pub fn load_review_brief(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> Result<Option<ReviewBrief>, String> {
    let cache_key = review_brief_cache_key(detail, provider);
    Ok(cache
        .get::<ReviewBrief>(&cache_key)?
        .map(|document| document.value))
}

pub fn generate_review_brief(
    cache: &CacheStore,
    input: GenerateReviewBriefInput,
) -> Result<ReviewBrief, String> {
    if input.working_directory.trim().is_empty() {
        return Err("Review brief generation requires a local checkout path.".to_string());
    }

    if !Path::new(&input.working_directory).exists() {
        return Err(format!(
            "The local checkout path '{}' does not exist.",
            input.working_directory
        ));
    }

    if input.files.is_empty() && input.raw_diff.trim().is_empty() {
        return Err("Review brief generation needs pull request files or a raw diff.".to_string());
    }

    let prompt = build_review_brief_prompt(&input);
    let response = agents::run_json_prompt(input.provider, &input.working_directory, prompt)?;
    let parsed = parse_tolerant::<ReviewBriefResponse>(&response.text)
        .map_err(|error| format!("Failed to parse review brief JSON: {}", error.message))?;
    let brief = merge_review_brief(parsed, &input, response.model)?;

    let cache_key = review_brief_cache_key_from_parts(
        &input.repository,
        input.number,
        input.provider,
        &input.code_version_key,
    );
    cache.put(&cache_key, &brief, now_ms())?;

    Ok(brief)
}

pub fn build_review_brief_generation_input(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
    working_directory: &str,
) -> GenerateReviewBriefInput {
    GenerateReviewBriefInput {
        provider,
        working_directory: working_directory.to_string(),
        repository: detail.repository.clone(),
        number: detail.number,
        code_version_key: tour_code_version_key(detail),
        title: detail.title.clone(),
        author_body: trim_text(&detail.body, MAX_BODY_CHARS),
        url: detail.url.clone(),
        author_login: detail.author_login.clone(),
        review_decision: detail.review_decision.clone(),
        base_ref_name: detail.base_ref_name.clone(),
        head_ref_name: detail.head_ref_name.clone(),
        head_ref_oid: detail.head_ref_oid.clone(),
        created_at: detail.created_at.clone(),
        updated_at: detail.updated_at.clone(),
        labels: detail.labels.clone(),
        additions: detail.additions,
        deletions: detail.deletions,
        changed_files: detail.changed_files,
        commits_count: detail.commits_count,
        files: detail.files.iter().map(map_file_context).collect(),
        raw_diff: trim_text(&detail.raw_diff, MAX_RAW_DIFF_CHARS),
        parsed_diff: detail.parsed_diff.clone(),
        latest_reviews: detail
            .latest_reviews
            .iter()
            .take(MAX_REVIEWS)
            .map(map_review_context)
            .collect(),
        review_threads: prioritize_review_threads(&detail.review_threads)
            .into_iter()
            .take(MAX_THREADS)
            .map(|thread| map_thread_context(&thread))
            .collect(),
    }
}

pub fn build_review_brief_request_key(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> String {
    format!(
        "{}:{}#{}:{}",
        provider.slug(),
        detail.repository,
        detail.number,
        tour_code_version_key(detail)
    )
}

pub fn review_brief_cache_key(detail: &PullRequestDetail, provider: CodeTourProvider) -> String {
    review_brief_cache_key_from_parts(
        &detail.repository,
        detail.number,
        provider,
        &tour_code_version_key(detail),
    )
}

pub fn review_brief_cache_key_from_parts(
    repository: &str,
    number: i64,
    provider: CodeTourProvider,
    code_version: &str,
) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        REVIEW_BRIEF_CACHE_KEY_PREFIX,
        provider.slug(),
        repository,
        number,
        code_version,
    )
}

pub fn build_review_brief_prompt(input: &GenerateReviewBriefInput) -> String {
    let schema = serde_json::to_string_pretty(
        &serde_json::from_str::<Value>(crate::agents::schema::REVIEW_BRIEF_OUTPUT_SCHEMA_JSON)
            .expect("review brief schema must parse"),
    )
    .expect("schema must serialize");
    let context =
        serde_json::to_string_pretty(&build_prompt_context(input)).expect("context must serialize");

    [
        "You are generating a concise Review Brief for a GitHub pull request before the reviewer opens the diff.",
        "Act like a senior reviewer orienting another reviewer who already knows the codebase.",
        "Return strict JSON only. No markdown fences, no prose outside JSON.",
        "Stay grounded in the provided PR metadata, author body when present, raw diff, parsed diff, files, review threads, reviews, and local checkout.",
        "Use the local checkout only for quick read-only verification of changed files or direct supporting context.",
        "Do not edit files, run write commands, create branches, or write back to GitHub.",
        "If the author body is empty or unhelpful, infer intent neutrally from the title, diff, files, and discussion.",
        "Phrase inferred intent as Likely intent inside the brief.",
        "Do not call out an empty, missing, or weak author description as a warning.",
        "Keep the brief short enough for a compact native desktop overview panel.",
        "Use changedSummary for concrete code changes, not repository background.",
        "Use risksQuestions for review risks, checks, and unresolved questions.",
        "",
        "JSON schema:",
        &schema,
        "",
        "Pull-request context:",
        &context,
    ]
    .join("\n")
}

fn build_prompt_context(input: &GenerateReviewBriefInput) -> Value {
    json!({
        "repository": input.repository,
        "workingDirectory": input.working_directory,
        "pullRequest": {
            "number": input.number,
            "title": input.title,
            "url": input.url,
            "authorLogin": input.author_login,
            "reviewDecision": input.review_decision,
            "baseRefName": input.base_ref_name,
            "headRefName": input.head_ref_name,
            "headRefOid": input.head_ref_oid,
            "createdAt": input.created_at,
            "updatedAt": input.updated_at,
            "labels": input.labels,
            "stats": {
                "commits": input.commits_count,
                "changedFiles": input.changed_files,
                "additions": input.additions,
                "deletions": input.deletions,
            },
            "authorBodyPresent": !input.author_body.trim().is_empty(),
            "authorBody": trim_text(&input.author_body, MAX_BODY_CHARS),
        },
        "files": input
            .files
            .iter()
            .take(MAX_FILES)
            .map(|file| json!({
                "path": file.path,
                "changeType": file.change_type,
                "additions": file.additions,
                "deletions": file.deletions,
            }))
            .collect::<Vec<_>>(),
        "rawDiff": trim_text(&input.raw_diff, MAX_RAW_DIFF_CHARS),
        "parsedDiff": summarize_parsed_diff(&input.parsed_diff),
        "latestReviews": input
            .latest_reviews
            .iter()
            .take(MAX_REVIEWS)
            .map(|review| json!({
                "authorLogin": review.author_login,
                "state": review.state,
                "submittedAt": review.submitted_at,
                "body": trim_text(&review.body, MAX_REVIEW_BODY_CHARS),
            }))
            .collect::<Vec<_>>(),
        "reviewThreads": input
            .review_threads
            .iter()
            .take(MAX_THREADS)
            .map(|thread| json!({
                "path": thread.path,
                "line": thread.line,
                "diffSide": thread.diff_side,
                "subjectType": thread.subject_type,
                "isResolved": thread.is_resolved,
                "comments": thread
                    .comments
                    .iter()
                    .take(MAX_COMMENTS_PER_THREAD)
                    .map(|comment| json!({
                        "authorLogin": comment.author_login,
                        "body": trim_text(&comment.body, MAX_COMMENT_BODY_CHARS),
                    }))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    })
}

fn summarize_parsed_diff(files: &[ParsedDiffFile]) -> Vec<Value> {
    files
        .iter()
        .take(MAX_PARSED_DIFF_FILES)
        .map(|file| {
            json!({
                "path": file.path,
                "previousPath": file.previous_path,
                "isBinary": file.is_binary,
                "hunks": file
                    .hunks
                    .iter()
                    .take(MAX_HUNKS_PER_FILE)
                    .map(summarize_hunk)
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn summarize_hunk(hunk: &ParsedDiffHunk) -> Value {
    json!({
        "header": hunk.header,
        "lines": hunk
            .lines
            .iter()
            .filter(|line| matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion))
            .take(MAX_LINES_PER_HUNK)
            .map(summarize_line)
            .collect::<Vec<_>>(),
    })
}

fn summarize_line(line: &ParsedDiffLine) -> Value {
    let kind = match &line.kind {
        DiffLineKind::Addition => "addition",
        DiffLineKind::Deletion => "deletion",
        DiffLineKind::Context => "context",
        DiffLineKind::Meta => "meta",
    };

    json!({
        "kind": kind,
        "leftLine": line.left_line_number,
        "rightLine": line.right_line_number,
        "content": trim_text(&line.content, 220),
    })
}

fn merge_review_brief(
    response: ReviewBriefResponse,
    input: &GenerateReviewBriefInput,
    model: Option<String>,
) -> Result<ReviewBrief, String> {
    let likely_intent = normalized_required_text(response.likely_intent, "likelyIntent")?;
    let changed_summary = normalize_text_items(response.changed_summary, "changedSummary")?;
    let risks_questions = normalize_text_items(response.risks_questions, "risksQuestions")?;
    let warnings = normalize_optional_text_items(response.warnings)
        .into_iter()
        .filter(|warning| !is_missing_author_body_warning(warning))
        .collect();
    let related_file_paths = response
        .related_file_paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .take(12)
        .collect();

    Ok(ReviewBrief {
        provider: input.provider,
        generated_at_ms: now_ms(),
        code_version_key: input.code_version_key.clone(),
        confidence: response.confidence,
        likely_intent,
        changed_summary,
        risks_questions,
        warnings,
        related_file_paths,
        model,
    })
}

fn normalized_required_text(value: String, field: &str) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(format!("Review brief response omitted {field}."))
    } else {
        Ok(value)
    }
}

fn normalize_text_items(values: Vec<String>, field: &str) -> Result<Vec<String>, String> {
    let normalized = normalize_optional_text_items(values);
    if normalized.is_empty() {
        Err(format!("Review brief response omitted {field}."))
    } else {
        Ok(normalized)
    }
}

fn normalize_optional_text_items(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .take(8)
        .collect()
}

fn is_missing_author_body_warning(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    (lower.contains("missing") || lower.contains("empty") || lower.contains("no "))
        && (lower.contains("description")
            || lower.contains("author body")
            || lower.contains("pr body"))
}

fn map_file_context(file: &PullRequestFile) -> ReviewBriefFileContext {
    ReviewBriefFileContext {
        path: file.path.clone(),
        change_type: file.change_type.clone(),
        additions: file.additions,
        deletions: file.deletions,
    }
}

fn map_review_context(review: &PullRequestReview) -> CodeTourReviewContext {
    CodeTourReviewContext {
        author_login: review.author_login.clone(),
        state: review.state.clone(),
        body: review.body.clone(),
        submitted_at: review.submitted_at.clone(),
    }
}

fn map_thread_context(thread: &PullRequestReviewThread) -> CodeTourReviewThreadContext {
    CodeTourReviewThreadContext {
        path: thread.path.clone(),
        line: thread.line,
        diff_side: if thread.diff_side.is_empty() {
            None
        } else {
            Some(thread.diff_side.clone())
        },
        is_resolved: thread.is_resolved,
        subject_type: thread.subject_type.clone(),
        comments: thread
            .comments
            .iter()
            .map(|comment| CodeTourReviewCommentContext {
                author_login: comment.author_login.clone(),
                body: comment.body.clone(),
            })
            .collect(),
    }
}

fn prioritize_review_threads(threads: &[PullRequestReviewThread]) -> Vec<&PullRequestReviewThread> {
    let mut prioritized = threads.iter().collect::<Vec<_>>();
    prioritized.sort_by_key(|thread| thread.is_resolved);
    prioritized
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        code_tour::CodeTourProvider,
        diff::parse_unified_diff,
        github::{PullRequestDataCompleteness, PullRequestDetail, PullRequestFile},
    };

    fn detail(updated_at: &str, head_ref_oid: Option<&str>, raw_diff: &str) -> PullRequestDetail {
        PullRequestDetail {
            id: "pr1".to_string(),
            repository: "acme/api".to_string(),
            number: 42,
            title: "Tighten session handling".to_string(),
            body: String::new(),
            url: "https://example.com/pr/42".to_string(),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature/session-guard".to_string(),
            base_ref_oid: Some("base123".to_string()),
            head_ref_oid: head_ref_oid.map(str::to_string),
            additions: 3,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-04-17T00:00:00Z".to_string(),
            updated_at: updated_at.to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: std::collections::BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: vec![PullRequestFile {
                path: "src/session.rs".to_string(),
                additions: 3,
                deletions: 1,
                change_type: "MODIFIED".to_string(),
            }],
            raw_diff: raw_diff.to_string(),
            parsed_diff: parse_unified_diff(raw_diff),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }

    #[test]
    fn review_brief_cache_key_varies_by_provider_and_head() {
        let first = detail(
            "2026-04-17T10:00:00Z",
            Some("head123"),
            "diff --git a/a b/a\n+one\n",
        );
        let changed = detail(
            "2026-04-17T10:00:00Z",
            Some("head456"),
            "diff --git a/a b/a\n+one\n",
        );

        assert_ne!(
            review_brief_cache_key(&first, CodeTourProvider::Codex),
            review_brief_cache_key(&first, CodeTourProvider::Copilot)
        );
        assert_ne!(
            review_brief_cache_key(&first, CodeTourProvider::Codex),
            review_brief_cache_key(&changed, CodeTourProvider::Codex)
        );
    }

    #[test]
    fn review_brief_cache_key_ignores_metadata_updates_when_head_matches() {
        let mut first = detail(
            "2026-04-17T10:00:00Z",
            Some("head123"),
            "diff --git a/a b/a\n+one\n",
        );
        let mut second = detail(
            "2026-04-17T11:00:00Z",
            Some("head123"),
            "diff --git a/a b/a\n+one\n",
        );
        first.title = "Old title".to_string();
        second.title = "New title".to_string();
        second.body = "Updated description".to_string();

        assert_eq!(
            review_brief_cache_key(&first, CodeTourProvider::Codex),
            review_brief_cache_key(&second, CodeTourProvider::Codex)
        );
    }

    #[test]
    fn review_brief_prompt_includes_schema_checkout_diff_files_and_inference_rules() {
        let raw_diff = r#"diff --git a/src/session.rs b/src/session.rs
--- a/src/session.rs
+++ b/src/session.rs
@@ -1,3 +1,5 @@
 pub fn check() {
-    allow();
+    require_token();
 }
"#;
        let detail = detail("2026-04-17T10:00:00Z", Some("head123"), raw_diff);
        let input = build_review_brief_generation_input(
            &detail,
            CodeTourProvider::Copilot,
            "/tmp/acme-api",
        );
        let prompt = build_review_brief_prompt(&input);

        assert!(prompt.contains("JSON schema:"));
        assert!(prompt.contains("\"workingDirectory\": \"/tmp/acme-api\""));
        assert!(prompt.contains("\"rawDiff\""));
        assert!(prompt.contains("\"parsedDiff\""));
        assert!(prompt.contains("src/session.rs"));
        assert!(prompt.contains("Phrase inferred intent as Likely intent"));
        assert!(prompt.contains(
            "Do not call out an empty, missing, or weak author description as a warning"
        ));
    }

    #[test]
    fn review_brief_merge_filters_missing_description_warnings() {
        let input = build_review_brief_generation_input(
            &detail(
                "2026-04-17T10:00:00Z",
                Some("head123"),
                "diff --git a/a b/a\n+one\n",
            ),
            CodeTourProvider::Codex,
            "/tmp/acme-api",
        );
        let brief = merge_review_brief(
            ReviewBriefResponse {
                confidence: ReviewBriefConfidence::High,
                likely_intent: "Likely intent: tighten session checks.".to_string(),
                changed_summary: vec!["Adds a token requirement.".to_string()],
                risks_questions: vec!["Verify existing sessions still work.".to_string()],
                warnings: vec![
                    "Missing PR description.".to_string(),
                    "Generated file is large.".to_string(),
                ],
                related_file_paths: vec!["src/session.rs".to_string()],
            },
            &input,
            Some("model".to_string()),
        )
        .expect("brief should merge");

        assert_eq!(brief.warnings, vec!["Generated file is large."]);
    }
}
