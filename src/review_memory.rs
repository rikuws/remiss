use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::{
    agents::{self, jsonrepair::parse_tolerant, AgentJsonPromptOptions},
    cache::CacheStore,
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk},
    github::{PullRequestDetail, PullRequestReviewThread},
    review_ai::{ReviewAiProgressUpdate, ReviewAiProvider},
};

const REVIEW_MEMORY_CACHE_PREFIX: &str = "review-memory-v1";
const REVIEW_MEMORY_LLM_EXTRACTION_PREFIX: &str = "review-memory-llm-extraction-v1";
const REVIEW_MEMORY_DOCUMENT_VERSION: &str = "review-memory-v1";
const REVIEW_MEMORY_LLM_EXTRACTION_VERSION: &str = "review-memory-llm-extraction-v1";
const MAX_MEMORY_ENTRIES: usize = 500;
const MAX_PROMPT_SIGNALS: usize = 3;
const MAX_EXCERPT_CHARS: usize = 220;
const MAX_LLM_CANDIDATES: usize = 8;
const MAX_LLM_FILES: usize = 60;
const MAX_LLM_THREADS: usize = 18;
const MAX_LLM_COMMENTS: usize = 12;
const MAX_LLM_REVIEWS: usize = 8;
const MAX_LLM_HUNKS: usize = 40;
const MAX_LLM_HUNK_LINES: usize = 18;
const MAX_LLM_BODY_CHARS: usize = 2_500;
const MAX_LLM_COMMENT_CHARS: usize = 700;
const MAX_LLM_DIFF_LINE_CHARS: usize = 220;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMemoryDocument {
    pub version: String,
    pub repository: String,
    #[serde(default)]
    pub entries: Vec<ReviewMemoryEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMemoryEntry {
    pub id: String,
    pub repository: String,
    pub kind: ReviewMemoryKind,
    #[serde(default)]
    pub reading_level: ReviewMemoryReadingLevel,
    #[serde(default)]
    pub origin: ReviewMemoryOrigin,
    pub scope: ReviewMemoryScope,
    pub statement: String,
    #[serde(default)]
    pub why_useful_next_time: Option<String>,
    #[serde(default)]
    pub normalized_tags: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<ReviewMemoryEvidence>,
    pub confidence: ReviewMemoryConfidence,
    pub status: ReviewMemoryStatus,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub first_seen_pr: Option<i64>,
    #[serde(default)]
    pub last_seen_pr: Option<i64>,
    #[serde(default)]
    pub valid_from_oid: Option<String>,
    #[serde(default)]
    pub valid_until_oid: Option<String>,
    #[serde(default)]
    pub stale_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMemoryReadingLevel {
    Mechanics,
    Behavior,
    Intent,
    #[default]
    History,
}

impl ReviewMemoryReadingLevel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Mechanics => "mechanics",
            Self::Behavior => "behavior",
            Self::Intent => "intent",
            Self::History => "history",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReviewMemoryOrigin {
    #[default]
    Deterministic,
    LlmCandidate,
    UserPromoted,
}

impl ReviewMemoryOrigin {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Deterministic => "observed from review data",
            Self::LlmCandidate => "candidate from PR discussion",
            Self::UserPromoted => "project note",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMemoryKind {
    ReviewConcern,
    DesignDecision,
    CodebaseConvention,
    KnownRisk,
    TestingExpectation,
    GeneratedCodeCaveat,
    MigrationNote,
    OpenQuestion,
    ResolvedQuestion,
}

impl ReviewMemoryKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ReviewConcern => "review concern",
            Self::DesignDecision => "design decision",
            Self::CodebaseConvention => "codebase convention",
            Self::KnownRisk => "known risk",
            Self::TestingExpectation => "testing expectation",
            Self::GeneratedCodeCaveat => "generated-code caveat",
            Self::MigrationNote => "migration note",
            Self::OpenQuestion => "open question",
            Self::ResolvedQuestion => "resolved question",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReviewMemoryScope {
    Repository,
    Directory {
        path: String,
    },
    File {
        path: String,
    },
    Symbol {
        path: String,
        name: String,
        kind: String,
    },
    StackLayerPattern {
        title: String,
    },
    TestTarget {
        path: String,
        symbol: String,
    },
}

impl ReviewMemoryScope {
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Directory { path }
            | Self::File { path }
            | Self::Symbol { path, .. }
            | Self::TestTarget { path, .. } => Some(path),
            _ => None,
        }
    }

    pub fn symbol_name(&self) -> Option<&str> {
        match self {
            Self::Symbol { name, .. } => Some(name),
            Self::TestTarget { symbol, .. } => Some(symbol),
            _ => None,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Repository => "repository".to_string(),
            Self::Directory { path } => path.clone(),
            Self::File { path } => path.clone(),
            Self::Symbol { path, name, .. } => format!("{path}::{name}"),
            Self::StackLayerPattern { title } => title.clone(),
            Self::TestTarget { path, symbol } => format!("{path}::{symbol}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReviewMemoryEvidence {
    PullRequest {
        number: i64,
        title: String,
        url: String,
        head_oid: Option<String>,
    },
    PullRequestBody {
        number: i64,
        excerpt: String,
    },
    ReviewThread {
        pr_number: i64,
        path: String,
        line: Option<i64>,
        resolved: bool,
        excerpt: String,
        comment_ids: Vec<String>,
    },
    ReviewComment {
        pr_number: i64,
        author_login: String,
        excerpt: String,
    },
    DiffHunk {
        pr_number: i64,
        path: String,
        hunk_header: Option<String>,
    },
    UserNote {
        body: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMemoryConfidence {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMemoryStatus {
    Candidate,
    Open,
    Resolved,
    Promoted,
    Stale,
    Hidden,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMemoryPromptContext {
    #[serde(default)]
    pub signals: Vec<ReviewMemorySignal>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMemorySignal {
    pub id: String,
    pub kind: ReviewMemoryKind,
    #[serde(default)]
    pub reading_level: ReviewMemoryReadingLevel,
    #[serde(default)]
    pub origin: ReviewMemoryOrigin,
    pub scope: ReviewMemoryScope,
    pub statement: String,
    #[serde(default)]
    pub why_useful_next_time: Option<String>,
    pub evidence_summary: String,
    pub confidence: ReviewMemoryConfidence,
    pub status: ReviewMemoryStatus,
    pub first_seen_pr: Option<i64>,
    pub last_seen_pr: Option<i64>,
    pub path: Option<String>,
    pub line: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewMemoryTarget {
    pub path: String,
    pub symbol_name: Option<String>,
    pub symbol_kind: Option<String>,
}

pub fn load_review_memory(
    cache: &CacheStore,
    repository: &str,
) -> Result<ReviewMemoryDocument, String> {
    let key = review_memory_cache_key(repository);
    Ok(cache
        .get::<ReviewMemoryDocument>(&key)?
        .map(|document| document.value)
        .filter(|document| document.version == REVIEW_MEMORY_DOCUMENT_VERSION)
        .unwrap_or_else(|| ReviewMemoryDocument {
            version: REVIEW_MEMORY_DOCUMENT_VERSION.to_string(),
            repository: repository.to_string(),
            entries: Vec::new(),
        }))
}

pub fn save_review_memory(
    cache: &CacheStore,
    document: &ReviewMemoryDocument,
) -> Result<(), String> {
    let key = review_memory_cache_key(&document.repository);
    cache.put(&key, document, now_ms())
}

pub fn record_pull_request_memory(
    cache: &CacheStore,
    detail: &PullRequestDetail,
) -> Result<usize, String> {
    let mut document = load_review_memory(cache, &detail.repository)?;
    let entries = extract_review_memory_entries(detail);
    let inserted = upsert_entries(&mut document, entries);
    save_review_memory(cache, &document)?;
    Ok(inserted)
}

pub fn extract_review_memory_entries(detail: &PullRequestDetail) -> Vec<ReviewMemoryEntry> {
    let mut entries = detail
        .review_threads
        .iter()
        .filter_map(|thread| entry_from_review_thread(detail, thread))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    entries
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMemoryLlmExtractionDocument {
    pub version: String,
    pub repository: String,
    pub pr_number: i64,
    pub provider: ReviewAiProvider,
    pub code_version_key: String,
    pub generated_at_ms: i64,
    #[serde(default)]
    pub model: Option<String>,
    pub candidate_count: usize,
    pub inserted_count: usize,
    #[serde(default)]
    pub warnings: Vec<String>,
}

pub fn generate_llm_review_memory_candidates(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
    working_directory: &str,
    force: bool,
) -> Result<ReviewMemoryLlmExtractionDocument, String> {
    generate_llm_review_memory_candidates_with_progress(
        cache,
        detail,
        provider,
        working_directory,
        force,
        &mut |_| {},
    )
}

pub fn generate_llm_review_memory_candidates_with_progress(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
    working_directory: &str,
    force: bool,
    on_progress: &mut dyn FnMut(ReviewAiProgressUpdate),
) -> Result<ReviewMemoryLlmExtractionDocument, String> {
    if working_directory.trim().is_empty() {
        return Err("Review Memory extraction requires a local checkout path.".to_string());
    }

    if !Path::new(working_directory).exists() {
        return Err(format!(
            "The local checkout path '{working_directory}' does not exist."
        ));
    }

    let cache_key = llm_extraction_cache_key(detail, provider);
    if !force {
        if let Some(document) = cache
            .get::<ReviewMemoryLlmExtractionDocument>(&cache_key)?
            .map(|document| document.value)
            .filter(|document| {
                document.version == REVIEW_MEMORY_LLM_EXTRACTION_VERSION
                    && document.repository == detail.repository
                    && document.pr_number == detail.number
                    && document.provider == provider
                    && document.code_version_key == detail_code_version_key(detail)
            })
        {
            return Ok(document);
        }
    }

    let prompt = build_llm_review_memory_prompt(detail);
    let response = agents::run_json_prompt_with_options_and_progress(
        provider,
        working_directory,
        prompt,
        AgentJsonPromptOptions::review_memory(),
        on_progress,
    )?;
    let parsed = parse_tolerant::<LlmReviewMemoryResponse>(&response.text).map_err(|error| {
        format!(
            "Failed to parse Review Memory candidate JSON: {}",
            error.message
        )
    })?;
    let entries = entries_from_llm_response(detail, parsed.candidates.as_slice());
    let candidate_count = entries.len();
    let mut memory = load_review_memory(cache, &detail.repository)?;
    let inserted_count = upsert_entries(&mut memory, entries);
    save_review_memory(cache, &memory)?;

    let document = ReviewMemoryLlmExtractionDocument {
        version: REVIEW_MEMORY_LLM_EXTRACTION_VERSION.to_string(),
        repository: detail.repository.clone(),
        pr_number: detail.number,
        provider,
        code_version_key: detail_code_version_key(detail),
        generated_at_ms: now_ms(),
        model: response.model,
        candidate_count,
        inserted_count,
        warnings: parsed
            .warnings
            .into_iter()
            .map(|warning| trim_text(&normalize_whitespace(&warning), MAX_EXCERPT_CHARS))
            .filter(|warning| !warning.trim().is_empty())
            .take(5)
            .collect(),
    };
    cache.put(&cache_key, &document, now_ms())?;
    Ok(document)
}

pub fn review_memory_prompt_context_for_detail(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    extra_targets: &[ReviewMemoryTarget],
    max_signals: usize,
) -> Result<ReviewMemoryPromptContext, String> {
    let document = load_review_memory(cache, &detail.repository)?;
    Ok(review_memory_prompt_context(
        &document,
        detail,
        extra_targets,
        max_signals,
    ))
}

pub fn review_memory_prompt_context(
    document: &ReviewMemoryDocument,
    detail: &PullRequestDetail,
    extra_targets: &[ReviewMemoryTarget],
    max_signals: usize,
) -> ReviewMemoryPromptContext {
    let mut targets = targets_from_detail(detail);
    targets.extend_from_slice(extra_targets);
    dedup_targets(&mut targets);
    let target_paths = targets
        .iter()
        .map(|target| target.path.as_str())
        .collect::<BTreeSet<_>>();

    let mut scored = document
        .entries
        .iter()
        .filter(|entry| entry.repository == detail.repository)
        .filter(|entry| !entry_seen_in_current_pr(entry, detail.number))
        .filter(|entry| {
            !matches!(
                entry.status,
                ReviewMemoryStatus::Hidden | ReviewMemoryStatus::Stale
            )
        })
        .filter_map(|entry| {
            let score = score_entry(entry, detail, &targets, &target_paths);
            (score > 0).then_some((score, entry))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    let signals = scored
        .into_iter()
        .take(max_signals.max(1))
        .map(|(_, entry)| signal_from_entry(entry))
        .collect::<Vec<_>>();

    let limitations = if document.entries.is_empty() {
        vec!["Only locally cached review history was searched; no prior review memory exists for this repository yet.".to_string()]
    } else if signals.is_empty() {
        vec![
            "Only locally cached review history was searched; current-pull-request memory is excluded from history signals.".to_string(),
        ]
    } else {
        vec!["Only locally cached review history was searched.".to_string()]
    };

    ReviewMemoryPromptContext {
        signals: signals
            .into_iter()
            .take(max_signals.clamp(1, MAX_PROMPT_SIGNALS))
            .collect(),
        limitations,
    }
}

pub fn targets_from_detail(detail: &PullRequestDetail) -> Vec<ReviewMemoryTarget> {
    let mut targets = detail
        .files
        .iter()
        .map(|file| ReviewMemoryTarget {
            path: file.path.clone(),
            symbol_name: None,
            symbol_kind: None,
        })
        .collect::<Vec<_>>();

    for parsed in &detail.parsed_diff {
        for hunk in &parsed.hunks {
            if let Some(symbol) = symbol_from_hunk_header(&hunk.header) {
                targets.push(ReviewMemoryTarget {
                    path: parsed.path.clone(),
                    symbol_name: Some(symbol.name),
                    symbol_kind: Some(symbol.kind),
                });
            }
        }
    }

    dedup_targets(&mut targets);
    targets
}

pub fn memory_cache_key_for_tests(repository: &str) -> String {
    review_memory_cache_key(repository)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmReviewMemoryResponse {
    #[serde(default)]
    candidates: Vec<LlmReviewMemoryCandidateResponse>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmReviewMemoryCandidateResponse {
    kind: ReviewMemoryKind,
    reading_level: ReviewMemoryReadingLevel,
    scope: LlmReviewMemoryScopeResponse,
    statement: String,
    #[serde(default)]
    why_useful_next_time: Option<String>,
    confidence: ReviewMemoryConfidence,
    #[serde(default)]
    normalized_tags: Vec<String>,
    #[serde(default)]
    evidence_refs: Vec<LlmReviewMemoryEvidenceRefResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmReviewMemoryScopeResponse {
    #[serde(rename = "type")]
    scope_type: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmReviewMemoryEvidenceRefResponse {
    source: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<i64>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    comment_id: Option<String>,
    #[serde(default)]
    hunk_header: Option<String>,
    #[serde(default)]
    excerpt: Option<String>,
    #[serde(default)]
    author_login: Option<String>,
}

fn review_memory_cache_key(repository: &str) -> String {
    format!("{REVIEW_MEMORY_CACHE_PREFIX}:{repository}")
}

fn entry_seen_in_current_pr(entry: &ReviewMemoryEntry, current_pr_number: i64) -> bool {
    entry.first_seen_pr == Some(current_pr_number) || entry.last_seen_pr == Some(current_pr_number)
}

fn llm_extraction_cache_key(detail: &PullRequestDetail, provider: ReviewAiProvider) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        REVIEW_MEMORY_LLM_EXTRACTION_PREFIX,
        provider.slug(),
        detail.repository,
        detail.number,
        detail_code_version_key(detail)
    )
}

fn upsert_entries(document: &mut ReviewMemoryDocument, entries: Vec<ReviewMemoryEntry>) -> usize {
    let mut by_id = document
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut inserted = 0usize;

    for entry in entries {
        if let Some(index) = by_id.get(&entry.id).copied() {
            let existing = &mut document.entries[index];
            existing.kind = entry.kind;
            existing.reading_level = entry.reading_level;
            existing.origin = entry.origin;
            existing.scope = entry.scope;
            existing.statement = entry.statement;
            existing.why_useful_next_time = entry.why_useful_next_time;
            existing.normalized_tags = entry.normalized_tags;
            existing.evidence = entry.evidence;
            existing.confidence = entry.confidence;
            existing.status = entry.status;
            existing.updated_at = entry.updated_at;
            existing.last_seen_pr = entry.last_seen_pr;
            existing.valid_from_oid = entry.valid_from_oid;
            existing.valid_until_oid = entry.valid_until_oid;
            existing.stale_reason = entry.stale_reason;
        } else {
            by_id.insert(entry.id.clone(), document.entries.len());
            document.entries.push(entry);
            inserted += 1;
        }
    }

    document.entries.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    if document.entries.len() > MAX_MEMORY_ENTRIES {
        document.entries.truncate(MAX_MEMORY_ENTRIES);
    }
    inserted
}

fn build_llm_review_memory_prompt(detail: &PullRequestDetail) -> String {
    let schema = serde_json::to_string_pretty(&llm_review_memory_output_schema())
        .expect("schema must serialize");
    let context = serde_json::to_string_pretty(&llm_review_memory_context(detail))
        .expect("context must serialize");

    [
        "You are extracting candidate Review Memory entries for Remiss, a read-only pull request review IDE.",
        "Your job is not to review the PR. Your job is to preserve durable, evidence-backed reading context for future reviews.",
        "Use this four-level code-reading model:",
        "1. mechanics: syntactic or structural changes, call order, variable flow. Store only when the mechanical fact supports future behavior, intent, or history understanding.",
        "2. behavior: state changes, data flow, invariants, error handling, ignored cases, or externally visible behavior.",
        "3. intent: reconstructed assumptions, trade-offs, chosen design, or author/reviewer-stated rationale. Label reconstructed intent as candidate memory, not fact.",
        "4. history: prior compromise, workaround, relax/tighten movement, review-thread concern, older behavior, or trap that future reviewers should know.",
        "Prefer behavior, intent, and history candidates over mechanics.",
        "Do not store superficial call-flow summaries, obvious file changes, or one-off implementation details that a future reviewer can see from the diff.",
        "Do not infer a project-wide convention from one PR unless PR text or review discussion explicitly states it as a convention.",
        "Every candidate must have concrete evidenceRefs pointing to PR body, review thread, review comment, or diff hunk context supplied below.",
        "Set status implicitly to candidate by returning only candidates. Remiss will store these as candidate memory, not promoted truth.",
        "Write statements as durable observations for future reviews, not instructions or checklists.",
        "Return strict JSON only. No markdown fences or prose outside JSON.",
        "",
        "JSON schema:",
        &schema,
        "",
        "Pull-request evidence context:",
        &context,
    ]
    .join("\n")
}

fn llm_review_memory_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "candidates": {
                "type": "array",
                "maxItems": MAX_LLM_CANDIDATES,
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": [
                                "review_concern",
                                "design_decision",
                                "codebase_convention",
                                "known_risk",
                                "testing_expectation",
                                "generated_code_caveat",
                                "migration_note",
                                "open_question",
                                "resolved_question"
                            ]
                        },
                        "readingLevel": {
                            "type": "string",
                            "enum": ["mechanics", "behavior", "intent", "history"]
                        },
                        "scope": {
                            "type": "object",
                            "properties": {
                                "type": {
                                    "type": "string",
                                    "enum": ["repository", "directory", "file", "symbol", "stackLayerPattern", "testTarget"]
                                },
                                "path": { "type": ["string", "null"] },
                                "name": { "type": ["string", "null"] },
                                "kind": { "type": ["string", "null"] },
                                "title": { "type": ["string", "null"] },
                                "symbol": { "type": ["string", "null"] }
                            },
                            "required": ["type"],
                            "additionalProperties": false
                        },
                        "statement": { "type": "string" },
                        "whyUsefulNextTime": { "type": ["string", "null"] },
                        "confidence": { "type": "string", "enum": ["low", "medium", "high"] },
                        "normalizedTags": { "type": "array", "items": { "type": "string" } },
                        "evidenceRefs": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "source": {
                                        "type": "string",
                                        "enum": ["pr_body", "review_thread", "review_comment", "diff_hunk"]
                                    },
                                    "path": { "type": ["string", "null"] },
                                    "line": { "type": ["integer", "null"] },
                                    "threadId": { "type": ["string", "null"] },
                                    "commentId": { "type": ["string", "null"] },
                                    "hunkHeader": { "type": ["string", "null"] },
                                    "excerpt": { "type": ["string", "null"] },
                                    "authorLogin": { "type": ["string", "null"] }
                                },
                                "required": ["source"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["kind", "readingLevel", "scope", "statement", "confidence", "evidenceRefs"],
                    "additionalProperties": false
                }
            },
            "warnings": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["candidates"],
        "additionalProperties": false
    })
}

fn llm_review_memory_context(detail: &PullRequestDetail) -> Value {
    json!({
        "repository": detail.repository,
        "pullRequest": {
            "number": detail.number,
            "title": detail.title,
            "url": detail.url,
            "authorLogin": detail.author_login,
            "baseRefName": detail.base_ref_name,
            "headRefName": detail.head_ref_name,
            "headRefOid": detail.head_ref_oid,
            "createdAt": detail.created_at,
            "updatedAt": detail.updated_at,
            "body": trim_text(&detail.body, MAX_LLM_BODY_CHARS),
            "labels": detail.labels,
            "reviewDecision": detail.review_decision,
            "stats": {
                "commits": detail.commits_count,
                "changedFiles": detail.changed_files,
                "additions": detail.additions,
                "deletions": detail.deletions,
            }
        },
        "files": detail.files.iter().take(MAX_LLM_FILES).map(|file| {
            json!({
                "path": file.path,
                "changeType": file.change_type,
                "additions": file.additions,
                "deletions": file.deletions,
            })
        }).collect::<Vec<_>>(),
        "comments": detail.comments.iter().take(MAX_LLM_COMMENTS).map(|comment| {
            json!({
                "id": comment.id,
                "authorLogin": comment.author_login,
                "createdAt": comment.created_at,
                "body": trim_text(&comment.body, MAX_LLM_COMMENT_CHARS),
            })
        }).collect::<Vec<_>>(),
        "latestReviews": detail.latest_reviews.iter().take(MAX_LLM_REVIEWS).map(|review| {
            json!({
                "id": review.id,
                "authorLogin": review.author_login,
                "state": review.state,
                "submittedAt": review.submitted_at,
                "body": trim_text(&review.body, MAX_LLM_COMMENT_CHARS),
            })
        }).collect::<Vec<_>>(),
        "reviewThreads": detail.review_threads.iter().take(MAX_LLM_THREADS).map(|thread| {
            json!({
                "id": thread.id,
                "path": thread.path,
                "line": thread.line,
                "originalLine": thread.original_line,
                "diffSide": thread.diff_side,
                "subjectType": thread.subject_type,
                "isResolved": thread.is_resolved,
                "isOutdated": thread.is_outdated,
                "comments": thread.comments.iter().take(4).map(|comment| {
                    json!({
                        "id": comment.id,
                        "authorLogin": comment.author_login,
                        "line": comment.line,
                        "originalLine": comment.original_line,
                        "createdAt": comment.created_at,
                        "body": trim_text(&comment.body, MAX_LLM_COMMENT_CHARS),
                    })
                }).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
        "diffHunks": detail.parsed_diff.iter().flat_map(|file| {
            file.hunks.iter().map(move |hunk| {
                json!({
                    "path": file.path,
                    "hunkHeader": hunk.header,
                    "symbol": symbol_from_hunk_header(&hunk.header).map(|symbol| {
                        json!({ "name": symbol.name, "kind": symbol.kind })
                    }),
                    "lines": hunk.lines.iter().take(MAX_LLM_HUNK_LINES).map(|line| {
                        json!({
                            "kind": format!("{:?}", line.kind),
                            "leftLineNumber": line.left_line_number,
                            "rightLineNumber": line.right_line_number,
                            "content": trim_text(&line.content, MAX_LLM_DIFF_LINE_CHARS),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
        }).take(MAX_LLM_HUNKS).collect::<Vec<_>>(),
    })
}

fn entries_from_llm_response(
    detail: &PullRequestDetail,
    candidates: &[LlmReviewMemoryCandidateResponse],
) -> Vec<ReviewMemoryEntry> {
    let mut entries = candidates
        .iter()
        .take(MAX_LLM_CANDIDATES)
        .filter_map(|candidate| entry_from_llm_candidate(detail, candidate))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    entries
}

fn entry_from_llm_candidate(
    detail: &PullRequestDetail,
    candidate: &LlmReviewMemoryCandidateResponse,
) -> Option<ReviewMemoryEntry> {
    let statement = trim_text(&normalize_whitespace(&candidate.statement), 360);
    if statement.is_empty() {
        return None;
    }

    let why_useful_next_time = candidate
        .why_useful_next_time
        .as_deref()
        .map(normalize_whitespace)
        .map(|value| trim_text(&value, 260))
        .filter(|value| !value.trim().is_empty());
    if candidate.reading_level == ReviewMemoryReadingLevel::Mechanics
        && why_useful_next_time.is_none()
    {
        return None;
    }

    let scope = scope_from_llm_response(&candidate.scope);
    let mut evidence = vec![ReviewMemoryEvidence::PullRequest {
        number: detail.number,
        title: detail.title.clone(),
        url: detail.url.clone(),
        head_oid: detail.head_ref_oid.clone(),
    }];
    evidence.extend(
        candidate
            .evidence_refs
            .iter()
            .filter_map(|evidence_ref| evidence_from_llm_ref(detail, evidence_ref)),
    );
    dedup_evidence(&mut evidence);
    if evidence.len() <= 1 {
        return None;
    }

    let confidence = match candidate.confidence {
        ReviewMemoryConfidence::High => ReviewMemoryConfidence::Medium,
        ReviewMemoryConfidence::Medium => ReviewMemoryConfidence::Medium,
        ReviewMemoryConfidence::Low => ReviewMemoryConfidence::Low,
    };
    let mut normalized_tags = vec![
        normalize_tag(candidate.kind.label()),
        normalize_tag(candidate.reading_level.label()),
        normalize_tag("candidate"),
    ];
    if let Some(path) = scope.path() {
        normalized_tags.push(normalize_tag(path));
    }
    if let Some(symbol) = scope.symbol_name() {
        normalized_tags.push(normalize_tag(symbol));
    }
    normalized_tags.extend(
        candidate
            .normalized_tags
            .iter()
            .map(|tag| normalize_tag(tag))
            .filter(|tag| !tag.trim().is_empty()),
    );
    normalized_tags.sort();
    normalized_tags.dedup();

    Some(ReviewMemoryEntry {
        id: stable_candidate_entry_id(
            &detail.repository,
            detail.number,
            &candidate.reading_level,
            &candidate.kind,
            &scope,
            &statement,
        ),
        repository: detail.repository.clone(),
        kind: candidate.kind.clone(),
        reading_level: candidate.reading_level.clone(),
        origin: ReviewMemoryOrigin::LlmCandidate,
        scope,
        statement,
        why_useful_next_time,
        normalized_tags,
        evidence,
        confidence,
        status: ReviewMemoryStatus::Candidate,
        created_at: detail.updated_at.clone(),
        updated_at: detail.updated_at.clone(),
        first_seen_pr: Some(detail.number),
        last_seen_pr: Some(detail.number),
        valid_from_oid: detail.head_ref_oid.clone(),
        valid_until_oid: None,
        stale_reason: None,
    })
}

fn entry_from_review_thread(
    detail: &PullRequestDetail,
    thread: &PullRequestReviewThread,
) -> Option<ReviewMemoryEntry> {
    if thread.comments.is_empty() || thread.path.trim().is_empty() {
        return None;
    }

    let excerpt = thread_excerpt(thread)?;
    let symbol = symbol_for_thread(detail, thread);
    let scope = if let Some(symbol) = symbol.as_ref() {
        ReviewMemoryScope::Symbol {
            path: thread.path.clone(),
            name: symbol.name.clone(),
            kind: symbol.kind.clone(),
        }
    } else {
        ReviewMemoryScope::File {
            path: thread.path.clone(),
        }
    };
    let line = thread.line.or(thread.original_line);
    let status = if thread.is_resolved {
        ReviewMemoryStatus::Resolved
    } else {
        ReviewMemoryStatus::Open
    };
    let kind = if thread.is_resolved {
        ReviewMemoryKind::ResolvedQuestion
    } else {
        ReviewMemoryKind::ReviewConcern
    };
    let status_word = if thread.is_resolved {
        "resolved"
    } else {
        "open"
    };
    let scope_label = symbol
        .as_ref()
        .map(|symbol| symbol.name.clone())
        .unwrap_or_else(|| thread.path.clone());
    let statement = format!(
        "{} review thread on {} discussed: {}",
        capitalize(status_word),
        scope_label,
        excerpt
    );
    let created_at = thread
        .comments
        .first()
        .map(|comment| comment.created_at.clone())
        .unwrap_or_else(|| detail.updated_at.clone());
    let updated_at = thread
        .comments
        .iter()
        .filter_map(|comment| {
            (!comment.updated_at.trim().is_empty()).then(|| comment.updated_at.clone())
        })
        .max()
        .unwrap_or_else(|| detail.updated_at.clone());
    let comment_ids = thread
        .comments
        .iter()
        .map(|comment| comment.id.clone())
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    let hunk_header = hunk_for_thread(detail, thread).map(|hunk| hunk.header.clone());
    let mut tags = vec![
        normalize_tag(kind.label()),
        normalize_tag(status_word),
        normalize_tag(&thread.path),
    ];
    if let Some(symbol) = symbol.as_ref() {
        tags.push(normalize_tag(&symbol.name));
    }
    tags.sort();
    tags.dedup();

    let id = stable_entry_id(&detail.repository, detail.number, &thread.id, &scope, &kind);

    Some(ReviewMemoryEntry {
        id,
        repository: detail.repository.clone(),
        kind,
        reading_level: ReviewMemoryReadingLevel::History,
        origin: ReviewMemoryOrigin::Deterministic,
        scope,
        statement,
        why_useful_next_time: Some(
            "Future reviews touching this area should compare the current change with this prior review discussion."
                .to_string(),
        ),
        normalized_tags: tags,
        evidence: vec![
            ReviewMemoryEvidence::PullRequest {
                number: detail.number,
                title: detail.title.clone(),
                url: detail.url.clone(),
                head_oid: detail.head_ref_oid.clone(),
            },
            ReviewMemoryEvidence::ReviewThread {
                pr_number: detail.number,
                path: thread.path.clone(),
                line,
                resolved: thread.is_resolved,
                excerpt,
                comment_ids,
            },
            ReviewMemoryEvidence::DiffHunk {
                pr_number: detail.number,
                path: thread.path.clone(),
                hunk_header,
            },
        ],
        confidence: ReviewMemoryConfidence::High,
        status,
        created_at,
        updated_at,
        first_seen_pr: Some(detail.number),
        last_seen_pr: Some(detail.number),
        valid_from_oid: detail.head_ref_oid.clone(),
        valid_until_oid: None,
        stale_reason: None,
    })
}

fn score_entry(
    entry: &ReviewMemoryEntry,
    detail: &PullRequestDetail,
    targets: &[ReviewMemoryTarget],
    target_paths: &BTreeSet<&str>,
) -> i64 {
    let mut score = match &entry.scope {
        ReviewMemoryScope::Symbol { path, name, .. } => targets
            .iter()
            .filter(|target| target.path == *path)
            .filter(|target| {
                target
                    .symbol_name
                    .as_deref()
                    .map(|symbol| symbols_match(symbol, name))
                    .unwrap_or(false)
            })
            .map(|_| 120)
            .max()
            .unwrap_or_else(|| {
                target_paths
                    .contains(path.as_str())
                    .then_some(70)
                    .unwrap_or(0)
            }),
        ReviewMemoryScope::File { path } => target_paths
            .contains(path.as_str())
            .then_some(80)
            .unwrap_or(0),
        ReviewMemoryScope::Directory { path } => target_paths
            .iter()
            .any(|target| target.starts_with(path))
            .then_some(45)
            .unwrap_or(0),
        ReviewMemoryScope::Repository => 20,
        ReviewMemoryScope::StackLayerPattern { title } => {
            let normalized_title = normalize_tag(title);
            targets
                .iter()
                .any(|target| {
                    target
                        .symbol_name
                        .as_deref()
                        .map(|symbol| normalize_tag(symbol).contains(&normalized_title))
                        .unwrap_or(false)
                })
                .then_some(35)
                .unwrap_or(0)
        }
        ReviewMemoryScope::TestTarget { path, symbol } => targets
            .iter()
            .filter(|target| target.path == *path)
            .filter(|target| {
                target
                    .symbol_name
                    .as_deref()
                    .map(|name| symbols_match(name, symbol))
                    .unwrap_or(false)
            })
            .map(|_| 95)
            .max()
            .unwrap_or(0),
    };

    if score == 0 {
        return 0;
    }

    if entry.last_seen_pr == Some(detail.number) {
        score += 15;
    } else if entry
        .last_seen_pr
        .map(|number| number < detail.number)
        .unwrap_or(false)
    {
        score += 8;
    }

    score += match entry.status {
        ReviewMemoryStatus::Open => 20,
        ReviewMemoryStatus::Resolved => 10,
        ReviewMemoryStatus::Promoted => 25,
        ReviewMemoryStatus::Candidate => 5,
        ReviewMemoryStatus::Stale | ReviewMemoryStatus::Hidden => 0,
    };

    score += match entry.confidence {
        ReviewMemoryConfidence::High => 12,
        ReviewMemoryConfidence::Medium => 6,
        ReviewMemoryConfidence::Low => 0,
    };

    score
}

fn signal_from_entry(entry: &ReviewMemoryEntry) -> ReviewMemorySignal {
    let (path, line) = entry_location(entry);
    ReviewMemorySignal {
        id: entry.id.clone(),
        kind: entry.kind.clone(),
        reading_level: entry.reading_level.clone(),
        origin: entry.origin.clone(),
        scope: entry.scope.clone(),
        statement: entry.statement.clone(),
        why_useful_next_time: entry.why_useful_next_time.clone(),
        evidence_summary: evidence_summary(entry),
        confidence: entry.confidence.clone(),
        status: entry.status.clone(),
        first_seen_pr: entry.first_seen_pr,
        last_seen_pr: entry.last_seen_pr,
        path,
        line,
    }
}

fn evidence_summary(entry: &ReviewMemoryEntry) -> String {
    entry
        .evidence
        .iter()
        .find_map(|evidence| match evidence {
            ReviewMemoryEvidence::ReviewThread {
                pr_number,
                path,
                line,
                resolved,
                ..
            } => Some(format!(
                "PR #{pr_number} review thread on {}{}, {}",
                path,
                line.map(|line| format!(":{line}")).unwrap_or_default(),
                if *resolved { "resolved" } else { "unresolved" }
            )),
            _ => None,
        })
        .or_else(|| {
            entry.evidence.iter().find_map(|evidence| match evidence {
                ReviewMemoryEvidence::PullRequest { number, title, .. } => {
                    Some(format!("PR #{number}: {title}"))
                }
                ReviewMemoryEvidence::PullRequestBody { number, .. } => {
                    Some(format!("PR #{number} author description"))
                }
                _ => None,
            })
        })
        .unwrap_or_else(|| format!("Evidence attached to {}", entry.scope.label()))
}

fn entry_location(entry: &ReviewMemoryEntry) -> (Option<String>, Option<i64>) {
    entry
        .evidence
        .iter()
        .find_map(|evidence| match evidence {
            ReviewMemoryEvidence::ReviewThread { path, line, .. } => {
                Some((Some(path.clone()), *line))
            }
            ReviewMemoryEvidence::DiffHunk { path, .. } => Some((Some(path.clone()), None)),
            _ => None,
        })
        .unwrap_or_else(|| (entry.scope.path().map(str::to_string), None))
}

fn thread_excerpt(thread: &PullRequestReviewThread) -> Option<String> {
    let text = thread
        .comments
        .iter()
        .map(|comment| comment.body.trim())
        .filter(|body| !body.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let excerpt = trim_text(&normalize_whitespace(&text), MAX_EXCERPT_CHARS);
    (!excerpt.is_empty()).then_some(excerpt)
}

fn symbol_for_thread(
    detail: &PullRequestDetail,
    thread: &PullRequestReviewThread,
) -> Option<MemorySymbol> {
    hunk_for_thread(detail, thread).and_then(|hunk| symbol_from_hunk_header(&hunk.header))
}

fn hunk_for_thread<'a>(
    detail: &'a PullRequestDetail,
    thread: &PullRequestReviewThread,
) -> Option<&'a ParsedDiffHunk> {
    let line = thread.line.or(thread.original_line)?;
    let parsed = detail
        .parsed_diff
        .iter()
        .find(|file| file.path == thread.path)?;
    hunk_containing_line(parsed, line)
}

fn hunk_containing_line(parsed: &ParsedDiffFile, line: i64) -> Option<&ParsedDiffHunk> {
    parsed.hunks.iter().find(|hunk| {
        hunk.lines.iter().any(|diff_line| {
            matches!(
                diff_line.kind,
                DiffLineKind::Addition | DiffLineKind::Context
            ) && diff_line.right_line_number == Some(line)
        }) || hunk.lines.iter().any(|diff_line| {
            matches!(
                diff_line.kind,
                DiffLineKind::Deletion | DiffLineKind::Context
            ) && diff_line.left_line_number == Some(line)
        })
    })
}

fn scope_from_llm_response(scope: &LlmReviewMemoryScopeResponse) -> ReviewMemoryScope {
    let scope_type = scope.scope_type.trim();
    match scope_type {
        "directory" => scope
            .path
            .as_deref()
            .map(normalize_memory_path)
            .filter(|path| !path.is_empty())
            .map(|path| ReviewMemoryScope::Directory { path })
            .unwrap_or(ReviewMemoryScope::Repository),
        "file" => scope
            .path
            .as_deref()
            .map(normalize_memory_path)
            .filter(|path| !path.is_empty())
            .map(|path| ReviewMemoryScope::File { path })
            .unwrap_or(ReviewMemoryScope::Repository),
        "symbol" => {
            let path = scope
                .path
                .as_deref()
                .map(normalize_memory_path)
                .unwrap_or_default();
            let name = scope
                .name
                .as_deref()
                .or(scope.symbol.as_deref())
                .map(normalize_whitespace)
                .unwrap_or_default();
            if path.is_empty() {
                ReviewMemoryScope::Repository
            } else if name.is_empty() {
                ReviewMemoryScope::File { path }
            } else {
                ReviewMemoryScope::Symbol {
                    path,
                    name,
                    kind: scope
                        .kind
                        .as_deref()
                        .map(normalize_whitespace)
                        .filter(|kind| !kind.is_empty())
                        .unwrap_or_else(|| "symbol".to_string()),
                }
            }
        }
        "stackLayerPattern" => scope
            .title
            .as_deref()
            .map(normalize_whitespace)
            .filter(|title| !title.is_empty())
            .map(|title| ReviewMemoryScope::StackLayerPattern { title })
            .unwrap_or(ReviewMemoryScope::Repository),
        "testTarget" => {
            let path = scope
                .path
                .as_deref()
                .map(normalize_memory_path)
                .unwrap_or_default();
            let symbol = scope
                .symbol
                .as_deref()
                .or(scope.name.as_deref())
                .map(normalize_whitespace)
                .unwrap_or_default();
            if path.is_empty() {
                ReviewMemoryScope::Repository
            } else if symbol.is_empty() {
                ReviewMemoryScope::File { path }
            } else {
                ReviewMemoryScope::TestTarget { path, symbol }
            }
        }
        _ => ReviewMemoryScope::Repository,
    }
}

fn evidence_from_llm_ref(
    detail: &PullRequestDetail,
    evidence_ref: &LlmReviewMemoryEvidenceRefResponse,
) -> Option<ReviewMemoryEvidence> {
    match evidence_ref.source.trim() {
        "pr_body" | "author_body" | "pull_request_body" => {
            let excerpt = evidence_ref
                .excerpt
                .as_deref()
                .map(normalize_whitespace)
                .filter(|excerpt| !excerpt.is_empty())
                .unwrap_or_else(|| normalize_whitespace(&detail.body));
            (!excerpt.is_empty()).then(|| ReviewMemoryEvidence::PullRequestBody {
                number: detail.number,
                excerpt: trim_text(&excerpt, MAX_EXCERPT_CHARS),
            })
        }
        "review_thread" => {
            let thread = matching_thread(detail, evidence_ref)?;
            let excerpt = evidence_ref
                .excerpt
                .as_deref()
                .map(normalize_whitespace)
                .filter(|excerpt| !excerpt.is_empty())
                .or_else(|| thread_excerpt(thread))?;
            let comment_ids = thread
                .comments
                .iter()
                .map(|comment| comment.id.clone())
                .filter(|id| !id.trim().is_empty())
                .collect::<Vec<_>>();
            Some(ReviewMemoryEvidence::ReviewThread {
                pr_number: detail.number,
                path: thread.path.clone(),
                line: evidence_ref.line.or(thread.line).or(thread.original_line),
                resolved: thread.is_resolved,
                excerpt: trim_text(&excerpt, MAX_EXCERPT_CHARS),
                comment_ids,
            })
        }
        "review_comment" | "thread_comment" | "pr_comment" => {
            let (author_login, excerpt) = matching_comment(detail, evidence_ref)?;
            Some(ReviewMemoryEvidence::ReviewComment {
                pr_number: detail.number,
                author_login,
                excerpt,
            })
        }
        "diff_hunk" | "diff" => {
            let path = evidence_ref
                .path
                .as_deref()
                .map(normalize_memory_path)
                .filter(|path| !path.is_empty())?;
            let hunk_header = evidence_ref
                .hunk_header
                .as_ref()
                .map(|header| trim_text(&normalize_whitespace(header), MAX_EXCERPT_CHARS))
                .filter(|header| !header.is_empty())
                .or_else(|| hunk_header_for_ref(detail, &path, evidence_ref.line));
            Some(ReviewMemoryEvidence::DiffHunk {
                pr_number: detail.number,
                path,
                hunk_header,
            })
        }
        _ => None,
    }
}

fn matching_thread<'a>(
    detail: &'a PullRequestDetail,
    evidence_ref: &LlmReviewMemoryEvidenceRefResponse,
) -> Option<&'a PullRequestReviewThread> {
    evidence_ref
        .thread_id
        .as_deref()
        .and_then(|thread_id| {
            detail
                .review_threads
                .iter()
                .find(|thread| thread.id == thread_id)
        })
        .or_else(|| {
            let path = evidence_ref.path.as_deref()?;
            detail.review_threads.iter().find(|thread| {
                thread.path == path
                    && evidence_ref
                        .line
                        .map(|line| thread.line == Some(line) || thread.original_line == Some(line))
                        .unwrap_or(true)
            })
        })
}

fn matching_comment(
    detail: &PullRequestDetail,
    evidence_ref: &LlmReviewMemoryEvidenceRefResponse,
) -> Option<(String, String)> {
    if let Some(comment_id) = evidence_ref.comment_id.as_deref() {
        if let Some(comment) = detail
            .comments
            .iter()
            .find(|comment| comment.id == comment_id)
        {
            let excerpt = trim_text(&normalize_whitespace(&comment.body), MAX_EXCERPT_CHARS);
            if !excerpt.is_empty() {
                return Some((comment.author_login.clone(), excerpt));
            }
        }

        if let Some(comment) = detail
            .review_threads
            .iter()
            .flat_map(|thread| thread.comments.iter())
            .find(|comment| comment.id == comment_id)
        {
            let excerpt = trim_text(&normalize_whitespace(&comment.body), MAX_EXCERPT_CHARS);
            if !excerpt.is_empty() {
                return Some((comment.author_login.clone(), excerpt));
            }
        }
    }

    let excerpt = evidence_ref
        .excerpt
        .as_deref()
        .map(normalize_whitespace)
        .map(|excerpt| trim_text(&excerpt, MAX_EXCERPT_CHARS))
        .filter(|excerpt| !excerpt.is_empty())?;
    Some((
        evidence_ref
            .author_login
            .as_deref()
            .map(normalize_whitespace)
            .filter(|author| !author.is_empty())
            .unwrap_or_else(|| "reviewer".to_string()),
        excerpt,
    ))
}

fn hunk_header_for_ref(
    detail: &PullRequestDetail,
    path: &str,
    line: Option<i64>,
) -> Option<String> {
    let parsed = detail.parsed_diff.iter().find(|file| file.path == path)?;
    let hunk = line
        .and_then(|line| hunk_containing_line(parsed, line))
        .or_else(|| parsed.hunks.first())?;
    Some(trim_text(
        &normalize_whitespace(&hunk.header),
        MAX_EXCERPT_CHARS,
    ))
}

fn dedup_evidence(evidence: &mut Vec<ReviewMemoryEvidence>) {
    let mut seen = BTreeSet::<String>::new();
    evidence.retain(|item| seen.insert(format!("{item:?}")));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MemorySymbol {
    name: String,
    kind: String,
}

fn symbol_from_hunk_header(header: &str) -> Option<MemorySymbol> {
    let context = header
        .split("@@")
        .last()
        .map(str::trim)
        .filter(|context| !context.is_empty())?;
    symbol_from_code_context(context)
}

fn symbol_from_code_context(context: &str) -> Option<MemorySymbol> {
    let trimmed = context
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("export ")
        .trim_start_matches("default ");

    for (prefix, kind) in [
        ("fn ", "function"),
        ("struct ", "struct"),
        ("enum ", "enum"),
        ("trait ", "trait"),
        ("impl ", "impl"),
        ("mod ", "module"),
        ("type ", "type"),
        ("class ", "class"),
        ("interface ", "interface"),
        ("function ", "function"),
        ("def ", "function"),
        ("func ", "function"),
    ] {
        if let Some(name) = extract_after_pattern(trimmed, prefix) {
            return Some(MemorySymbol {
                name,
                kind: kind.to_string(),
            });
        }
    }

    None
}

fn extract_after_pattern(value: &str, pattern: &str) -> Option<String> {
    let (_, rest) = value.split_once(pattern)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let name = if pattern == "impl " {
        rest.split('{').next().unwrap_or(rest).trim()
    } else {
        rest.split(['(', '{', '<', ':', '=', ' '])
            .next()
            .unwrap_or(rest)
            .trim()
    };
    let clean = name
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != ':')
        .to_string();
    (!clean.is_empty()).then_some(clean)
}

fn stable_entry_id(
    repository: &str,
    pr_number: i64,
    thread_id: &str,
    scope: &ReviewMemoryScope,
    kind: &ReviewMemoryKind,
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(repository.as_bytes());
    hasher.update(pr_number.to_string().as_bytes());
    hasher.update(thread_id.as_bytes());
    hasher.update(format!("{scope:?}{kind:?}").as_bytes());
    format!("{:x}", hasher.finalize())
        .chars()
        .take(16)
        .collect()
}

fn stable_candidate_entry_id(
    repository: &str,
    pr_number: i64,
    reading_level: &ReviewMemoryReadingLevel,
    kind: &ReviewMemoryKind,
    scope: &ReviewMemoryScope,
    statement: &str,
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(repository.as_bytes());
    hasher.update(pr_number.to_string().as_bytes());
    hasher.update(format!("{reading_level:?}{kind:?}{scope:?}").as_bytes());
    hasher.update(normalize_tag(statement).as_bytes());
    format!("{:x}", hasher.finalize())
        .chars()
        .take(16)
        .collect()
}

fn detail_code_version_key(detail: &PullRequestDetail) -> String {
    detail
        .head_ref_oid
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("head-{value}"))
        .unwrap_or_else(|| {
            let mut hasher = Sha1::new();
            hasher.update(detail.raw_diff.as_bytes());
            format!("{:x}", hasher.finalize())
                .chars()
                .take(16)
                .collect::<String>()
        })
}

fn normalize_memory_path(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("a/")
        .trim_start_matches("b/")
        .replace('\\', "/")
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_text(value: &str, max_chars: usize) -> String {
    let normalized = value.trim();
    if normalized.chars().count() <= max_chars {
        return normalized.to_string();
    }

    let truncated = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>()
        .trim_end()
        .to_string();
    format!("{truncated}...")
}

fn normalize_tag(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn symbols_match(left: &str, right: &str) -> bool {
    let left = normalize_symbol_name(left);
    let right = normalize_symbol_name(right);
    left == right || left.ends_with(&format!("::{right}")) || right.ends_with(&format!("::{left}"))
}

fn normalize_symbol_name(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("fn ")
        .trim_start_matches("struct ")
        .trim_start_matches("enum ")
        .trim_start_matches("trait ")
        .trim_start_matches("impl ")
        .trim_start_matches("type ")
        .trim_start_matches("class ")
        .trim_start_matches("interface ")
        .trim_start_matches("function ")
        .trim_start_matches("def ")
        .trim_start_matches("func ")
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != ':')
        .to_ascii_lowercase()
}

fn dedup_targets(targets: &mut Vec<ReviewMemoryTarget>) {
    let mut seen = BTreeSet::<(String, Option<String>)>::new();
    targets.retain(|target| {
        if target.path.trim().is_empty() {
            return false;
        }
        seen.insert((target.path.clone(), target.symbol_name.clone()))
    });
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
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
        diff::{parse_unified_diff, ParsedDiffLine},
        github::{
            PullRequestComment, PullRequestDataCompleteness, PullRequestFile,
            PullRequestReviewComment,
        },
    };

    fn detail_with_thread(resolved: bool) -> PullRequestDetail {
        let raw_diff = r#"diff --git a/src/review_partner.rs b/src/review_partner.rs
--- a/src/review_partner.rs
+++ b/src/review_partner.rs
@@ -10,3 +10,4 @@ fn build_review_partner_prompt(
 pub fn build_review_partner_prompt() {
-    old();
+    new();
 }
"#;
        PullRequestDetail {
            id: "pr123".to_string(),
            repository: "rikuws/remiss".to_string(),
            number: 123,
            title: "Tighten Review Partner prompt".to_string(),
            body: "Avoid checklist phrasing. Fixes #42.".to_string(),
            url: "https://github.com/rikuws/remiss/pull/123".to_string(),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "prompt-memory".to_string(),
            base_ref_oid: Some("base".to_string()),
            head_ref_oid: Some("head".to_string()),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            commits: Vec::new(),
            created_at: "2026-05-18T10:00:00Z".to_string(),
            updated_at: "2026-05-18T10:30:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: BTreeMap::new(),
            comments: vec![PullRequestComment {
                id: "c1".to_string(),
                author_login: "reviewer".to_string(),
                author_avatar_url: None,
                body: "Connects to rikuws/remiss#42.".to_string(),
                created_at: "2026-05-18T10:05:00Z".to_string(),
                updated_at: "2026-05-18T10:05:00Z".to_string(),
                url: "https://github.com/rikuws/remiss/pull/123#issuecomment-1".to_string(),
            }],
            latest_reviews: Vec::new(),
            review_threads: vec![PullRequestReviewThread {
                id: "thread-1".to_string(),
                path: "src/review_partner.rs".to_string(),
                line: Some(10),
                original_line: None,
                start_line: None,
                original_start_line: None,
                diff_side: "RIGHT".to_string(),
                start_diff_side: None,
                is_collapsed: false,
                is_outdated: false,
                is_resolved: resolved,
                subject_type: "LINE".to_string(),
                resolved_by_login: resolved.then(|| "octocat".to_string()),
                viewer_can_reply: true,
                viewer_can_resolve: true,
                viewer_can_unresolve: true,
                comments: vec![PullRequestReviewComment {
                    id: "rt1".to_string(),
                    author_login: "reviewer".to_string(),
                    author_avatar_url: None,
                    body: "This summary reads like a review task instead of explanation."
                        .to_string(),
                    path: "src/review_partner.rs".to_string(),
                    line: Some(10),
                    original_line: None,
                    start_line: None,
                    original_start_line: None,
                    state: "SUBMITTED".to_string(),
                    created_at: "2026-05-18T10:10:00Z".to_string(),
                    updated_at: "2026-05-18T10:20:00Z".to_string(),
                    published_at: None,
                    reply_to_id: None,
                    viewer_can_update: false,
                    viewer_can_delete: false,
                    url: "https://github.com/rikuws/remiss/pull/123#discussion_r1".to_string(),
                }],
            }],
            viewer_pending_review: None,
            files: vec![PullRequestFile {
                path: "src/review_partner.rs".to_string(),
                additions: 1,
                deletions: 1,
                change_type: "MODIFIED".to_string(),
            }],
            raw_diff: raw_diff.to_string(),
            parsed_diff: parse_unified_diff(raw_diff),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }

    #[test]
    fn extracts_review_thread_memory_with_symbol_scope() {
        let detail = detail_with_thread(false);
        let entries = extract_review_memory_entries(&detail);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, ReviewMemoryKind::ReviewConcern);
        assert_eq!(entries[0].status, ReviewMemoryStatus::Open);
        assert_eq!(entries[0].reading_level, ReviewMemoryReadingLevel::History);
        assert_eq!(entries[0].origin, ReviewMemoryOrigin::Deterministic);
        assert!(entries[0].statement.contains("review task"));
        assert!(matches!(
            entries[0].scope,
            ReviewMemoryScope::Symbol { ref name, .. } if name == "build_review_partner_prompt"
        ));
    }

    #[test]
    fn retrieves_precise_memory_signals_for_touched_symbol() {
        let detail = detail_with_thread(true);
        let mut entry = extract_review_memory_entries(&detail).pop().expect("entry");
        entry.first_seen_pr = Some(122);
        entry.last_seen_pr = Some(122);
        let document = ReviewMemoryDocument {
            version: REVIEW_MEMORY_DOCUMENT_VERSION.to_string(),
            repository: detail.repository.clone(),
            entries: vec![entry],
        };
        let context = review_memory_prompt_context(&document, &detail, &[], 3);

        assert_eq!(context.signals.len(), 1);
        assert_eq!(context.signals[0].status, ReviewMemoryStatus::Resolved);
        assert!(context.signals[0]
            .evidence_summary
            .contains("PR #123 review thread"));
    }

    #[test]
    fn excludes_memory_from_current_pull_request() {
        let detail = detail_with_thread(true);
        let document = ReviewMemoryDocument {
            version: REVIEW_MEMORY_DOCUMENT_VERSION.to_string(),
            repository: detail.repository.clone(),
            entries: extract_review_memory_entries(&detail),
        };
        let context = review_memory_prompt_context(&document, &detail, &[], 3);

        assert!(context.signals.is_empty());
        assert!(context.limitations[0].contains("current-pull-request memory is excluded"));
    }

    #[test]
    fn ignores_unrelated_file_memory() {
        let detail = detail_with_thread(false);
        let mut entry = extract_review_memory_entries(&detail).pop().expect("entry");
        entry.scope = ReviewMemoryScope::File {
            path: "src/other.rs".to_string(),
        };
        let document = ReviewMemoryDocument {
            version: REVIEW_MEMORY_DOCUMENT_VERSION.to_string(),
            repository: detail.repository.clone(),
            entries: vec![entry],
        };

        let context = review_memory_prompt_context(&document, &detail, &[], 3);
        assert!(context.signals.is_empty());
    }

    #[test]
    fn hunk_symbol_extraction_handles_basic_rust_headers() {
        let parsed = ParsedDiffFile {
            path: "src/lib.rs".to_string(),
            previous_path: None,
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,2 +1,2 @@ pub async fn load_user(".to_string(),
                lines: vec![ParsedDiffLine {
                    kind: DiffLineKind::Addition,
                    prefix: "+".to_string(),
                    left_line_number: None,
                    right_line_number: Some(1),
                    content: "new".to_string(),
                }],
            }],
        };
        let symbol = symbol_from_hunk_header(&parsed.hunks[0].header).expect("symbol");
        assert_eq!(symbol.name, "load_user");
        assert_eq!(symbol.kind, "function");
    }

    #[test]
    fn llm_candidate_entries_store_reading_level_origin_and_evidence() {
        let detail = detail_with_thread(false);
        let response = serde_json::from_value::<LlmReviewMemoryResponse>(json!({
            "candidates": [{
                "kind": "known_risk",
                "readingLevel": "behavior",
                "scope": {
                    "type": "symbol",
                    "path": "src/review_partner.rs",
                    "name": "build_review_partner_prompt",
                    "kind": "function"
                },
                "statement": "Review Partner prompt changes can shift the right rail from explanation into task assignment.",
                "whyUsefulNextTime": "Future prompt edits should check whether generated rows remain explanations.",
                "confidence": "high",
                "normalizedTags": ["review partner", "prompt"],
                "evidenceRefs": [
                    { "source": "pr_body", "excerpt": "Avoid checklist phrasing." },
                    { "source": "review_thread", "threadId": "thread-1" },
                    {
                        "source": "diff_hunk",
                        "path": "src/review_partner.rs",
                        "hunkHeader": "@@ -10,3 +10,4 @@ fn build_review_partner_prompt("
                    }
                ]
            }]
        }))
        .expect("response");
        let entries = entries_from_llm_response(&detail, &response.candidates);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, ReviewMemoryKind::KnownRisk);
        assert_eq!(entries[0].reading_level, ReviewMemoryReadingLevel::Behavior);
        assert_eq!(entries[0].origin, ReviewMemoryOrigin::LlmCandidate);
        assert_eq!(entries[0].status, ReviewMemoryStatus::Candidate);
        assert_eq!(entries[0].confidence, ReviewMemoryConfidence::Medium);
        assert!(entries[0]
            .why_useful_next_time
            .as_deref()
            .unwrap_or_default()
            .contains("Future prompt edits"));
        assert!(entries[0]
            .evidence
            .iter()
            .any(|evidence| matches!(evidence, ReviewMemoryEvidence::PullRequestBody { .. })));
    }

    #[test]
    fn llm_candidate_entries_drop_unsupported_mechanics() {
        let detail = detail_with_thread(false);
        let response = serde_json::from_value::<LlmReviewMemoryResponse>(json!({
            "candidates": [{
                "kind": "design_decision",
                "readingLevel": "mechanics",
                "scope": {
                    "type": "file",
                    "path": "src/review_partner.rs"
                },
                "statement": "The prompt added another sentence.",
                "confidence": "medium",
                "evidenceRefs": [
                    {
                        "source": "diff_hunk",
                        "path": "src/review_partner.rs",
                        "hunkHeader": "@@ -10,3 +10,4 @@ fn build_review_partner_prompt("
                    }
                ]
            }]
        }))
        .expect("response");

        assert!(entries_from_llm_response(&detail, &response.candidates).is_empty());
    }
}
