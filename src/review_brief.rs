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

const REVIEW_BRIEF_CACHE_KEY_PREFIX: &str = "review-brief-v2";
const REVIEW_BRIEF_BUDGET_ERROR_PREFIX: &str = "Review brief compact budget exceeded";
const REVIEW_BRIEF_PARAGRAPH_MAX_CHARS: usize = 280;
const REVIEW_BRIEF_RETRY_PARAGRAPH_TARGET_CHARS: usize = 220;
const REVIEW_BRIEF_INTENT_MAX_CHARS: usize = 120;
const REVIEW_BRIEF_ITEM_MAX_CHARS: usize = 100;
const REVIEW_BRIEF_CHANGED_MIN_ITEMS: usize = 1;
const REVIEW_BRIEF_CHANGED_MAX_ITEMS: usize = 2;
const REVIEW_BRIEF_RISKS_REQUIRED_ITEMS: usize = 1;
const REVIEW_BRIEF_WARNINGS_MAX_ITEMS: usize = 1;
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
    pub brief_paragraph: String,
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
    brief_paragraph: String,
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

    let (parsed, model) = request_review_brief_response(&input, false)?;
    let brief = match merge_review_brief(parsed, &input, model) {
        Ok(brief) => brief,
        Err(error) if is_review_brief_budget_error(&error) => {
            let (retry_parsed, retry_model) = request_review_brief_response(&input, true)?;
            merge_review_brief(retry_parsed, &input, retry_model).map_err(|retry_error| {
                if is_review_brief_budget_error(&retry_error) {
                    format!(
                        "Review brief response still exceeded compact limits after retry. {retry_error}"
                    )
                } else {
                    retry_error
                }
            })?
        }
        Err(error) => return Err(error),
    };

    let cache_key = review_brief_cache_key_from_parts(
        &input.repository,
        input.number,
        input.provider,
        &input.code_version_key,
    );
    cache.put(&cache_key, &brief, now_ms())?;

    Ok(brief)
}

fn request_review_brief_response(
    input: &GenerateReviewBriefInput,
    compact_retry: bool,
) -> Result<(ReviewBriefResponse, Option<String>), String> {
    let prompt = build_review_brief_prompt_for_attempt(input, compact_retry);
    let response = agents::run_json_prompt(input.provider, &input.working_directory, prompt)?;
    let parsed = parse_tolerant::<ReviewBriefResponse>(&response.text)
        .map_err(|error| format!("Failed to parse review brief JSON: {}", error.message))?;

    Ok((parsed, response.model))
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
    build_review_brief_prompt_for_attempt(input, false)
}

fn build_retry_review_brief_prompt(input: &GenerateReviewBriefInput) -> String {
    build_review_brief_prompt_for_attempt(input, true)
}

fn build_review_brief_prompt_for_attempt(
    input: &GenerateReviewBriefInput,
    compact_retry: bool,
) -> String {
    let schema = serde_json::to_string_pretty(
        &serde_json::from_str::<Value>(crate::agents::schema::REVIEW_BRIEF_OUTPUT_SCHEMA_JSON)
            .expect("review brief schema must parse"),
    )
    .expect("schema must serialize");
    let context =
        serde_json::to_string_pretty(&build_prompt_context(input)).expect("context must serialize");

    let mut lines = vec![
        "You are generating a compact Review Brief for a GitHub pull request before the reviewer opens the diff.".to_string(),
        "Act like a senior reviewer orienting another reviewer who already knows the codebase.".to_string(),
        "Return strict JSON only. No markdown fences, no prose outside JSON.".to_string(),
        "Stay grounded in the provided PR metadata, author body when present, raw diff, parsed diff, files, review threads, reviews, and local checkout.".to_string(),
        "Use the local checkout only for quick read-only verification of changed files or direct supporting context.".to_string(),
        "Do not edit files, run write commands, create branches, or write back to GitHub.".to_string(),
        "If the author body is empty or unhelpful, infer intent neutrally from the title, diff, files, and discussion.".to_string(),
        "Use likelyIntent for the neutral inferred intent; do not prefix any field with 'Likely intent:'.".to_string(),
        "Do not call out an empty, missing, or weak author description as a warning.".to_string(),
        format!(
            "briefParagraph must be one natural-prose paragraph under {REVIEW_BRIEF_PARAGRAPH_MAX_CHARS} characters: no bullets, no markdown, no newlines, and no section labels like Likely intent, Changes, Watch, or Risk."
        ),
        format!(
            "Use changedSummary for {REVIEW_BRIEF_CHANGED_MIN_ITEMS}-{REVIEW_BRIEF_CHANGED_MAX_ITEMS} concrete code-change points, each under {REVIEW_BRIEF_ITEM_MAX_CHARS} characters."
        ),
        format!(
            "Use risksQuestions for exactly {REVIEW_BRIEF_RISKS_REQUIRED_ITEMS} concrete review risk, check, or unresolved question under {REVIEW_BRIEF_ITEM_MAX_CHARS} characters."
        ),
        format!(
            "Keep likelyIntent under {REVIEW_BRIEF_INTENT_MAX_CHARS} characters and warnings to at most {REVIEW_BRIEF_WARNINGS_MAX_ITEMS} hidden item."
        ),
    ];

    if compact_retry {
        lines.push(format!(
            "Your previous response violated the compact output limits. Rewrite more aggressively: keep briefParagraph under {REVIEW_BRIEF_RETRY_PARAGRAPH_TARGET_CHARS} characters, keep changedSummary to one item unless the second is essential, and use terse natural prose."
        ));
    }

    lines.extend([
        "".to_string(),
        "JSON schema:".to_string(),
        schema,
        "".to_string(),
        "Pull-request context:".to_string(),
        context,
    ]);

    lines.join("\n")
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
    let brief_paragraph = normalize_brief_paragraph(response.brief_paragraph)?;
    let likely_intent = normalized_required_limited_text(
        response.likely_intent,
        "likelyIntent",
        REVIEW_BRIEF_INTENT_MAX_CHARS,
    )?;
    let changed_summary = normalize_text_items(
        response.changed_summary,
        "changedSummary",
        REVIEW_BRIEF_CHANGED_MIN_ITEMS,
        REVIEW_BRIEF_CHANGED_MAX_ITEMS,
        REVIEW_BRIEF_ITEM_MAX_CHARS,
    )?;
    let risks_questions = normalize_text_items(
        response.risks_questions,
        "risksQuestions",
        REVIEW_BRIEF_RISKS_REQUIRED_ITEMS,
        REVIEW_BRIEF_RISKS_REQUIRED_ITEMS,
        REVIEW_BRIEF_ITEM_MAX_CHARS,
    )?;
    let warnings =
        normalize_optional_text_items(response.warnings, "warnings", REVIEW_BRIEF_ITEM_MAX_CHARS)?
            .into_iter()
            .filter(|warning| !is_missing_author_body_warning(warning))
            .collect::<Vec<_>>();
    if warnings.len() > REVIEW_BRIEF_WARNINGS_MAX_ITEMS {
        return Err(compact_budget_error(format!(
            "warnings returned {} items, max {}.",
            warnings.len(),
            REVIEW_BRIEF_WARNINGS_MAX_ITEMS
        )));
    }
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
        brief_paragraph,
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

fn normalize_brief_paragraph(value: String) -> Result<String, String> {
    let value = normalized_required_limited_text(
        value,
        "briefParagraph",
        REVIEW_BRIEF_PARAGRAPH_MAX_CHARS,
    )?;
    let trimmed = value.trim_start();
    let lower = value.to_ascii_lowercase();

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return Err(compact_budget_error(
            "briefParagraph must be natural prose, not a bullet.".to_string(),
        ));
    }

    if value.contains('`') || value.contains("**") {
        return Err(compact_budget_error(
            "briefParagraph must not use markdown formatting.".to_string(),
        ));
    }

    if lower.contains("likely intent:")
        || lower.contains("changes:")
        || lower.contains("watch:")
        || lower.contains("risk:")
        || lower.contains("risks/questions")
    {
        return Err(compact_budget_error(
            "briefParagraph must not include section labels.".to_string(),
        ));
    }

    Ok(value)
}

fn normalized_required_limited_text(
    value: String,
    field: &str,
    max_chars: usize,
) -> Result<String, String> {
    let value = normalized_required_text(value, field)?;
    validate_compact_text(&value, field, max_chars)?;
    Ok(value)
}

fn validate_compact_text(value: &str, field: &str, max_chars: usize) -> Result<(), String> {
    let char_count = value.chars().count();
    if char_count > max_chars {
        return Err(compact_budget_error(format!(
            "{field} has {char_count} characters, max {max_chars}."
        )));
    }

    if value.contains('\n') || value.contains('\r') {
        return Err(compact_budget_error(format!(
            "{field} must be a single line."
        )));
    }

    Ok(())
}

fn normalize_text_items(
    values: Vec<String>,
    field: &str,
    min_items: usize,
    max_items: usize,
    max_chars: usize,
) -> Result<Vec<String>, String> {
    let normalized = normalize_optional_text_items(values, field, max_chars)?;
    if normalized.len() < min_items {
        Err(format!("Review brief response omitted {field}."))
    } else if normalized.len() > max_items {
        Err(compact_budget_error(format!(
            "{field} returned {} items, max {max_items}.",
            normalized.len()
        )))
    } else {
        Ok(normalized)
    }
}

fn normalize_optional_text_items(
    values: Vec<String>,
    field: &str,
    max_chars: usize,
) -> Result<Vec<String>, String> {
    values
        .into_iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let value = value.trim().to_string();
            if value.is_empty() {
                None
            } else {
                Some((index, value))
            }
        })
        .map(|(index, value)| {
            validate_compact_text(&value, &format!("{field}[{index}]"), max_chars)?;
            Ok(value)
        })
        .collect()
}

fn compact_budget_error(message: String) -> String {
    format!("{REVIEW_BRIEF_BUDGET_ERROR_PREFIX}: {message}")
}

fn is_review_brief_budget_error(error: &str) -> bool {
    error.starts_with(REVIEW_BRIEF_BUDGET_ERROR_PREFIX)
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
            viewer_pending_review: None,
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
        assert!(
            review_brief_cache_key(&first, CodeTourProvider::Codex).starts_with("review-brief-v2:")
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
        assert!(prompt.contains("briefParagraph"));
        assert!(prompt.contains("one natural-prose paragraph"));
        assert!(prompt.contains("no bullets, no markdown, no newlines"));
        assert!(prompt.contains("no section labels"));
        assert!(prompt.contains("\"workingDirectory\": \"/tmp/acme-api\""));
        assert!(prompt.contains("\"rawDiff\""));
        assert!(prompt.contains("\"parsedDiff\""));
        assert!(prompt.contains("src/session.rs"));
        assert!(prompt.contains("Use likelyIntent for the neutral inferred intent"));
        assert!(prompt.contains(
            "Do not call out an empty, missing, or weak author description as a warning"
        ));
    }

    #[test]
    fn review_brief_retry_prompt_is_stricter() {
        let input = build_review_brief_generation_input(
            &detail(
                "2026-04-17T10:00:00Z",
                Some("head123"),
                "diff --git a/a b/a\n+one\n",
            ),
            CodeTourProvider::Copilot,
            "/tmp/acme-api",
        );
        let prompt = build_retry_review_brief_prompt(&input);

        assert!(prompt.contains("previous response violated the compact output limits"));
        assert!(prompt.contains("under 220 characters"));
        assert!(prompt.contains("keep changedSummary to one item unless the second is essential"));
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
                brief_paragraph: "This tightens session checks by requiring a token before allowing access; verify existing sessions still authenticate cleanly.".to_string(),
                likely_intent: "Tighten session checks.".to_string(),
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

    #[test]
    fn review_brief_merge_rejects_overlong_compact_fields() {
        let input = build_review_brief_generation_input(
            &detail(
                "2026-04-17T10:00:00Z",
                Some("head123"),
                "diff --git a/a b/a\n+one\n",
            ),
            CodeTourProvider::Codex,
            "/tmp/acme-api",
        );
        let error = merge_review_brief(
            ReviewBriefResponse {
                confidence: ReviewBriefConfidence::High,
                brief_paragraph: "a".repeat(REVIEW_BRIEF_PARAGRAPH_MAX_CHARS + 1),
                likely_intent: "Tighten session checks.".to_string(),
                changed_summary: vec!["Adds a token requirement.".to_string()],
                risks_questions: vec!["Verify existing sessions still work.".to_string()],
                warnings: Vec::new(),
                related_file_paths: Vec::new(),
            },
            &input,
            Some("model".to_string()),
        )
        .expect_err("overlong paragraph should be rejected");

        assert!(is_review_brief_budget_error(&error));
        assert!(error.contains("briefParagraph"));
    }

    #[test]
    fn review_brief_merge_rejects_too_many_summary_items() {
        let input = build_review_brief_generation_input(
            &detail(
                "2026-04-17T10:00:00Z",
                Some("head123"),
                "diff --git a/a b/a\n+one\n",
            ),
            CodeTourProvider::Codex,
            "/tmp/acme-api",
        );
        let error = merge_review_brief(
            ReviewBriefResponse {
                confidence: ReviewBriefConfidence::High,
                brief_paragraph: "This tightens session checks by requiring a token before allowing access; verify existing sessions still authenticate cleanly.".to_string(),
                likely_intent: "Tighten session checks.".to_string(),
                changed_summary: vec![
                    "Adds a token requirement.".to_string(),
                    "Updates the session guard.".to_string(),
                    "Refreshes related checks.".to_string(),
                ],
                risks_questions: vec!["Verify existing sessions still work.".to_string()],
                warnings: Vec::new(),
                related_file_paths: Vec::new(),
            },
            &input,
            Some("model".to_string()),
        )
        .expect_err("too many summary items should be rejected");

        assert!(is_review_brief_budget_error(&error));
        assert!(error.contains("changedSummary"));
    }
}
