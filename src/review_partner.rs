use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::Path,
    process::Command,
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tree_sitter::{Node, Parser};

use crate::{
    agents::{self, jsonrepair::parse_tolerant, AgentJsonPromptOptions},
    cache::CacheStore,
    code_tour::{
        find_parsed_diff_file, tour_code_version_key, CodeTourProgressUpdate, CodeTourProvider,
        CodeTourPullRequestCommentContext, CodeTourReviewCommentContext, CodeTourReviewContext,
        CodeTourReviewThreadContext,
    },
    diff::{DiffLineKind, ParsedDiffFile},
    github::{PullRequestComment, PullRequestDetail, PullRequestReview, PullRequestReviewThread},
    lsp::{LspSessionManager, LspTextDocumentRequest},
    review_memory::{ReviewMemoryPromptContext, ReviewMemorySignal, ReviewMemoryStatus},
    semantic_review::{
        summarize_semantic_review, RemissSemanticFocusSummary, RemissSemanticLayerSummary,
        RemissSemanticReview, RemissSemanticReviewSummary,
    },
    stacks::model::{
        normalize_stack_layer_title, ChangeAtom, LineRange, ReviewStack, ReviewStackLayer,
        STACK_GENERATOR_VERSION,
    },
    structural_evidence::{StructuralEvidencePack, StructuralEvidenceStatus},
};

mod context;
mod util;

#[cfg(test)]
mod tests;

use self::context::*;
use self::util::*;

pub const REVIEW_PARTNER_GENERATOR_VERSION: &str = "review-partner-v22";
pub const REVIEW_PARTNER_CONTEXT_VERSION: &str = "review-partner-context-v5";

const REVIEW_PARTNER_CACHE_KEY_PREFIX: &str = "review-partner-v22";
const MAX_PARTNER_LAYERS: usize = 24;
const MAX_LAYER_ATOMS: usize = 32;
pub const MAX_FOCUS_RECORDS: usize = 160;
const MAX_FOCUS_TARGET_ATOMS: usize = 8;
const MAX_FOCUS_SECTIONS: usize = 3;
const MAX_FOCUS_TITLE_CHARS: usize = 180;
const MAX_CONTEXT_SYMBOLS_PER_LAYER: usize = 8;
const MAX_REFERENCES_PER_SYMBOL: usize = 8;
const MAX_SIMILAR_LOCATIONS_PER_LAYER: usize = 8;
const MAX_STYLE_NOTES_PER_LAYER: usize = 5;
const MAX_SECTION_ITEMS: usize = 8;
const MAX_COMMENTS_PER_THREAD: usize = 3;
const MAX_BRIEF_TEXT_CHARS: usize = 1200;
const MAX_ITEM_TEXT_CHARS: usize = 260;
const MAX_HISTORY_ITEM_TEXT_CHARS: usize = 1_000;
const MAX_LIMITATION_TEXT_CHARS: usize = 500;
const MAX_PROMPT_SNIPPET_CHARS: usize = 260;
const MAX_RG_LOCATIONS: usize = 18;
const MAX_SCAN_FILES: usize = 450;
const MAX_SCAN_FILE_BYTES: u64 = 280_000;
const MAX_SCAN_DEPTH: usize = 7;
const MAX_EVIDENCE_FILES: usize = 40;
const MAX_EVIDENCE_CHANGES: usize = 80;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedReviewPartnerContext {
    pub provider: CodeTourProvider,
    #[serde(default)]
    pub model: Option<String>,
    pub generated_at_ms: i64,
    pub code_version_key: String,
    pub generator_version: String,
    pub context_version: String,
    pub structural_evidence_version: String,
    pub stack_brief: String,
    #[serde(default)]
    pub stack_concerns: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub stack: ReviewStack,
    pub structural_evidence: StructuralEvidencePack,
    #[serde(default)]
    pub semantic_review: Option<RemissSemanticReviewSummary>,
    #[serde(default)]
    pub review_memory: ReviewMemoryPromptContext,
    pub context: ReviewPartnerContextPack,
    pub layers: Vec<ReviewPartnerLayer>,
    #[serde(default)]
    pub focus_targets: Vec<ReviewPartnerFocusTarget>,
    #[serde(default)]
    pub focus_records: Vec<ReviewPartnerFocusRecord>,
}

impl GeneratedReviewPartnerContext {
    pub fn layer(&self, layer_id: &str) -> Option<&ReviewPartnerLayer> {
        self.layers.iter().find(|layer| layer.layer_id == layer_id)
    }

    pub fn focus_record(&self, key: &str) -> Option<&ReviewPartnerFocusRecord> {
        self.focus_records.iter().find(|record| record.key == key)
    }

    pub fn focus_target(&self, key: &str) -> Option<&ReviewPartnerFocusTarget> {
        self.focus_targets.iter().find(|target| target.key == key)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerFocusTarget {
    pub key: String,
    pub file_path: String,
    #[serde(default)]
    pub hunk_header: Option<String>,
    #[serde(default)]
    pub hunk_index: Option<usize>,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub atom_ids: Vec<String>,
    #[serde(default)]
    pub layer_id: Option<String>,
    pub title: String,
    pub subtitle: String,
    pub match_kind: ReviewPartnerFocusMatchKind,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReviewPartnerFocusMatchKind {
    Layer,
    AtomRange,
    AtomHunk,
    Hunk,
    File,
}

impl ReviewPartnerFocusMatchKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Layer => "Stack layer",
            Self::AtomRange => "Focused change",
            Self::AtomHunk => "Focused hunk",
            Self::Hunk => "Hunk context",
            Self::File => "File context",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerFocusRecord {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub target: ReviewPartnerFocusTarget,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub usage_context: Vec<ReviewPartnerUsageGroup>,
    #[serde(default)]
    pub codebase_fit: ReviewPartnerCodebaseFit,
    #[serde(default)]
    pub sections: Vec<ReviewPartnerFocusSection>,
    #[serde(default)]
    pub understanding_checkpoints: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub assumptions: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub history_signals: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub limitations: Vec<String>,
    pub generated_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerFocusSection {
    pub title: String,
    #[serde(default)]
    pub items: Vec<ReviewPartnerItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerUsageGroup {
    pub symbol: String,
    pub summary: String,
    #[serde(default)]
    pub usages: Vec<ReviewPartnerItem>,
}

impl ReviewPartnerUsageGroup {
    fn new(
        symbol: impl Into<String>,
        summary: impl Into<String>,
        usages: Vec<ReviewPartnerItem>,
    ) -> Self {
        Self {
            symbol: limit_text(symbol.into(), MAX_ITEM_TEXT_CHARS),
            summary: limit_text(summary.into(), MAX_ITEM_TEXT_CHARS),
            usages: usages.into_iter().take(MAX_SECTION_ITEMS).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerCodebaseFit {
    pub follows: bool,
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<ReviewPartnerItem>,
}

impl Default for ReviewPartnerCodebaseFit {
    fn default() -> Self {
        Self {
            follows: true,
            summary: "follows codebase style".to_string(),
            evidence: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerLayer {
    pub layer_id: String,
    pub title: String,
    pub brief: String,
    #[serde(default)]
    pub changed_items: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub removed_items: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub usage_context: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub similar_code: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub codebase_fit: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub concerns: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub limitations: Vec<String>,
    pub structural_evidence_status: StructuralEvidenceStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerItem {
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<usize>,
}

impl ReviewPartnerItem {
    fn new(
        title: impl Into<String>,
        detail: impl Into<String>,
        path: Option<String>,
        line: Option<usize>,
    ) -> Self {
        Self::new_with_limits(title, detail, path, line, MAX_ITEM_TEXT_CHARS)
    }

    fn new_with_limits(
        title: impl Into<String>,
        detail: impl Into<String>,
        path: Option<String>,
        line: Option<usize>,
        detail_max_length: usize,
    ) -> Self {
        Self {
            title: limit_text(title.into(), MAX_ITEM_TEXT_CHARS),
            detail: limit_text(detail.into(), detail_max_length),
            path: path.filter(|path| !path.trim().is_empty()),
            line: line.filter(|line| *line > 0),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerContextPack {
    pub version: String,
    #[serde(default)]
    pub layers: Vec<ReviewPartnerCollectedLayer>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl ReviewPartnerContextPack {
    pub fn empty() -> Self {
        Self {
            version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
            layers: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn layer(&self, layer_id: &str) -> Option<&ReviewPartnerCollectedLayer> {
        self.layers.iter().find(|layer| layer.layer_id == layer_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerCollectedLayer {
    pub layer_id: String,
    #[serde(default)]
    pub semantic_layers: Vec<ReviewPartnerSemanticLayer>,
    #[serde(default)]
    pub semantic_focus: Vec<RemissSemanticFocusSummary>,
    #[serde(default)]
    pub changed_symbols: Vec<ReviewPartnerCollectedSymbol>,
    #[serde(default)]
    pub removed_symbols: Vec<ReviewPartnerCollectedSymbol>,
    #[serde(default)]
    pub similar_locations: Vec<ReviewPartnerLocation>,
    #[serde(default)]
    pub style_notes: Vec<ReviewPartnerItem>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerSemanticLayer {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    #[serde(default)]
    pub atom_ids: Vec<String>,
    #[serde(default)]
    pub file_paths: Vec<String>,
    #[serde(default)]
    pub hunk_indices: Vec<usize>,
    #[serde(default)]
    pub entity_names: Vec<String>,
    pub change_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerCollectedSymbol {
    pub symbol: String,
    pub path: String,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub atom_ids: Vec<String>,
    pub search_strategy: String,
    pub reference_count: usize,
    #[serde(default)]
    pub references: Vec<ReviewPartnerLocation>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPartnerLocation {
    pub path: String,
    pub line: usize,
    #[serde(default)]
    pub snippet: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GenerateReviewPartnerInput {
    pub provider: CodeTourProvider,
    pub working_directory: String,
    pub repository: String,
    pub number: i64,
    pub code_version_key: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub comments: Vec<CodeTourPullRequestCommentContext>,
    pub latest_reviews: Vec<CodeTourReviewContext>,
    pub review_threads: Vec<CodeTourReviewThreadContext>,
    pub stack: ReviewStack,
    pub structural_evidence: StructuralEvidencePack,
    pub semantic_review: Option<RemissSemanticReviewSummary>,
    pub review_memory: ReviewMemoryPromptContext,
    pub context: ReviewPartnerContextPack,
    pub focus_targets: Vec<ReviewPartnerFocusTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerResponse {
    stack_brief: String,
    #[serde(default)]
    stack_concerns: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    limitations: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    layers: Vec<ReviewPartnerLayerResponse>,
    #[serde(default)]
    focus_records: Vec<ReviewPartnerFocusRecordResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerLayerResponse {
    layer_id: String,
    brief: String,
    #[serde(default)]
    changed_items: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    removed_items: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    usage_context: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    similar_code: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    codebase_fit: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    concerns: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    limitations: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerItemResponse {
    title: String,
    detail: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerUsageGroupResponse {
    symbol: String,
    summary: String,
    #[serde(default)]
    usages: Vec<ReviewPartnerItemResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerCodebaseFitResponse {
    follows: bool,
    summary: String,
    #[serde(default)]
    evidence: Vec<ReviewPartnerItemResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerFocusRecordResponse {
    key: String,
    title: String,
    #[serde(default)]
    subtitle: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    usage_context: Vec<ReviewPartnerUsageGroupResponse>,
    #[serde(default)]
    codebase_fit: Option<ReviewPartnerCodebaseFitResponse>,
    #[serde(default)]
    sections: Vec<ReviewPartnerFocusSectionResponse>,
    #[serde(default)]
    understanding_checkpoints: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    assumptions: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    history_signals: Vec<ReviewPartnerItemResponse>,
    #[serde(default)]
    limitations: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerFocusSectionResponse {
    title: String,
    #[serde(default)]
    items: Vec<ReviewPartnerItemResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewPartnerSingleFocusResponse {
    record: ReviewPartnerFocusRecordResponse,
}

pub fn load_review_partner_context(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> Result<Option<GeneratedReviewPartnerContext>, String> {
    let cache_key = review_partner_cache_key(detail, provider);
    Ok(cache
        .get::<GeneratedReviewPartnerContext>(&cache_key)?
        .map(|document| document.value)
        .filter(|document| review_partner_document_matches_current(document, detail, provider)))
}

pub fn save_review_partner_context(
    cache: &CacheStore,
    document: &GeneratedReviewPartnerContext,
) -> Result<(), String> {
    if document.generator_version != REVIEW_PARTNER_GENERATOR_VERSION
        || document.context_version != REVIEW_PARTNER_CONTEXT_VERSION
    {
        return Ok(());
    }

    let cache_key = review_partner_cache_key_from_parts(
        &document.stack.repository,
        document.stack.selected_pr_number,
        document.provider,
        &document.code_version_key,
        &document.stack.generator_version,
        &document.context_version,
    );
    cache.put(&cache_key, document, now_ms())
}

fn review_partner_document_matches_current(
    document: &GeneratedReviewPartnerContext,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> bool {
    document.generator_version == REVIEW_PARTNER_GENERATOR_VERSION
        && document.context_version == REVIEW_PARTNER_CONTEXT_VERSION
        && document.stack.generator_version == STACK_GENERATOR_VERSION
        && document.provider.slug() == provider.slug()
        && document.stack.repository == detail.repository
        && document.stack.selected_pr_number == detail.number
        && document.code_version_key == tour_code_version_key(detail)
}

pub fn generate_review_partner_context(
    cache: &CacheStore,
    input: GenerateReviewPartnerInput,
) -> Result<GeneratedReviewPartnerContext, String> {
    generate_review_partner_context_with_progress(cache, input, &mut |_| {})
}

pub fn generate_review_partner_context_with_progress(
    cache: &CacheStore,
    input: GenerateReviewPartnerInput,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) -> Result<GeneratedReviewPartnerContext, String> {
    if input.working_directory.trim().is_empty() {
        return Err("Review Partner generation requires a local checkout path.".to_string());
    }

    if !Path::new(&input.working_directory).exists() {
        return Err(format!(
            "The local checkout path '{}' does not exist.",
            input.working_directory
        ));
    }

    let prompt = build_review_partner_prompt(&input);
    let response = agents::run_json_prompt_with_options_and_progress(
        input.provider,
        &input.working_directory,
        prompt,
        AgentJsonPromptOptions::review_partner(),
        on_progress,
    )?;
    let parsed = parse_tolerant::<ReviewPartnerResponse>(&response.text)
        .map_err(|error| format!("Failed to parse Review Partner JSON: {}", error.message))?;
    let partner = merge_review_partner(parsed, &input, response.model)?;
    save_review_partner_context(cache, &partner)?;
    Ok(partner)
}

pub fn fallback_review_partner_context(
    input: &GenerateReviewPartnerInput,
    warning: Option<String>,
) -> GeneratedReviewPartnerContext {
    let mut warnings = input.structural_evidence.warnings.clone();
    let fallback_reason = warning.clone();
    if let Some(warning) = warning {
        warnings.push(warning);
    }

    GeneratedReviewPartnerContext {
        provider: input.provider,
        model: None,
        generated_at_ms: now_ms(),
        code_version_key: input.code_version_key.clone(),
        generator_version: REVIEW_PARTNER_GENERATOR_VERSION.to_string(),
        context_version: input.context.version.clone(),
        structural_evidence_version: input.structural_evidence.version.clone(),
        stack_brief: fallback_stack_brief(&input.stack),
        stack_concerns: Vec::new(),
        limitations: input.context.warnings.clone(),
        warnings,
        fallback_reason,
        stack: input.stack.clone(),
        structural_evidence: input.structural_evidence.clone(),
        semantic_review: input.semantic_review.clone(),
        review_memory: input.review_memory.clone(),
        context: input.context.clone(),
        layers: input
            .stack
            .layers
            .iter()
            .map(|layer| fallback_layer(layer, input))
            .collect(),
        focus_targets: input.focus_targets.clone(),
        focus_records: input
            .focus_targets
            .iter()
            .map(|target| fallback_focus_record(input, target, None))
            .collect(),
    }
}

pub fn build_review_partner_generation_input(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
    working_directory: &str,
    stack: ReviewStack,
    structural_evidence: StructuralEvidencePack,
    semantic_review: Option<RemissSemanticReview>,
    lsp_session_manager: Option<Arc<LspSessionManager>>,
) -> GenerateReviewPartnerInput {
    let semantic_review = semantic_review.as_ref().map(summarize_semantic_review);
    let context = collect_review_partner_context(
        detail,
        &stack,
        Path::new(working_directory),
        semantic_review.as_ref(),
        lsp_session_manager.as_deref(),
    );

    let focus_targets = build_review_partner_focus_targets(&stack, &structural_evidence)
        .into_iter()
        .take(MAX_FOCUS_RECORDS)
        .collect();

    GenerateReviewPartnerInput {
        provider,
        working_directory: working_directory.to_string(),
        repository: detail.repository.clone(),
        number: detail.number,
        code_version_key: tour_code_version_key(detail),
        title: detail.title.clone(),
        body: trim_text(&detail.body, 2_500),
        url: detail.url.clone(),
        base_ref_name: detail.base_ref_name.clone(),
        head_ref_name: detail.head_ref_name.clone(),
        comments: detail
            .comments
            .iter()
            .take(MAX_PARTNER_LAYERS)
            .map(map_comment_context)
            .collect(),
        latest_reviews: detail
            .latest_reviews
            .iter()
            .take(MAX_PARTNER_LAYERS)
            .map(map_review_context)
            .collect(),
        review_threads: prioritize_review_threads(&detail.review_threads)
            .into_iter()
            .take(MAX_PARTNER_LAYERS)
            .map(|thread| map_thread_context(&thread))
            .collect(),
        stack,
        structural_evidence,
        semantic_review,
        review_memory: ReviewMemoryPromptContext::default(),
        context,
        focus_targets,
    }
}

pub fn build_review_partner_request_key(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> String {
    format!(
        "{}:{}#{}:{}:{}:{}:{}",
        provider.slug(),
        detail.repository,
        detail.number,
        tour_code_version_key(detail),
        REVIEW_PARTNER_GENERATOR_VERSION,
        STACK_GENERATOR_VERSION,
        REVIEW_PARTNER_CONTEXT_VERSION,
    )
}

fn map_comment_context(comment: &PullRequestComment) -> CodeTourPullRequestCommentContext {
    CodeTourPullRequestCommentContext {
        author_login: comment.author_login.clone(),
        body: trim_text(&comment.body, MAX_PROMPT_SNIPPET_CHARS),
        created_at: comment.created_at.clone(),
    }
}

fn map_review_context(review: &PullRequestReview) -> CodeTourReviewContext {
    CodeTourReviewContext {
        author_login: review.author_login.clone(),
        state: review.state.clone(),
        body: trim_text(&review.body, MAX_PROMPT_SNIPPET_CHARS),
        submitted_at: review.submitted_at.clone(),
    }
}

fn map_thread_context(thread: &PullRequestReviewThread) -> CodeTourReviewThreadContext {
    CodeTourReviewThreadContext {
        path: thread.path.clone(),
        line: thread.line.or(thread.original_line),
        diff_side: if thread.diff_side.trim().is_empty() {
            thread.start_diff_side.clone()
        } else {
            Some(thread.diff_side.clone())
        },
        is_resolved: thread.is_resolved,
        subject_type: thread.subject_type.clone(),
        comments: thread
            .comments
            .iter()
            .take(MAX_COMMENTS_PER_THREAD)
            .map(|comment| CodeTourReviewCommentContext {
                author_login: comment.author_login.clone(),
                body: trim_text(&comment.body, MAX_PROMPT_SNIPPET_CHARS),
            })
            .collect(),
    }
}

fn prioritize_review_threads(threads: &[PullRequestReviewThread]) -> Vec<PullRequestReviewThread> {
    let mut prioritized = threads.to_vec();
    prioritized.sort_by_key(|thread| thread.is_resolved);
    prioritized
}

pub fn review_partner_cache_key(detail: &PullRequestDetail, provider: CodeTourProvider) -> String {
    review_partner_cache_key_from_parts(
        &detail.repository,
        detail.number,
        provider,
        &tour_code_version_key(detail),
        STACK_GENERATOR_VERSION,
        REVIEW_PARTNER_CONTEXT_VERSION,
    )
}

pub fn review_partner_cache_key_from_parts(
    repository: &str,
    number: i64,
    provider: CodeTourProvider,
    code_version: &str,
    stack_version: &str,
    context_version: &str,
) -> String {
    format!(
        "{REVIEW_PARTNER_CACHE_KEY_PREFIX}:{}:{}:{}:{}:{}:{}",
        provider.slug(),
        repository,
        number,
        code_version,
        stack_version,
        context_version,
    )
}

pub fn build_review_partner_prompt(input: &GenerateReviewPartnerInput) -> String {
    let context =
        serde_json::to_string_pretty(&build_prompt_context(input)).expect("context must serialize");
    let schema = serde_json::to_string_pretty(&review_partner_output_schema())
        .expect("schema must serialize");

    [
        "You are generating compact code explanation context for Remiss, a read-only pull request review IDE.",
        "The goal is explaining the scoped code. Produce code explanations and understanding checkpoints, not review prompts or assignments.",
        "The virtual stack layers are already validated. Preserve layer order, layer IDs, and atom coverage.",
        "Avoid checklists, verdict tables, evidence ledgers, pass/fail reports, tutorials, walkthroughs, and generic guides.",
        "Avoid emoji, markdown headings, decorative labels, code fences, and code sketches.",
        "Return compact right-rail explanation the reader cannot infer from the visible diff alone: concrete behavior summary, removed-code impact, similar existing code, grounded codebase-fit mismatch, and concrete implementation concerns when supported.",
        "Use historyContext.signals only as evidence-backed review memory. Do not treat prior signals as current truth when the current code contradicts them.",
        "When historyContext conflicts with current code, surface the conflict in assumptions, historySignals, or limitations instead of resolving it silently.",
        "Generate focusRecords for the supplied focusTargets. Each focus record explains one stack layer, not one diff hunk.",
        "The supplied focusTargets live at Pull-request context.focusTargets.",
        "Set each focusRecords[].key to the exact matching focusTargets[].key string. Do not use layerId, atom id, title, file path, or a generated key in that field.",
        "Each focus record must include one complete natural-language summary paragraph that synthesizes what changed, how the code behaves, the invariant/state change/error handling it affects, the supported intent or trade-off, and any relevant history signal.",
        "The summary must not be a file inventory, line list, stack-generator explanation, or statement about how Remiss grouped the change.",
        "Do not name changed files in the summary unless one specific file is itself the behavior being explained. Prefer the subsystem, flow, symbol contract, state, or invariant.",
        "Never write placeholder scaffolding such as 'the useful meaning is', 'what state changes', or 'which invariant'. Fill in the concrete behavior or leave the uncertainty to assumptions.",
        "Include only understandingCheckpoints that help the reviewer understand or verify the code, not generic review advice.",
        "A checkpoint should name the concrete invariant, edge case, assumption, or codebase pattern the reviewer should keep in mind.",
        "Use assumptions for inferred intent or behavior that is plausible but not directly proven by the supplied context.",
        "Use historySignals only for prior PRs, older behavior, supplied historyContext.signals, or verified historical context. Do not turn discussion from the current pull request into History rows.",
        "When explaining a layer, distinguish visible behavior from inferred intent and from unverifiable history.",
        "If generated, mechanical, or broad AI-assisted changes are present, call out the human verification surface: edge cases, invariants, and callsites that cannot be trusted from generation alone.",
        "Write the summary as factual code explanation, never as a question, instruction, checklist item, or review task.",
        "Rewrite any question-shaped draft into a declarative explanation before returning JSON.",
        "Never end a summary with an ellipsis.",
        "Match the supplied focus scope exactly. Ground intent in the code, diff, or collected context.",
        "Use semanticEvidence and collectedContext.semanticFocus as internal evidence for entity-level behavior when it directly overlaps this focus scope.",
        "Usage rows are generated by Remiss from tree-sitter syntax context. Leave usage lists out of the JSON.",
        "Use codebaseFit only for grounded mismatch evidence and only the 2-3 strongest non-empty secondary sections.",
        "Use compact prose rows, not checklist or bullet phrasing.",
        "Keep Usage context and Codebase fit out of sections.",
        "For codebaseFit, set follows=true when the collected context does not support a concrete mismatch. If follows=false, every evidence item must link to the existing code location that shows the mismatch.",
        "Keep stack-wide prose out of focus records. Repeat the layer brief only when it is the only useful context.",
        "Use the collectedContext as bounded read-only investigation. Treat partial context as partial.",
        "Use semanticEvidence as internal code-structure evidence for entity-level grouping, moved or reordered code, layer-to-atom mappings, focus entities, and impact context when it is present.",
        "Never mention Sem, semanticEvidence, internal tooling, atom IDs, layer IDs, semantic targets, loose file buckets, or grouping mechanics in user-facing fields.",
        "If intent or history is not supported by the supplied context, put the gap in assumptions or historySignals instead of inventing it in the summary.",
        "Only call out duplication, style mismatch, or overly defensive code when the supplied context supports it.",
        "Write complete sentences. Avoid truncating text with ellipses or placeholders like 'and more'.",
        "Use item.path and item.line only when they refer to a location present in collectedContext, stack atoms, or structuralEvidence.",
        "Return strict JSON only. No markdown fences or prose outside JSON.",
        "",
        "JSON schema:",
        &schema,
        "",
        "Pull-request context:",
        &context,
    ]
    .join("\n")
}

fn merge_review_partner(
    response: ReviewPartnerResponse,
    input: &GenerateReviewPartnerInput,
    model: Option<String>,
) -> Result<GeneratedReviewPartnerContext, String> {
    if response_omitted_focus_records_after_prompt_truncation(&response, input) {
        return Err(
            "Review Partner response omitted focus records after Copilot saw a truncated prompt."
                .to_string(),
        );
    }

    let valid_layer_ids = input
        .stack
        .layers
        .iter()
        .map(|layer| layer.id.clone())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::<String>::new();
    let mut response_layers = BTreeMap::<String, ReviewPartnerLayerResponse>::new();

    for layer in response.layers {
        if !valid_layer_ids.contains(&layer.layer_id) {
            return Err(format!(
                "Review Partner response referenced unknown layer id '{}'.",
                layer.layer_id
            ));
        }
        if !seen.insert(layer.layer_id.clone()) {
            return Err(format!(
                "Review Partner response duplicated layer id '{}'.",
                layer.layer_id
            ));
        }
        response_layers.insert(layer.layer_id.clone(), layer);
    }

    let layers = input
        .stack
        .layers
        .iter()
        .map(|layer| {
            response_layers
                .remove(&layer.id)
                .map(|response| merge_layer(layer, response, input))
                .unwrap_or_else(|| fallback_layer(layer, input))
        })
        .collect::<Vec<_>>();
    let mut response_focus_records = response
        .focus_records
        .into_iter()
        .map(|record| (record.key.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let focus_records = input
        .focus_targets
        .iter()
        .map(|target| {
            take_response_focus_record_for_target(&mut response_focus_records, target)
                .map(|record| {
                    merge_focus_record(
                        target,
                        record,
                        &input.context,
                        &input.stack,
                        &input.review_memory,
                    )
                })
                .unwrap_or_else(|| fallback_focus_record(input, target, None))
        })
        .collect::<Vec<_>>();

    Ok(GeneratedReviewPartnerContext {
        provider: input.provider,
        model,
        generated_at_ms: now_ms(),
        code_version_key: input.code_version_key.clone(),
        generator_version: REVIEW_PARTNER_GENERATOR_VERSION.to_string(),
        context_version: input.context.version.clone(),
        structural_evidence_version: input.structural_evidence.version.clone(),
        stack_brief: default_if_empty(response.stack_brief, &fallback_stack_brief(&input.stack)),
        stack_concerns: normalize_items(response.stack_concerns),
        limitations: normalize_text_items(response.limitations),
        warnings: normalize_text_items(response.warnings),
        fallback_reason: None,
        stack: input.stack.clone(),
        structural_evidence: input.structural_evidence.clone(),
        semantic_review: input.semantic_review.clone(),
        review_memory: input.review_memory.clone(),
        context: input.context.clone(),
        layers,
        focus_targets: input.focus_targets.clone(),
        focus_records,
    })
}

fn take_response_focus_record_for_target(
    records: &mut BTreeMap<String, ReviewPartnerFocusRecordResponse>,
    target: &ReviewPartnerFocusTarget,
) -> Option<ReviewPartnerFocusRecordResponse> {
    for key in response_focus_record_key_candidates(target) {
        if let Some(record) = records.remove(&key) {
            return Some(record);
        }
    }
    None
}

fn response_focus_record_key_candidates(target: &ReviewPartnerFocusTarget) -> Vec<String> {
    let mut keys = Vec::new();
    push_unique_key(&mut keys, target.key.clone());

    if target.match_kind == ReviewPartnerFocusMatchKind::Layer {
        if let Some(layer_id) = target.layer_id.as_deref() {
            push_unique_key(&mut keys, layer_id.to_string());
            push_unique_key(&mut keys, format!("layer:{layer_id}"));
        }
    }

    if target.atom_ids.len() == 1 {
        if let Some(atom_id) = target.atom_ids.first() {
            push_unique_key(&mut keys, atom_id.clone());
            push_unique_key(&mut keys, format!("atom:{atom_id}"));
        }
    }

    keys
}

fn push_unique_key(keys: &mut Vec<String>, key: String) {
    if !key.trim().is_empty() && !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}

fn response_omitted_focus_records_after_prompt_truncation(
    response: &ReviewPartnerResponse,
    input: &GenerateReviewPartnerInput,
) -> bool {
    !input.focus_targets.is_empty()
        && response.focus_records.is_empty()
        && response.warnings.iter().any(|warning| {
            let normalized = warning.to_ascii_lowercase();
            normalized.contains("truncated") && normalized.contains("focustarget")
        })
}

fn merge_layer(
    layer: &ReviewStackLayer,
    response: ReviewPartnerLayerResponse,
    input: &GenerateReviewPartnerInput,
) -> ReviewPartnerLayer {
    let fallback = fallback_layer(layer, input);
    let _legacy_usage_context = response.usage_context;
    let brief = normalize_layer_brief(layer, response.brief, &fallback.brief);
    ReviewPartnerLayer {
        layer_id: layer.id.clone(),
        title: normalize_stack_layer_title(&layer.title, "Stack layer"),
        brief,
        changed_items: normalize_items_or(response.changed_items, fallback.changed_items),
        removed_items: normalize_items_or(response.removed_items, fallback.removed_items),
        usage_context: fallback.usage_context,
        similar_code: normalize_items_or(response.similar_code, fallback.similar_code),
        codebase_fit: normalize_items_or(response.codebase_fit, fallback.codebase_fit),
        concerns: normalize_items_or(response.concerns, fallback.concerns),
        limitations: normalize_text_items(response.limitations)
            .into_iter()
            .chain(fallback.limitations)
            .take(MAX_SECTION_ITEMS)
            .collect(),
        structural_evidence_status: input
            .structural_evidence
            .status_for_atom_ids(&layer.atom_ids),
    }
}

fn fallback_layer(
    layer: &ReviewStackLayer,
    input: &GenerateReviewPartnerInput,
) -> ReviewPartnerLayer {
    let context = input.context.layer(&layer.id);
    let status = input
        .structural_evidence
        .status_for_atom_ids(&layer.atom_ids);
    let mut limitations = context
        .map(|context| context.limitations.clone())
        .unwrap_or_default();
    if status != StructuralEvidenceStatus::Full {
        limitations.push(status.label().to_string());
    }

    ReviewPartnerLayer {
        layer_id: layer.id.clone(),
        title: normalize_stack_layer_title(&layer.title, "Stack layer"),
        brief: fallback_layer_brief(layer, context),
        changed_items: context
            .map(items_from_semantic_focus)
            .filter(|items| !items.is_empty())
            .or_else(|| {
                context
                    .map(items_from_changed_symbols)
                    .filter(|items| !items.is_empty())
            })
            .or_else(|| {
                context
                    .map(items_from_semantic_layers)
                    .filter(|items| !items.is_empty())
            })
            .unwrap_or_else(|| items_from_layer_atoms(layer, &input.stack)),
        removed_items: context.map(items_from_removed_symbols).unwrap_or_default(),
        usage_context: context.map(items_from_usages).unwrap_or_default(),
        similar_code: context
            .map(items_from_similar_locations)
            .unwrap_or_default(),
        codebase_fit: context.map(items_from_style_notes).unwrap_or_default(),
        concerns: layer
            .warnings
            .iter()
            .map(|warning| {
                ReviewPartnerItem::new(
                    warning.code.clone(),
                    warning.message.clone(),
                    warning.path.clone(),
                    None,
                )
            })
            .take(MAX_SECTION_ITEMS)
            .collect(),
        limitations: limitations.into_iter().take(MAX_SECTION_ITEMS).collect(),
        structural_evidence_status: status,
    }
}

fn fallback_layer_brief(
    layer: &ReviewStackLayer,
    context: Option<&ReviewPartnerCollectedLayer>,
) -> String {
    let fallback = fallback_layer_title_summary(layer);
    context
        .and_then(|context| semantic_layer_brief(layer, context))
        .map(|summary| normalize_layer_brief(layer, summary, &fallback))
        .unwrap_or_else(|| normalize_layer_brief(layer, layer.summary.clone(), &fallback))
}

fn fallback_layer_title_summary(layer: &ReviewStackLayer) -> String {
    let action = layer_action_verb_lower(&layer.title);
    let subject = layer_subject_for_sentence(&layer.title);
    limit_text(
        format!("This change {action} {subject}."),
        MAX_BRIEF_TEXT_CHARS,
    )
}

fn normalize_layer_brief(
    layer: &ReviewStackLayer,
    brief: impl AsRef<str>,
    fallback: &str,
) -> String {
    normalize_review_partner_summary_text(brief.as_ref())
        .or_else(|| normalize_review_partner_summary_text(fallback))
        .unwrap_or_else(|| fallback_layer_title_summary(layer))
}

fn semantic_layer_brief(
    layer: &ReviewStackLayer,
    context: &ReviewPartnerCollectedLayer,
) -> Option<String> {
    let mut files = BTreeSet::<String>::new();
    let mut entities = BTreeSet::<String>::new();

    for semantic_layer in &context.semantic_layers {
        files.extend(semantic_layer.file_paths.iter().cloned());
        for name in &semantic_layer.entity_names {
            insert_summary_entity_name(&mut entities, name);
        }
    }
    for focus in &context.semantic_focus {
        if let Some(entity) = focus
            .target_entity
            .as_ref()
            .or_else(|| focus.overlapping_entities.first())
        {
            insert_summary_entity_name(&mut entities, &entity.name);
            files.insert(entity.file_path.clone());
        }
    }
    for symbol in context
        .changed_symbols
        .iter()
        .chain(context.removed_symbols.iter())
    {
        insert_summary_entity_name(&mut entities, &symbol.symbol);
        files.insert(symbol.path.clone());
    }

    if files.is_empty() && entities.is_empty() {
        return None;
    }

    let scope = layer_scope_description(layer, &files, &entities);
    let action = layer_action_verb_lower(&layer.title);
    let meaning = layer_meaning_sentence(&files, &entities);

    Some(limit_text(
        format!("This change {action} {scope}. {meaning}"),
        MAX_BRIEF_TEXT_CHARS,
    ))
}

fn layer_scope_description(
    layer: &ReviewStackLayer,
    files: &BTreeSet<String>,
    entities: &BTreeSet<String>,
) -> String {
    let subject = layer_subject_for_sentence(&layer.title);
    if !files.is_empty() && files.iter().all(|path| is_lockfile_path(path)) {
        return "dependency resolution state".to_string();
    }
    if !files.is_empty() && files.iter().all(|path| is_config_path(path)) {
        return "build or runtime configuration".to_string();
    }
    if !files.is_empty() && files.iter().all(|path| is_test_path(path)) {
        return format!("test coverage for {subject}");
    }
    if !entities.is_empty() {
        let entity_list = natural_symbol_list(entities.iter().map(String::as_str), 3);
        return format!("{subject} around {entity_list}");
    }
    subject
}

fn layer_meaning_sentence(files: &BTreeSet<String>, entities: &BTreeSet<String>) -> String {
    if !files.is_empty() && files.iter().all(|path| is_lockfile_path(path)) {
        return "It affects the pinned version set every build resolves, not application control flow.".to_string();
    }
    if !files.is_empty() && files.iter().all(|path| is_config_path(path)) {
        return "It affects the environment or build invariant the rest of the code now relies on."
            .to_string();
    }
    if !files.is_empty() && files.iter().all(|path| is_test_path(path)) {
        return "It records the behavior, edge case, or regression the test suite now protects."
            .to_string();
    }
    if !entities.is_empty() {
        return "It affects the behavior, state, and caller contract attached to those symbols."
            .to_string();
    }
    "It affects the scoped behavior carried by the diff and the surfaced usage context.".to_string()
}

fn insert_summary_entity_name(entities: &mut BTreeSet<String>, name: &str) {
    let name = clean_symbol(name);
    if !name.is_empty() && !name.contains('/') && !name.contains('\\') {
        entities.insert(name);
    }
}

fn is_lockfile_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with("cargo.lock")
        || lower.ends_with("package-lock.json")
        || lower.ends_with("pnpm-lock.yaml")
        || lower.ends_with("yarn.lock")
        || lower.ends_with("poetry.lock")
        || lower.ends_with("gradle.lockfile")
        || lower.contains("lockfile")
}

fn is_config_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json")
        || lower.ends_with(".lock")
        || lower.ends_with(".properties")
        || lower.ends_with("build.gradle")
        || lower.ends_with("settings.gradle")
        || lower.ends_with("pom.xml")
        || lower.ends_with("package.json")
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
}

fn strip_line_inventory(value: &str) -> String {
    let trimmed = value.trim();
    for marker in [" around lines", " around line", " near lines", " near line"] {
        if let Some(index) = trimmed.to_ascii_lowercase().find(marker) {
            return default_if_empty(trimmed[..index].trim().to_string(), "this change");
        }
    }
    default_if_empty(trimmed.to_string(), "this change")
}

fn layer_subject_for_sentence(title: &str) -> String {
    let subject = strip_line_inventory(&layer_subject(title));
    let lower = subject.to_ascii_lowercase();
    if lower == "this layer" || lower == "stack layer" || lower == "this change" {
        return "the scoped behavior".to_string();
    }
    if lower.starts_with("the ")
        || lower.starts_with("a ")
        || lower.starts_with("an ")
        || lower.starts_with("this ")
    {
        subject
    } else {
        format!("the {subject}")
    }
}

fn layer_action_verb_lower(title: &str) -> &'static str {
    match layer_action_verb(title) {
        "Adds" => "adds",
        "Removes" => "removes",
        "Moves" => "moves",
        "Renames" => "renames",
        "Refactors" => "refactors",
        "Updates" => "updates",
        _ => "affects",
    }
}

fn layer_action_verb(title: &str) -> &'static str {
    let lower = title.trim_start().to_ascii_lowercase();
    if lower.starts_with("add ") {
        "Adds"
    } else if lower.starts_with("remove ") || lower.starts_with("delete ") {
        "Removes"
    } else if lower.starts_with("move ") {
        "Moves"
    } else if lower.starts_with("rename ") {
        "Renames"
    } else if lower.starts_with("refactor ") {
        "Refactors"
    } else if lower.starts_with("update ") || lower.starts_with("change ") {
        "Updates"
    } else {
        "Covers"
    }
}

fn layer_subject(title: &str) -> String {
    let title = title.trim();
    for prefix in [
        "Add ",
        "Remove ",
        "Delete ",
        "Move ",
        "Rename ",
        "Refactor ",
        "Update ",
        "Change ",
    ] {
        if let Some(rest) = title.strip_prefix(prefix) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    default_if_empty(title.to_string(), "this layer")
}

fn natural_symbol_list<'a>(values: impl Iterator<Item = &'a str>, max_items: usize) -> String {
    let values = values
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return "the affected symbols".to_string();
    }

    let visible = values.iter().take(max_items).copied().collect::<Vec<_>>();
    let extra_count = values.len().saturating_sub(visible.len());
    match visible.as_slice() {
        [one] if extra_count == 0 => (*one).to_string(),
        [one] => format!("{one} and related symbols"),
        [one, two] if extra_count == 0 => format!("{one} and {two}"),
        _ if extra_count == 0 => {
            let mut text = visible.join(", ");
            if let Some((head, tail)) = text.rsplit_once(", ") {
                text = format!("{head}, and {tail}");
            }
            text
        }
        _ => format!("{}, and related symbols", visible.join(", ")),
    }
}

fn merge_focus_record(
    target: &ReviewPartnerFocusTarget,
    response: ReviewPartnerFocusRecordResponse,
    context: &ReviewPartnerContextPack,
    stack: &ReviewStack,
    review_memory: &ReviewMemoryPromptContext,
) -> ReviewPartnerFocusRecord {
    let _legacy_usage_context = normalize_usage_groups(response.usage_context);
    let (sections, legacy_codebase_fit_items) = normalize_focus_sections(response.sections);
    let usage_context = usage_groups_for_target(target, context);
    let codebase_fit = response
        .codebase_fit
        .map(normalize_codebase_fit)
        .unwrap_or_else(|| codebase_fit_from_items(legacy_codebase_fit_items));
    let fallback_summary = fallback_focus_summary_from_stack(target, stack, context);
    let summary = response
        .summary
        .map(|summary| normalize_focus_summary(target, &summary))
        .filter(|summary| !summary.trim().is_empty() && summary != &target.title)
        .unwrap_or(fallback_summary);
    let history_signals = merge_history_signals(
        review_memory_history_items_for_target(target, stack, review_memory),
        normalize_items(response.history_signals),
    );

    ReviewPartnerFocusRecord {
        key: target.key.clone(),
        title: default_if_empty(response.title, &target.title),
        subtitle: response
            .subtitle
            .map(|subtitle| limit_text(subtitle, MAX_FOCUS_TITLE_CHARS))
            .filter(|subtitle| !subtitle.trim().is_empty())
            .unwrap_or_else(|| target.subtitle.clone()),
        target: target.clone(),
        summary,
        usage_context,
        codebase_fit,
        sections,
        understanding_checkpoints: normalize_items(response.understanding_checkpoints),
        assumptions: normalize_items(response.assumptions),
        history_signals,
        limitations: normalize_text_items(response.limitations),
        generated_at_ms: now_ms(),
    }
}

fn normalize_focus_summary(target: &ReviewPartnerFocusTarget, summary: &str) -> String {
    normalize_review_partner_summary_text(summary).unwrap_or_else(|| target.title.clone())
}

fn normalize_review_partner_summary_text(summary: &str) -> Option<String> {
    let summary = summary.trim().trim_end_matches("...").trim_end();
    if summary.is_empty() {
        return None;
    }

    if let Some((_, remainder)) = summary.split_once('?') {
        let remainder = remainder.trim();
        if !remainder.is_empty() {
            return normalize_review_partner_summary_text(remainder);
        }
        return None;
    }

    if review_partner_summary_starts_like_prompt(summary)
        || review_partner_summary_mentions_internal_tooling(summary)
        || review_partner_summary_contains_prompt_scaffold(summary)
        || review_partner_summary_looks_like_file_inventory(summary)
    {
        return None;
    }

    Some(summary.to_string())
}

fn review_partner_summary_starts_like_prompt(summary: &str) -> bool {
    let lower = summary.trim_start().to_ascii_lowercase();
    [
        "does ",
        "does this ",
        "do ",
        "is ",
        "are ",
        "check whether ",
        "review how ",
        "verify that ",
        "can ",
        "should ",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn review_partner_summary_mentions_internal_tooling(summary: &str) -> bool {
    let lower = summary.trim_start().to_ascii_lowercase();
    if lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == "sem")
    {
        return true;
    }

    [
        "sem ties",
        "sem groups",
        "semantic target",
        "loose file bucket",
        "semantic evidence",
        "semanticevidence",
        "atom id",
        "atom ids",
        "layer id",
        "layer ids",
        "grouped by",
        "grouping mechanics",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn review_partner_summary_contains_prompt_scaffold(summary: &str) -> bool {
    let lower = summary.trim_start().to_ascii_lowercase();
    [
        "the useful meaning is",
        "useful meaning is",
        "what state changes",
        "which invariant or edge case",
        "what tests, comments, callers, or prior signals",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn review_partner_summary_looks_like_file_inventory(summary: &str) -> bool {
    let lower = summary.trim_start().to_ascii_lowercase();
    if lower.contains("changed files")
        || lower.contains("file inventory")
        || lower.contains("files changed")
    {
        return true;
    }

    let path_like_tokens = summary
        .split_whitespace()
        .filter(|token| review_partner_summary_token_looks_like_path(token))
        .count();
    let starts_like_inventory = [
        "this layer covers ",
        "this layer includes ",
        "this layer touches ",
        "this change covers ",
        "this change includes ",
        "this change touches ",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix));
    let has_plus_others = lower.contains(" plus ") && lower.contains(" other");

    path_like_tokens >= 2
        || (starts_like_inventory && path_like_tokens >= 1)
        || (starts_like_inventory && has_plus_others)
}

fn review_partner_summary_token_looks_like_path(token: &str) -> bool {
    let token = token.trim_matches(|ch: char| {
        ch == ','
            || ch == '.'
            || ch == ';'
            || ch == ':'
            || ch == '('
            || ch == ')'
            || ch == '['
            || ch == ']'
            || ch == '`'
            || ch == '"'
            || ch == '\''
    });
    if token.is_empty() {
        return false;
    }
    let lower = token.to_ascii_lowercase();
    let has_path_separator = lower.contains('/') || lower.contains('\\');
    let pathy_prefix = lower.starts_with("src/")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.starts_with("backend/")
        || lower.starts_with("frontend/")
        || lower.starts_with("app/")
        || lower.starts_with("packages/");
    let known_extension = [
        ".rs", ".kt", ".java", ".ts", ".tsx", ".js", ".jsx", ".swift", ".go", ".py", ".rb",
        ".toml", ".json", ".yaml", ".yml", ".xml", ".gradle", ".md",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension));

    has_path_separator && (known_extension || pathy_prefix || lower.matches('/').count() >= 2)
}

fn fallback_focus_summary_from_stack(
    target: &ReviewPartnerFocusTarget,
    stack: &ReviewStack,
    context: &ReviewPartnerContextPack,
) -> String {
    let Some(layer_id) = target.layer_id.as_deref() else {
        return target.title.clone();
    };
    let Some(layer) = stack.layers.iter().find(|layer| layer.id == layer_id) else {
        return target.title.clone();
    };
    let layer_context = context.layer(layer_id);
    let fallback = fallback_layer_title_summary(layer);
    layer_context
        .and_then(|context| semantic_layer_brief(layer, context))
        .map(|summary| normalize_layer_brief(layer, summary, &fallback))
        .unwrap_or_else(|| normalize_layer_brief(layer, &layer.summary, &fallback))
}

pub fn fallback_focus_record(
    input: &GenerateReviewPartnerInput,
    target: &ReviewPartnerFocusTarget,
    warning: Option<String>,
) -> ReviewPartnerFocusRecord {
    let layer = target
        .layer_id
        .as_deref()
        .and_then(|layer_id| input.stack.layers.iter().find(|layer| layer.id == layer_id));
    let fallback_layer = layer.map(|layer| fallback_layer(layer, input));
    let mut sections = fallback_layer
        .as_ref()
        .map(|layer| focus_sections_from_layer(target, layer))
        .unwrap_or_default();
    let usage_context = usage_groups_for_target(target, &input.context);
    let codebase_fit = fallback_layer
        .as_ref()
        .map(|layer| codebase_fit_from_items(focus_items_for_target(target, &layer.codebase_fit)))
        .unwrap_or_default();
    let summary = fallback_focus_summary(target, fallback_layer.as_ref());

    if sections.is_empty() {
        sections.push(ReviewPartnerFocusSection {
            title: "Layer changes".to_string(),
            items: target
                .atom_ids
                .iter()
                .filter_map(|atom_id| input.stack.atom(atom_id))
                .map(|atom| {
                    ReviewPartnerItem::new(
                        atom.symbol_name
                            .clone()
                            .unwrap_or_else(|| atom.path.clone()),
                        format!(
                            "{} changed line{} in this focus area.",
                            atom.additions + atom.deletions,
                            if atom.additions + atom.deletions == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ),
                        Some(atom.path.clone()),
                        atom.new_range.and_then(line_from_range),
                    )
                })
                .take(MAX_SECTION_ITEMS)
                .collect(),
        });
    }

    let mut limitations = fallback_layer
        .as_ref()
        .map(|layer| layer.limitations.clone())
        .unwrap_or_default();
    if let Some(warning) = warning {
        limitations.push(warning);
    }

    ReviewPartnerFocusRecord {
        key: target.key.clone(),
        title: target.title.clone(),
        subtitle: target.subtitle.clone(),
        target: target.clone(),
        summary,
        usage_context,
        codebase_fit,
        sections: sections
            .into_iter()
            .filter(|section| !section.items.is_empty())
            .take(MAX_FOCUS_SECTIONS)
            .collect(),
        understanding_checkpoints: fallback_understanding_checkpoints(
            target,
            fallback_layer.as_ref(),
        ),
        assumptions: Vec::new(),
        history_signals: review_memory_history_items_for_target(
            target,
            &input.stack,
            &input.review_memory,
        ),
        limitations: limitations.into_iter().take(MAX_SECTION_ITEMS).collect(),
        generated_at_ms: now_ms(),
    }
}

fn fallback_understanding_checkpoints(
    target: &ReviewPartnerFocusTarget,
    layer: Option<&ReviewPartnerLayer>,
) -> Vec<ReviewPartnerItem> {
    let Some(layer) = layer else {
        return Vec::new();
    };

    let mut items = focus_items_for_target(target, &layer.concerns);
    items.extend(focus_items_for_target(target, &layer.codebase_fit));
    items.truncate(MAX_SECTION_ITEMS);
    items
}

fn review_memory_history_items_for_target(
    target: &ReviewPartnerFocusTarget,
    stack: &ReviewStack,
    review_memory: &ReviewMemoryPromptContext,
) -> Vec<ReviewPartnerItem> {
    review_memory
        .signals
        .iter()
        .filter(|signal| review_memory_signal_matches_target(signal, target, stack))
        .map(review_memory_signal_item)
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn review_memory_signal_matches_target(
    signal: &ReviewMemorySignal,
    target: &ReviewPartnerFocusTarget,
    stack: &ReviewStack,
) -> bool {
    let signal_path = signal
        .path
        .as_deref()
        .or_else(|| signal.scope.path())
        .unwrap_or_default();
    if signal_path.is_empty() {
        return false;
    }

    if signal_path == target.file_path {
        return true;
    }

    target.atom_ids.iter().any(|atom_id| {
        stack.atom(atom_id).is_some_and(|atom| {
            atom.path == signal_path
                || atom.previous_path.as_deref() == Some(signal_path)
                || signal
                    .scope
                    .symbol_name()
                    .map(|signal_symbol| {
                        atom.symbol_name
                            .as_deref()
                            .map(|atom_symbol| {
                                review_memory_symbols_match(atom_symbol, signal_symbol)
                            })
                            .unwrap_or(false)
                            || atom
                                .defined_symbols
                                .iter()
                                .any(|defined| review_memory_symbols_match(defined, signal_symbol))
                    })
                    .unwrap_or(false)
        })
    })
}

fn review_memory_signal_item(signal: &ReviewMemorySignal) -> ReviewPartnerItem {
    let status = review_memory_status_label(&signal.status);
    let pr = signal
        .last_seen_pr
        .or(signal.first_seen_pr)
        .map(|number| format!("PR #{number} "))
        .unwrap_or_default();
    let title = format!(
        "{pr}{status} {} {}",
        signal.reading_level.label(),
        signal.kind.label()
    );
    let mut detail = format!(
        "{} Origin: {}. Evidence: {}",
        signal.statement.trim_end_matches('.'),
        signal.origin.label(),
        signal.evidence_summary
    );
    if let Some(why_useful_next_time) = signal.why_useful_next_time.as_deref() {
        detail.push_str(" Next time: ");
        detail.push_str(why_useful_next_time);
    }
    ReviewPartnerItem::new_with_limits(
        title,
        detail,
        signal.path.clone(),
        signal.line.and_then(|line| usize::try_from(line).ok()),
        MAX_HISTORY_ITEM_TEXT_CHARS,
    )
}

fn merge_history_signals(
    memory_items: Vec<ReviewPartnerItem>,
    model_items: Vec<ReviewPartnerItem>,
) -> Vec<ReviewPartnerItem> {
    let mut seen = BTreeSet::<(String, String, Option<String>, Option<usize>)>::new();
    memory_items
        .into_iter()
        .chain(model_items)
        .filter(|item| {
            seen.insert((
                item.title.clone(),
                item.detail.clone(),
                item.path.clone(),
                item.line,
            ))
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn review_memory_status_label(status: &ReviewMemoryStatus) -> &'static str {
    match status {
        ReviewMemoryStatus::Candidate => "candidate",
        ReviewMemoryStatus::Open => "open",
        ReviewMemoryStatus::Resolved => "resolved",
        ReviewMemoryStatus::Promoted => "promoted",
        ReviewMemoryStatus::Stale => "stale",
        ReviewMemoryStatus::Hidden => "hidden",
    }
}

fn review_memory_symbols_match(left: &str, right: &str) -> bool {
    let left = normalize_review_memory_symbol(left);
    let right = normalize_review_memory_symbol(right);
    left == right || left.ends_with(&format!("::{right}")) || right.ends_with(&format!("::{left}"))
}

fn normalize_review_memory_symbol(value: &str) -> String {
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

pub fn generate_review_partner_focus_record(
    document: &GeneratedReviewPartnerContext,
    target: ReviewPartnerFocusTarget,
    working_directory: &str,
) -> Result<ReviewPartnerFocusRecord, String> {
    generate_review_partner_focus_record_with_progress(
        document,
        target,
        working_directory,
        &mut |_| {},
    )
}

pub fn generate_review_partner_focus_record_with_progress(
    document: &GeneratedReviewPartnerContext,
    target: ReviewPartnerFocusTarget,
    working_directory: &str,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) -> Result<ReviewPartnerFocusRecord, String> {
    if working_directory.trim().is_empty() {
        return Err("Review Partner focus generation requires a local checkout path.".to_string());
    }

    if !Path::new(working_directory).exists() {
        return Err(format!(
            "The local checkout path '{}' does not exist.",
            working_directory
        ));
    }

    let prompt = build_focus_record_prompt(document, &target);
    let response = agents::run_json_prompt_with_options_and_progress(
        document.provider,
        working_directory,
        prompt,
        AgentJsonPromptOptions::review_partner_focus(),
        on_progress,
    )?;
    let parsed =
        parse_tolerant::<ReviewPartnerSingleFocusResponse>(&response.text).map_err(|error| {
            format!(
                "Failed to parse Review Partner focus JSON: {}",
                error.message
            )
        })?;
    Ok(merge_focus_record(
        &target,
        parsed.record,
        &document.context,
        &document.stack,
        &document.review_memory,
    ))
}

pub fn upsert_focus_record(
    document: &mut GeneratedReviewPartnerContext,
    target: ReviewPartnerFocusTarget,
    record: ReviewPartnerFocusRecord,
) {
    if !document
        .focus_targets
        .iter()
        .any(|existing| existing.key == target.key)
    {
        document.focus_targets.push(target);
    }

    if let Some(existing) = document
        .focus_records
        .iter_mut()
        .find(|existing| existing.key == record.key)
    {
        *existing = record;
    } else {
        document.focus_records.push(record);
    }
}

fn focus_sections_from_layer(
    target: &ReviewPartnerFocusTarget,
    layer: &ReviewPartnerLayer,
) -> Vec<ReviewPartnerFocusSection> {
    [
        ("Similar code", layer.similar_code.as_slice()),
        ("Removed impact", layer.removed_items.as_slice()),
        ("Concerns", layer.concerns.as_slice()),
    ]
    .into_iter()
    .filter_map(|(title, items)| {
        let items = focus_items_for_target(target, items);
        (!items.is_empty()).then(|| ReviewPartnerFocusSection {
            title: title.to_string(),
            items,
        })
    })
    .take(MAX_FOCUS_SECTIONS)
    .collect()
}

fn focus_items_for_target(
    target: &ReviewPartnerFocusTarget,
    items: &[ReviewPartnerItem],
) -> Vec<ReviewPartnerItem> {
    if target.match_kind == ReviewPartnerFocusMatchKind::Layer {
        return items.iter().take(MAX_SECTION_ITEMS).cloned().collect();
    }

    let focused = items
        .iter()
        .filter(|item| {
            item.path.as_deref() == Some(target.file_path.as_str())
                || item
                    .line
                    .zip(target.line)
                    .map(|(item_line, target_line)| item_line.abs_diff(target_line) <= 12)
                    .unwrap_or(false)
        })
        .take(MAX_SECTION_ITEMS)
        .cloned()
        .collect::<Vec<_>>();

    if focused.is_empty() {
        items.iter().take(3).cloned().collect()
    } else {
        focused
    }
}

pub fn build_review_partner_focus_targets(
    stack: &ReviewStack,
    _structural_evidence: &StructuralEvidencePack,
) -> Vec<ReviewPartnerFocusTarget> {
    stack
        .layers
        .iter()
        .take(MAX_FOCUS_RECORDS)
        .map(|layer| focus_target_from_layer(stack, layer))
        .collect()
}

pub fn focus_target_for_layer(
    document: &GeneratedReviewPartnerContext,
    layer_id: &str,
) -> Option<ReviewPartnerFocusTarget> {
    document
        .focus_targets
        .iter()
        .find(|target| target.layer_id.as_deref() == Some(layer_id))
        .cloned()
}

pub fn focus_target_for_diff_focus(
    document: &GeneratedReviewPartnerContext,
    file_path: &str,
    line: Option<usize>,
    side: Option<&str>,
    hunk_header: Option<&str>,
) -> ReviewPartnerFocusTarget {
    if let Some(atom_target) =
        focus_atom_target_for_diff_focus(&document.stack, file_path, line, side, hunk_header)
    {
        return document
            .focus_target(&atom_target.key)
            .map(|existing| merge_existing_focus_target_metadata(atom_target.clone(), existing))
            .unwrap_or(atom_target);
    }

    if let Some(hunk_target) = focus_hunk_target_for_diff_focus(
        &document.stack,
        &document.structural_evidence,
        file_path,
        line,
        side,
        hunk_header,
    ) {
        return document
            .focus_target(&hunk_target.key)
            .map(|existing| merge_existing_focus_target_metadata(hunk_target.clone(), existing))
            .unwrap_or(hunk_target);
    }

    focus_target_from_file(file_path.to_string(), line, side.map(str::to_string), None)
}

fn merge_existing_focus_target_metadata(
    mut target: ReviewPartnerFocusTarget,
    existing: &ReviewPartnerFocusTarget,
) -> ReviewPartnerFocusTarget {
    target.title = existing.title.clone();
    if target.hunk_header.is_none() {
        target.hunk_header = existing.hunk_header.clone();
    }
    if target.hunk_index.is_none() {
        target.hunk_index = existing.hunk_index;
    }
    if target.line.is_none() {
        target.line = existing.line;
    }
    if target.side.is_none() {
        target.side = existing.side.clone();
    }
    if target.atom_ids.is_empty() {
        target.atom_ids = existing.atom_ids.clone();
    }
    if target.layer_id.is_none() {
        target.layer_id = existing.layer_id.clone();
    }
    target
}

fn focus_atom_target_for_diff_focus(
    stack: &ReviewStack,
    file_path: &str,
    line: Option<usize>,
    side: Option<&str>,
    hunk_header: Option<&str>,
) -> Option<ReviewPartnerFocusTarget> {
    let layers_by_atom = layer_ids_by_atom(stack);
    let preferred_left = side == Some("LEFT");
    let line_candidates = stack
        .atoms
        .iter()
        .filter(|atom| atom_matches_path(atom, file_path, preferred_left))
        .filter_map(|atom| {
            let range = if preferred_left {
                atom.old_range
            } else {
                atom.new_range
            }?;
            let line = line?;
            range_contains_line(range, line).then_some((range_len(range), atom))
        })
        .collect::<Vec<_>>();

    if let Some((_, atom)) = line_candidates
        .into_iter()
        .min_by_key(|(range_len, atom)| (*range_len, atom.additions + atom.deletions))
    {
        return Some(focus_target_from_atom(
            atom,
            layers_by_atom.get(&atom.id).cloned(),
            ReviewPartnerFocusMatchKind::AtomRange,
            line,
            side.map(str::to_string),
            hunk_header.map(str::to_string),
        ));
    }

    let hunk_header = hunk_header?;
    stack
        .atoms
        .iter()
        .filter(|atom| atom_matches_path(atom, file_path, preferred_left))
        .filter(|atom| atom.hunk_headers.iter().any(|header| header == hunk_header))
        .min_by_key(|atom| atom.additions + atom.deletions)
        .map(|atom| {
            focus_target_from_atom(
                atom,
                layers_by_atom.get(&atom.id).cloned(),
                ReviewPartnerFocusMatchKind::AtomHunk,
                line.or_else(|| atom.new_range.and_then(line_from_range)),
                side.map(str::to_string),
                Some(hunk_header.to_string()),
            )
        })
}

fn focus_hunk_target_for_diff_focus(
    stack: &ReviewStack,
    evidence: &StructuralEvidencePack,
    file_path: &str,
    line: Option<usize>,
    side: Option<&str>,
    hunk_header: Option<&str>,
) -> Option<ReviewPartnerFocusTarget> {
    let layers_by_atom = layer_ids_by_atom(stack);
    evidence
        .files
        .iter()
        .find(|file| file.path == file_path)
        .and_then(|file| {
            file.changes
                .iter()
                .find(|change| {
                    hunk_header
                        .map(|header| change.hunk_header == header)
                        .unwrap_or(false)
                        || line
                            .zip(change.new_range)
                            .map(|(line, range)| range_contains_line(range, line))
                            .unwrap_or(false)
                })
                .map(|change| {
                    let atom_ids = change
                        .atom_ids
                        .iter()
                        .take(MAX_FOCUS_TARGET_ATOMS)
                        .cloned()
                        .collect::<Vec<_>>();
                    let layer_id = atom_ids
                        .iter()
                        .find_map(|atom_id| layers_by_atom.get(atom_id).cloned());
                    focus_target_from_hunk(
                        file.path.clone(),
                        Some(change.hunk_header.clone()),
                        Some(change.hunk_index),
                        line.or_else(|| change.new_range.and_then(line_from_range)),
                        side.map(str::to_string),
                        atom_ids,
                        layer_id,
                    )
                })
        })
}

fn focus_target_from_atom(
    atom: &ChangeAtom,
    layer_id: Option<String>,
    match_kind: ReviewPartnerFocusMatchKind,
    line: Option<usize>,
    side: Option<String>,
    hunk_header: Option<String>,
) -> ReviewPartnerFocusTarget {
    let title = atom
        .symbol_name
        .clone()
        .or_else(|| atom.defined_symbols.first().cloned())
        .unwrap_or_else(|| atom.path.clone());
    let subtitle = focus_subtitle(&atom.path, line, match_kind);

    ReviewPartnerFocusTarget {
        key: format!("atom:{}", atom.id),
        file_path: atom.path.clone(),
        hunk_header,
        hunk_index: atom.hunk_indices.first().copied(),
        line,
        side,
        atom_ids: vec![atom.id.clone()],
        layer_id,
        title: limit_text(title, MAX_FOCUS_TITLE_CHARS),
        subtitle,
        match_kind,
    }
}

fn focus_target_from_layer(
    stack: &ReviewStack,
    layer: &ReviewStackLayer,
) -> ReviewPartnerFocusTarget {
    let atoms = stack.atoms_for_layer(layer);
    let file_path = atoms
        .iter()
        .find(|atom| !atom.path.trim().is_empty())
        .map(|atom| atom.path.clone())
        .or_else(|| stack.first_file_for_layer(layer))
        .unwrap_or_else(|| layer.title.clone());
    let line = atoms
        .iter()
        .find_map(|atom| atom.new_range.and_then(line_from_range));
    let changed_files = atoms
        .iter()
        .filter(|atom| !atom.path.trim().is_empty())
        .map(|atom| atom.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let subtitle = format!(
        "Stack layer · {} file{}, +{} -{}",
        changed_files,
        if changed_files == 1 { "" } else { "s" },
        layer.metrics.additions,
        layer.metrics.deletions
    );

    ReviewPartnerFocusTarget {
        key: format!("layer:{}", layer.id),
        file_path,
        hunk_header: None,
        hunk_index: None,
        line,
        side: Some("RIGHT".to_string()),
        atom_ids: layer
            .atom_ids
            .iter()
            .take(MAX_FOCUS_TARGET_ATOMS)
            .cloned()
            .collect(),
        layer_id: Some(layer.id.clone()),
        title: limit_text(
            normalize_stack_layer_title(&layer.title, "Stack layer"),
            MAX_FOCUS_TITLE_CHARS,
        ),
        subtitle,
        match_kind: ReviewPartnerFocusMatchKind::Layer,
    }
}

fn focus_target_from_hunk(
    file_path: String,
    hunk_header: Option<String>,
    hunk_index: Option<usize>,
    line: Option<usize>,
    side: Option<String>,
    atom_ids: Vec<String>,
    layer_id: Option<String>,
) -> ReviewPartnerFocusTarget {
    let key_seed = format!(
        "{}:{}:{}",
        file_path,
        hunk_index
            .map(|index| index.to_string())
            .unwrap_or_else(|| "-".to_string()),
        hunk_header.as_deref().unwrap_or("")
    );
    ReviewPartnerFocusTarget {
        key: format!("hunk:{}", short_hash(&key_seed)),
        file_path: file_path.clone(),
        hunk_header,
        hunk_index,
        line,
        side,
        atom_ids,
        layer_id,
        title: location_title(&file_path, line),
        subtitle: focus_subtitle(&file_path, line, ReviewPartnerFocusMatchKind::Hunk),
        match_kind: ReviewPartnerFocusMatchKind::Hunk,
    }
}

fn focus_target_from_file(
    file_path: String,
    line: Option<usize>,
    side: Option<String>,
    layer_id: Option<String>,
) -> ReviewPartnerFocusTarget {
    ReviewPartnerFocusTarget {
        key: format!("file:{}", short_hash(&file_path)),
        file_path: file_path.clone(),
        hunk_header: None,
        hunk_index: None,
        line,
        side,
        atom_ids: Vec::new(),
        layer_id,
        title: file_path.clone(),
        subtitle: focus_subtitle(&file_path, line, ReviewPartnerFocusMatchKind::File),
        match_kind: ReviewPartnerFocusMatchKind::File,
    }
}

fn layer_ids_by_atom(stack: &ReviewStack) -> BTreeMap<String, String> {
    stack
        .layers
        .iter()
        .flat_map(|layer| {
            layer
                .atom_ids
                .iter()
                .map(|atom_id| (atom_id.clone(), layer.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn atom_matches_path(atom: &ChangeAtom, file_path: &str, preferred_left: bool) -> bool {
    if preferred_left {
        atom.previous_path.as_deref() == Some(file_path) || atom.path == file_path
    } else {
        atom.path == file_path
    }
}

fn range_contains_line(range: LineRange, line: usize) -> bool {
    let Ok(line) = i64::try_from(line) else {
        return false;
    };
    range.start <= line && line <= range.end
}

fn range_len(range: LineRange) -> i64 {
    (range.end - range.start).abs()
}

fn focus_subtitle(
    file_path: &str,
    line: Option<usize>,
    _match_kind: ReviewPartnerFocusMatchKind,
) -> String {
    location_title(file_path, line)
}

fn location_title(file_path: &str, line: Option<usize>) -> String {
    line.map(|line| format!("{file_path}:{line}"))
        .unwrap_or_else(|| file_path.to_string())
}

fn short_hash(value: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(value.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    hash.chars().take(12).collect()
}

fn build_prompt_context(input: &GenerateReviewPartnerInput) -> Value {
    let mut issue_texts = vec![input.title.as_str(), input.body.as_str()];
    issue_texts.extend(input.comments.iter().map(|comment| comment.body.as_str()));
    issue_texts.extend(
        input
            .latest_reviews
            .iter()
            .map(|review| review.body.as_str()),
    );
    issue_texts.extend(
        input
            .review_threads
            .iter()
            .flat_map(|thread| thread.comments.iter().map(|comment| comment.body.as_str())),
    );

    json!({
        "repository": input.repository,
        "workingDirectory": input.working_directory,
        "pullRequest": {
            "number": input.number,
            "title": input.title,
            "url": input.url,
            "baseRefName": input.base_ref_name,
            "headRefName": input.head_ref_name,
            "body": trim_text(&input.body, 2_500),
        },
        "partnerVersion": REVIEW_PARTNER_GENERATOR_VERSION,
        "contextVersion": input.context.version,
        "structuralEvidenceVersion": input.structural_evidence.version,
        "focusTargets": input.focus_targets.iter().map(summarize_focus_target).collect::<Vec<_>>(),
        "historyContext": {
            "signals": input.review_memory.signals,
            "limitations": input.review_memory.limitations,
            "commitMessages": [],
            "linkedIssues": crate::agents::prompt::linked_issue_refs(issue_texts),
            "prComments": input
                .comments
                .iter()
                .take(MAX_PARTNER_LAYERS)
                .map(|comment| json!({
                    "authorLogin": comment.author_login,
                    "createdAt": comment.created_at,
                    "body": trim_text(&comment.body, MAX_PROMPT_SNIPPET_CHARS),
                }))
                .collect::<Vec<_>>(),
            "currentReviewThreadsForTouchedFiles": input
                .review_threads
                .iter()
                .take(MAX_PARTNER_LAYERS)
                .map(|thread| json!({
                    "path": thread.path,
                    "line": thread.line,
                    "diffSide": thread.diff_side,
                    "subjectType": thread.subject_type,
                    "isResolved": thread.is_resolved,
                }))
                .collect::<Vec<_>>(),
            "olderReviewThreadsForTouchedFiles": [],
            "recentChangesToTouchedFiles": [],
            "knownPriorPatterns": [],
        },
        "stack": {
            "id": input.stack.id,
            "source": input.stack.source,
            "kind": input.stack.kind,
            "generatorVersion": input.stack.generator_version,
            "layers": input.stack.layers.iter().take(MAX_PARTNER_LAYERS).map(|layer| {
                json!({
                    "id": layer.id,
                    "index": layer.index,
                    "title": layer.title,
                    "summary": layer.summary,
                    "rationale": layer.rationale,
                    "atomIds": layer.atom_ids.iter().take(MAX_LAYER_ATOMS).collect::<Vec<_>>(),
                    "dependsOnLayerIds": layer.depends_on_layer_ids,
                    "metrics": layer.metrics,
                    "confidence": layer.confidence,
                })
            }).collect::<Vec<_>>(),
            "atoms": input.stack.atoms.iter().map(|atom| {
                json!({
                    "id": atom.id,
                    "path": atom.path,
                    "previousPath": atom.previous_path,
                    "role": atom.role.label(),
                    "semanticKind": atom.semantic_kind,
                    "symbolName": atom.symbol_name,
                    "definedSymbols": atom.defined_symbols,
                    "referencedSymbols": atom.referenced_symbols.iter().take(12).collect::<Vec<_>>(),
                    "oldRange": atom.old_range,
                    "newRange": atom.new_range,
                    "changedLineCount": atom.additions + atom.deletions,
                    "riskScore": atom.risk_score,
                })
            }).collect::<Vec<_>>(),
        },
        "collectedContext": summarize_context_pack(&input.context),
        "structuralEvidence": summarize_structural_evidence(&input.structural_evidence),
        "semanticEvidence": summarize_semantic_evidence(input.semantic_review.as_ref()),
    })
}

fn summarize_focus_target(target: &ReviewPartnerFocusTarget) -> Value {
    json!({
        "key": target.key.as_str(),
        "filePath": target.file_path.as_str(),
        "hunkHeader": target.hunk_header.as_deref(),
        "hunkIndex": target.hunk_index,
        "line": target.line,
        "side": target.side.as_deref(),
        "atomIds": &target.atom_ids,
        "layerId": target.layer_id.as_deref(),
        "title": target.title.as_str(),
        "subtitle": target.subtitle.as_str(),
        "matchKind": target.match_kind,
    })
}

fn summarize_partner_layer_for_prompt(layer: &ReviewPartnerLayer) -> Value {
    json!({
        "layerId": layer.layer_id,
        "title": layer.title,
        "brief": layer.brief,
        "changedItems": layer.changed_items,
        "removedItems": layer.removed_items,
        "similarCode": layer.similar_code,
        "codebaseFit": layer.codebase_fit,
        "concerns": layer.concerns,
        "limitations": layer.limitations,
        "structuralEvidenceStatus": layer.structural_evidence_status,
    })
}

fn summarize_context_pack(context: &ReviewPartnerContextPack) -> Value {
    json!({
        "version": context.version,
        "warnings": context.warnings,
        "layers": context.layers.iter().map(|layer| {
            json!({
                "layerId": layer.layer_id,
                "semanticLayers": layer.semantic_layers,
                "semanticFocus": layer.semantic_focus,
                "changedSymbols": layer.changed_symbols.iter().map(summarize_collected_symbol).collect::<Vec<_>>(),
                "removedSymbols": layer.removed_symbols.iter().map(summarize_collected_symbol).collect::<Vec<_>>(),
                "similarLocations": layer.similar_locations,
                "styleNotes": layer.style_notes,
                "limitations": layer.limitations,
            })
        }).collect::<Vec<_>>(),
    })
}

fn summarize_collected_symbol(symbol: &ReviewPartnerCollectedSymbol) -> Value {
    json!({
        "symbol": symbol.symbol,
        "path": symbol.path,
        "line": symbol.line,
        "atomIds": symbol.atom_ids,
        "searchStrategy": symbol.search_strategy,
        "referenceCount": symbol.reference_count,
    })
}

fn summarize_structural_evidence(evidence: &StructuralEvidencePack) -> Value {
    let mut emitted_changes = 0usize;
    let files = evidence
        .files
        .iter()
        .take(MAX_EVIDENCE_FILES)
        .map(|file| {
            let remaining = MAX_EVIDENCE_CHANGES.saturating_sub(emitted_changes);
            let changes = file
                .changes
                .iter()
                .take(remaining)
                .map(|change| {
                    emitted_changes += 1;
                    json!({
                        "hunkIndex": change.hunk_index,
                        "hunkHeader": change.hunk_header,
                        "oldRange": change.old_range,
                        "newRange": change.new_range,
                        "atomIds": change.atom_ids,
                        "changedLineCount": change.changed_line_count,
                        "snippet": change.snippet.as_deref().map(|snippet| trim_text(snippet, MAX_PROMPT_SNIPPET_CHARS)),
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "path": file.path,
                "previousPath": file.previous_path,
                "status": file.status,
                "message": file.message,
                "matchedAtomIds": file.matched_atom_ids,
                "unmatchedHunkCount": file.unmatched_hunk_count,
                "changes": changes,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "version": evidence.version,
        "warnings": evidence.warnings,
        "files": files,
    })
}

fn summarize_semantic_evidence(evidence: Option<&RemissSemanticReviewSummary>) -> Value {
    const MAX_SEMANTIC_LAYERS: usize = 24;
    const MAX_SEMANTIC_ATOMS: usize = 32;
    const MAX_SEMANTIC_FILES: usize = 12;
    const MAX_SEMANTIC_ENTITIES: usize = 16;
    const MAX_SEMANTIC_WARNINGS: usize = 12;
    const MAX_SEMANTIC_FOCUS: usize = 24;

    let Some(evidence) = evidence else {
        return json!({
            "status": "unavailable",
            "warnings": ["Semantic evidence was not available."],
            "layers": [],
        });
    };

    json!({
        "status": if evidence.layer_count > 0 { "ready" } else { "empty" },
        "version": evidence.version,
        "semApiVersion": evidence.sem_api_version,
        "codeVersionKey": evidence.code_version_key,
        "analysisCacheKey": evidence.analysis_cache_key,
        "layerCacheKey": evidence.layer_cache_key,
        "summary": {
            "fileCount": evidence.file_count,
            "changeCount": evidence.change_count,
            "layerCount": evidence.layer_count,
            "addedCount": evidence.added_count,
            "modifiedCount": evidence.modified_count,
            "deletedCount": evidence.deleted_count,
            "movedCount": evidence.moved_count,
            "renamedCount": evidence.renamed_count,
            "reorderedCount": evidence.reordered_count,
            "orphanCount": evidence.orphan_count,
        },
        "warnings": evidence.warnings.iter().take(MAX_SEMANTIC_WARNINGS).collect::<Vec<_>>(),
        "focus": evidence.focus_summaries.iter().take(MAX_SEMANTIC_FOCUS).collect::<Vec<_>>(),
        "layers": evidence.layers.iter().take(MAX_SEMANTIC_LAYERS).map(|layer| {
            json!({
                "id": layer.id,
                "index": layer.index,
                "title": layer.title,
                "summary": trim_text(&layer.summary, MAX_ITEM_TEXT_CHARS),
                "rationale": trim_text(&layer.rationale, MAX_ITEM_TEXT_CHARS),
                "dependsOnLayerIds": layer.depends_on_layer_ids,
                "atomIds": layer.atom_ids.iter().take(MAX_SEMANTIC_ATOMS).collect::<Vec<_>>(),
                "filePaths": layer.file_paths.iter().take(MAX_SEMANTIC_FILES).collect::<Vec<_>>(),
                "hunkIndices": layer.hunk_indices,
                "entityNames": layer.entity_names.iter().take(MAX_SEMANTIC_ENTITIES).collect::<Vec<_>>(),
                "changeCount": layer.change_count,
            })
        }).collect::<Vec<_>>(),
    })
}

fn review_partner_output_schema() -> Value {
    let item_schema = json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "detail": { "type": "string" },
            "path": { "type": ["string", "null"] },
            "line": { "type": ["integer", "null"] }
        },
        "required": ["title", "detail"],
        "additionalProperties": false
    });
    let codebase_fit_schema = json!({
        "type": "object",
        "properties": {
            "follows": { "type": "boolean" },
            "summary": { "type": "string" },
            "evidence": { "type": "array", "items": item_schema.clone() }
        },
        "required": ["follows", "summary", "evidence"],
        "additionalProperties": false
    });
    let focus_section_schema = json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "items": { "type": "array", "items": item_schema.clone() }
        },
        "required": ["title", "items"],
        "additionalProperties": false
    });
    let focus_record_schema = json!({
        "type": "object",
        "properties": {
            "key": { "type": "string" },
            "title": { "type": "string" },
            "subtitle": { "type": ["string", "null"] },
            "summary": { "type": "string" },
            "codebaseFit": codebase_fit_schema,
            "sections": { "type": "array", "items": focus_section_schema },
            "understandingCheckpoints": { "type": "array", "items": item_schema.clone() },
            "assumptions": { "type": "array", "items": item_schema.clone() },
            "historySignals": { "type": "array", "items": item_schema.clone() }
        },
        "required": ["key", "title", "summary", "codebaseFit", "sections", "understandingCheckpoints", "assumptions", "historySignals"],
        "additionalProperties": false
    });

    json!({
        "type": "object",
        "properties": {
            "stackBrief": { "type": "string" },
            "stackConcerns": { "type": "array", "items": item_schema },
            "warnings": { "type": "array", "items": { "type": "string" } },
            "layers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "layerId": { "type": "string" },
                        "brief": { "type": "string" },
                        "changedItems": { "type": "array", "items": item_schema },
                        "removedItems": { "type": "array", "items": item_schema },
                        "similarCode": { "type": "array", "items": item_schema },
                        "codebaseFit": { "type": "array", "items": item_schema },
                        "concerns": { "type": "array", "items": item_schema }
                    },
                    "required": ["layerId", "brief", "changedItems", "removedItems", "similarCode", "codebaseFit", "concerns"],
                    "additionalProperties": false
                }
            },
            "focusRecords": { "type": "array", "items": focus_record_schema }
        },
        "required": ["stackBrief", "stackConcerns", "warnings", "layers", "focusRecords"],
        "additionalProperties": false
    })
}

fn focus_record_output_schema() -> Value {
    let item_schema = json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "detail": { "type": "string" },
            "path": { "type": ["string", "null"] },
            "line": { "type": ["integer", "null"] }
        },
        "required": ["title", "detail"],
        "additionalProperties": false
    });
    let codebase_fit_schema = json!({
        "type": "object",
        "properties": {
            "follows": { "type": "boolean" },
            "summary": { "type": "string" },
            "evidence": { "type": "array", "items": item_schema.clone() }
        },
        "required": ["follows", "summary", "evidence"],
        "additionalProperties": false
    });
    let section_schema = json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "items": { "type": "array", "items": item_schema }
        },
        "required": ["title", "items"],
        "additionalProperties": false
    });

    json!({
        "type": "object",
        "properties": {
            "record": {
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "title": { "type": "string" },
                    "subtitle": { "type": ["string", "null"] },
                    "summary": { "type": "string" },
                    "codebaseFit": codebase_fit_schema,
                    "sections": { "type": "array", "items": section_schema },
                    "understandingCheckpoints": { "type": "array", "items": item_schema.clone() },
                    "assumptions": { "type": "array", "items": item_schema.clone() },
                    "historySignals": { "type": "array", "items": item_schema.clone() }
                },
                "required": ["key", "title", "summary", "codebaseFit", "sections", "understandingCheckpoints", "assumptions", "historySignals"],
                "additionalProperties": false
            }
        },
        "required": ["record"],
        "additionalProperties": false
    })
}

fn build_focus_record_prompt(
    document: &GeneratedReviewPartnerContext,
    target: &ReviewPartnerFocusTarget,
) -> String {
    let schema =
        serde_json::to_string_pretty(&focus_record_output_schema()).expect("schema must serialize");
    let context = serde_json::to_string_pretty(&json!({
        "repository": document.stack.repository.as_str(),
        "pullRequestNumber": document.stack.selected_pr_number,
        "target": summarize_focus_target(target),
        "targetLayer": target
            .layer_id
            .as_deref()
            .and_then(|layer_id| document.layer(layer_id))
            .map(summarize_partner_layer_for_prompt),
        "targetAtoms": target.atom_ids.iter().filter_map(|atom_id| document.stack.atom(atom_id)).collect::<Vec<_>>(),
        "historyContext": {
            "signals": document.review_memory.signals,
            "limitations": document.review_memory.limitations,
        },
        "collectedContext": summarize_context_pack(&document.context),
        "structuralEvidence": summarize_structural_evidence(&document.structural_evidence),
        "semanticEvidence": summarize_semantic_evidence(document.semantic_review.as_ref()),
    }))
    .expect("context must serialize");

    [
        "You are generating one compact code explanation record for Remiss.",
        "This record appears in the right rail for the selected stack layer.",
        "The goal is explaining the scoped code, not assigning work or asking review questions.",
        "Return only context the reader cannot infer from the visible diff alone.",
        "Use historyContext.signals only as evidence-backed review memory. Do not treat prior signals as current truth when the current code contradicts them.",
        "When historyContext conflicts with current code, surface the conflict in assumptions, historySignals, or limitations instead of resolving it silently.",
        "Set record.key to the exact supplied target.key string. Do not use layerId, atom id, title, file path, or a generated key in that field.",
        "Avoid emoji, markdown headings, decorative labels, code fences, and code sketches.",
        "Include one complete natural-language summary paragraph that synthesizes what changed, how the code behaves, the invariant/state change/error handling it affects, the supported intent or trade-off, and any relevant history signal.",
        "The summary must not be a file inventory, line list, stack-generator explanation, or statement about how Remiss grouped the change.",
        "Do not name changed files in the summary unless one specific file is itself the behavior being explained. Prefer the subsystem, flow, symbol contract, state, or invariant.",
        "Never write placeholder scaffolding such as 'the useful meaning is', 'what state changes', or 'which invariant'. Fill in the concrete behavior or leave the uncertainty to assumptions.",
        "Include only understandingCheckpoints that help the reviewer understand or verify the code, not generic review advice.",
        "A checkpoint should name the concrete invariant, edge case, assumption, or codebase pattern the reviewer should keep in mind.",
        "Use assumptions for inferred intent or behavior that is plausible but not directly proven by the supplied context.",
        "Use historySignals only for prior PRs, older behavior, supplied historyContext.signals, or verified historical context. Do not turn discussion from the current pull request into History rows.",
        "Distinguish visible behavior from inferred intent and from unverifiable history.",
        "If generated, mechanical, or broad AI-assisted changes are present, call out the human verification surface: edge cases, invariants, and callsites that cannot be trusted from generation alone.",
        "Write the summary as factual code explanation, never as a question, instruction, checklist item, or review task.",
        "Rewrite any question-shaped draft into a declarative explanation before returning JSON.",
        "Never end a summary with an ellipsis.",
        "Match the supplied focus scope exactly. Ground intent in the code, diff, collected context, or review memory.",
        "Use semanticEvidence as internal code-structure evidence. Never mention Sem, semanticEvidence, internal tooling, atom IDs, layer IDs, semantic targets, loose file buckets, or grouping mechanics in user-facing fields.",
        "If intent or history is not supported by the supplied context, put the gap in assumptions or historySignals instead of inventing it in the summary.",
        "Usage rows are generated by Remiss from tree-sitter syntax context. Leave usage lists out of the JSON.",
        "Use codebaseFit only for grounded mismatch evidence and only the 2-3 strongest non-empty secondary sections.",
        "Use compact prose rows, not checklist or bullet phrasing.",
        "Keep Usage context and Codebase fit out of sections.",
        "For codebaseFit, set follows=true when the supplied context does not support a concrete mismatch. If follows=false, every evidence item must link to the existing code location that shows the mismatch.",
        "Keep stack-wide prose, checklists, and generic review advice out of this record.",
        "Use item.path and item.line only when grounded in the supplied context.",
        "Return strict JSON only. No markdown fences or prose outside JSON.",
        "",
        "JSON schema:",
        &schema,
        "",
        "Focus context:",
        &context,
    ]
    .join("\n")
}

fn normalize_focus_sections(
    values: Vec<ReviewPartnerFocusSectionResponse>,
) -> (Vec<ReviewPartnerFocusSection>, Vec<ReviewPartnerItem>) {
    let mut sections = Vec::new();
    let mut codebase_fit_items = Vec::new();

    for section in values {
        let title = limit_text(section.title, MAX_FOCUS_TITLE_CHARS);
        let items = normalize_items(section.items);
        if title.trim().is_empty() || items.is_empty() {
            continue;
        }

        match title.trim().to_ascii_lowercase().as_str() {
            "usage context" => {}
            "codebase fit" => codebase_fit_items.extend(items),
            "changed items" | "changed symbols" => {}
            _ if sections.len() < MAX_FOCUS_SECTIONS => {
                sections.push(ReviewPartnerFocusSection { title, items })
            }
            _ => {}
        }
    }

    (sections, codebase_fit_items)
}

fn normalize_usage_groups(
    values: Vec<ReviewPartnerUsageGroupResponse>,
) -> Vec<ReviewPartnerUsageGroup> {
    values
        .into_iter()
        .filter_map(|group| {
            let symbol = group.symbol.trim();
            let summary = group.summary.trim();
            let usages = normalize_items(group.usages);
            if (symbol.is_empty() && summary.is_empty()) || usages.is_empty() {
                return None;
            }

            Some(ReviewPartnerUsageGroup::new(
                if symbol.is_empty() { summary } else { symbol },
                if summary.is_empty() {
                    format!(
                        "{} usage{} surfaced.",
                        usages.len(),
                        if usages.len() == 1 { "" } else { "s" }
                    )
                } else {
                    summary.to_string()
                },
                usages,
            ))
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn normalize_codebase_fit(response: ReviewPartnerCodebaseFitResponse) -> ReviewPartnerCodebaseFit {
    let evidence = normalize_items(response.evidence)
        .into_iter()
        .filter(|item| item.path.is_some())
        .take(MAX_SECTION_ITEMS)
        .collect::<Vec<_>>();
    if response.follows || evidence.is_empty() {
        return ReviewPartnerCodebaseFit::default();
    }

    ReviewPartnerCodebaseFit {
        follows: false,
        summary: default_if_empty(response.summary, "does not fully follow codebase style"),
        evidence,
    }
}

fn codebase_fit_from_items(_items: Vec<ReviewPartnerItem>) -> ReviewPartnerCodebaseFit {
    ReviewPartnerCodebaseFit::default()
}

fn usage_groups_for_target(
    target: &ReviewPartnerFocusTarget,
    context: &ReviewPartnerContextPack,
) -> Vec<ReviewPartnerUsageGroup> {
    let Some(layer_id) = target.layer_id.as_deref() else {
        return Vec::new();
    };
    let Some(layer) = context.layer(layer_id) else {
        return Vec::new();
    };

    layer
        .changed_symbols
        .iter()
        .chain(layer.removed_symbols.iter())
        .filter_map(|symbol| {
            let usages = symbol
                .references
                .iter()
                .filter(|location| {
                    target.match_kind == ReviewPartnerFocusMatchKind::Layer
                        || target
                            .atom_ids
                            .iter()
                            .any(|atom_id| symbol.atom_ids.contains(atom_id))
                        || location.path == target.file_path
                })
                .take(MAX_SECTION_ITEMS)
                .map(|location| {
                    ReviewPartnerItem::new(
                        location_title(&location.path, Some(location.line)),
                        location
                            .snippet
                            .clone()
                            .unwrap_or_else(|| format!("Occurrence in {}", location.path)),
                        Some(location.path.clone()),
                        Some(location.line),
                    )
                })
                .collect::<Vec<_>>();
            if usages.is_empty() {
                return None;
            }
            Some(ReviewPartnerUsageGroup::new(
                symbol.symbol.clone(),
                format!(
                    "{} syntax occurrence{} surfaced by tree-sitter.",
                    usages.len(),
                    if usages.len() == 1 { "" } else { "s" }
                ),
                usages,
            ))
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn fallback_focus_summary(
    target: &ReviewPartnerFocusTarget,
    layer: Option<&ReviewPartnerLayer>,
) -> String {
    let summary = layer
        .map(|layer| default_if_empty(layer.brief.clone(), &target.title))
        .unwrap_or_else(|| target.title.clone());
    normalize_focus_summary(target, &summary)
}

fn normalize_items_or(
    values: Vec<ReviewPartnerItemResponse>,
    fallback: Vec<ReviewPartnerItem>,
) -> Vec<ReviewPartnerItem> {
    let normalized = normalize_items(values);
    if normalized.is_empty() {
        fallback
    } else {
        normalized
    }
}

fn normalize_items(values: Vec<ReviewPartnerItemResponse>) -> Vec<ReviewPartnerItem> {
    values
        .into_iter()
        .filter_map(|item| {
            let title = item.title.trim();
            let detail = item.detail.trim();
            if title.is_empty() && detail.is_empty() {
                return None;
            }
            Some(ReviewPartnerItem::new(
                if title.is_empty() { detail } else { title },
                detail,
                item.path,
                item.line,
            ))
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn normalize_text_items(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| limit_text(value, MAX_LIMITATION_TEXT_CHARS))
        .filter(|value| !value.trim().is_empty())
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn fallback_stack_brief(stack: &ReviewStack) -> String {
    format!(
        "{} stack layer{} prepared with bounded usage and codebase context.",
        stack.layers.len(),
        if stack.layers.len() == 1 { "" } else { "s" }
    )
}

fn default_if_empty(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        limit_text(trimmed, MAX_BRIEF_TEXT_CHARS)
    }
}
