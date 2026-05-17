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
    code_tour::{find_parsed_diff_file, tour_code_version_key, CodeTourProvider},
    diff::{DiffLineKind, ParsedDiffFile},
    github::PullRequestDetail,
    lsp::{LspSessionManager, LspTextDocumentRequest},
    semantic_review::{
        summarize_semantic_review, RemissSemanticFocusSummary, RemissSemanticLayerSummary,
        RemissSemanticReview, RemissSemanticReviewSummary,
    },
    stacks::model::{
        ChangeAtom, LineRange, ReviewStack, ReviewStackLayer, STACK_GENERATOR_VERSION,
    },
    structural_evidence::{StructuralEvidencePack, StructuralEvidenceStatus},
};

mod context;

#[cfg(test)]
mod tests;

use self::context::*;

pub const REVIEW_PARTNER_GENERATOR_VERSION: &str = "review-partner-v15";
pub const REVIEW_PARTNER_CONTEXT_VERSION: &str = "review-partner-context-v5";

const REVIEW_PARTNER_CACHE_KEY_PREFIX: &str = "review-partner-v15";
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
const MAX_BRIEF_TEXT_CHARS: usize = 1200;
const MAX_ITEM_TEXT_CHARS: usize = 260;
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
        Self {
            title: limit_text(title.into(), MAX_ITEM_TEXT_CHARS),
            detail: limit_text(detail.into(), MAX_ITEM_TEXT_CHARS),
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
    pub stack: ReviewStack,
    pub structural_evidence: StructuralEvidencePack,
    pub semantic_review: Option<RemissSemanticReviewSummary>,
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
    let response = agents::run_json_prompt_with_options(
        input.provider,
        &input.working_directory,
        prompt,
        AgentJsonPromptOptions::review_partner(),
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
        stack,
        structural_evidence,
        semantic_review,
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
        "The goal is explaining the scoped code. Produce code explanations, not review prompts or assignments.",
        "The virtual stack layers are already validated. Preserve layer order, layer IDs, and atom coverage.",
        "Avoid checklists, verdict tables, evidence ledgers, pass/fail reports, tutorials, walkthroughs, and generic guides.",
        "Avoid emoji, markdown headings, decorative labels, code fences, and code sketches.",
        "Return compact right-rail explanation the reader cannot infer from the visible diff alone: concrete stack-layer summary, removed-code impact, similar existing code, grounded codebase-fit mismatch, and concrete implementation concerns when supported.",
        "Generate focusRecords for the supplied focusTargets. Each focus record explains one stack layer, not one diff hunk.",
        "Each focus record must include one complete concrete summary paragraph that explains what changed in this stack layer, how the code behaves, and why this layer is relevant.",
        "Write the summary as factual code explanation, never as a question, instruction, checklist item, or review task.",
        "Rewrite any question-shaped draft into a declarative explanation before returning JSON.",
        "Never end a summary with an ellipsis.",
        "Match the supplied focus scope exactly. Ground intent in the code, diff, or collected context.",
        "Use semanticEvidence and collectedContext.semanticFocus first for entity-level context when it directly overlaps this focus scope.",
        "Usage rows are generated by Remiss from tree-sitter syntax context. Leave usage lists out of the JSON.",
        "Use codebaseFit only for grounded mismatch evidence and only the 2-3 strongest non-empty secondary sections.",
        "Use compact prose rows, not checklist or bullet phrasing.",
        "Keep Usage context and Codebase fit out of sections.",
        "For codebaseFit, set follows=true when the collected context does not support a concrete mismatch. If follows=false, every evidence item must link to the existing code location that shows the mismatch.",
        "Keep stack-wide prose out of focus records. Repeat the layer brief only when it is the only useful context.",
        "Use the collectedContext as bounded read-only investigation. Treat partial context as partial.",
        "Use semanticEvidence as deterministic Sem context for entity-level grouping, moved or reordered code, layer-to-atom mappings, focus entities, and impact context when it is present.",
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
            response_focus_records
                .remove(&target.key)
                .map(|record| merge_focus_record(target, record, &input.context))
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
        context: input.context.clone(),
        layers,
        focus_targets: input.focus_targets.clone(),
        focus_records,
    })
}

fn merge_layer(
    layer: &ReviewStackLayer,
    response: ReviewPartnerLayerResponse,
    input: &GenerateReviewPartnerInput,
) -> ReviewPartnerLayer {
    let fallback = fallback_layer(layer, input);
    let _legacy_usage_context = response.usage_context;
    ReviewPartnerLayer {
        layer_id: layer.id.clone(),
        title: layer.title.clone(),
        brief: default_if_empty(response.brief, &fallback.brief),
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
        title: layer.title.clone(),
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
    context
        .and_then(|context| semantic_layer_brief(layer, context))
        .unwrap_or_else(|| default_if_empty(layer.summary.clone(), &layer.title))
}

fn semantic_layer_brief(
    layer: &ReviewStackLayer,
    context: &ReviewPartnerCollectedLayer,
) -> Option<String> {
    if context.semantic_layers.is_empty() {
        return None;
    }

    let mut files = BTreeSet::<String>::new();
    let mut entities = BTreeSet::<String>::new();
    let mut change_count = 0usize;

    for semantic_layer in &context.semantic_layers {
        files.extend(semantic_layer.file_paths.iter().cloned());
        entities.extend(
            semantic_layer
                .entity_names
                .iter()
                .filter(|name| !name.trim().is_empty())
                .cloned(),
        );
        change_count += semantic_layer.change_count;
    }
    for focus in &context.semantic_focus {
        if let Some(entity) = focus
            .target_entity
            .as_ref()
            .or_else(|| focus.overlapping_entities.first())
        {
            entities.insert(entity.name.clone());
            files.insert(entity.file_path.clone());
        }
    }

    if files.is_empty() && entities.is_empty() {
        return None;
    }

    let action = layer_action_verb(&layer.title);
    let subject = layer_subject(&layer.title);
    let entity_clause = if entities.is_empty() {
        String::new()
    } else {
        format!(
            " around {}",
            natural_list(entities.iter().map(String::as_str), 3)
        )
    };
    let file_clause = if files.is_empty() {
        String::new()
    } else {
        format!(" in {}", natural_list(files.iter().map(String::as_str), 3))
    };
    let sem_clause = if change_count == 0 {
        "Sem groups these edits by semantic target rather than by a loose file bucket.".to_string()
    } else {
        format!(
            "Sem ties {} to the same semantic target rather than a loose file bucket.",
            if change_count == 1 {
                "one changed entity".to_string()
            } else {
                format!("{change_count} changed entities")
            }
        )
    };

    Some(limit_text(
        format!("{action} {subject}{entity_clause}{file_clause}. {sem_clause}"),
        MAX_BRIEF_TEXT_CHARS,
    ))
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

fn natural_list<'a>(values: impl Iterator<Item = &'a str>, max_items: usize) -> String {
    let values = values
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return "this area".to_string();
    }

    let visible = values.iter().take(max_items).copied().collect::<Vec<_>>();
    let extra_count = values.len().saturating_sub(visible.len());
    match visible.as_slice() {
        [one] if extra_count == 0 => (*one).to_string(),
        [one] => format!(
            "{one} plus {extra_count} other{}",
            if extra_count == 1 { "" } else { "s" }
        ),
        [one, two] if extra_count == 0 => format!("{one} and {two}"),
        _ => {
            let mut text = visible.join(", ");
            if extra_count == 0 {
                if let Some((head, tail)) = text.rsplit_once(", ") {
                    text = format!("{head}, and {tail}");
                }
                text
            } else {
                format!(
                    "{text}, plus {extra_count} other{}",
                    if extra_count == 1 { "" } else { "s" }
                )
            }
        }
    }
}

fn merge_focus_record(
    target: &ReviewPartnerFocusTarget,
    response: ReviewPartnerFocusRecordResponse,
    context: &ReviewPartnerContextPack,
) -> ReviewPartnerFocusRecord {
    let _legacy_usage_context = normalize_usage_groups(response.usage_context);
    let (sections, legacy_codebase_fit_items) = normalize_focus_sections(response.sections);
    let usage_context = usage_groups_for_target(target, context);
    let codebase_fit = response
        .codebase_fit
        .map(normalize_codebase_fit)
        .unwrap_or_else(|| codebase_fit_from_items(legacy_codebase_fit_items));
    let summary = response
        .summary
        .map(|summary| normalize_focus_summary(target, &summary))
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or_else(|| target.title.clone());
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
        limitations: normalize_text_items(response.limitations),
        generated_at_ms: now_ms(),
    }
}

fn normalize_focus_summary(target: &ReviewPartnerFocusTarget, summary: &str) -> String {
    let summary = summary.trim().trim_end_matches("...").trim_end();
    if summary.is_empty() {
        return target.title.clone();
    }

    if let Some((_, remainder)) = summary.split_once('?') {
        let remainder = remainder.trim();
        if !remainder.is_empty() {
            return normalize_focus_summary(target, remainder);
        }
        return target.title.clone();
    }

    if review_partner_summary_starts_like_prompt(summary) {
        return target.title.clone();
    }

    summary.to_string()
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
        .map(|layer| layer.limitations)
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
        limitations: limitations.into_iter().take(MAX_SECTION_ITEMS).collect(),
        generated_at_ms: now_ms(),
    }
}

pub fn generate_review_partner_focus_record(
    document: &GeneratedReviewPartnerContext,
    target: ReviewPartnerFocusTarget,
    working_directory: &str,
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
    let response = agents::run_json_prompt_with_options(
        document.provider,
        working_directory,
        prompt,
        AgentJsonPromptOptions::review_partner_focus(),
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
        title: limit_text(layer.title.clone(), MAX_FOCUS_TITLE_CHARS),
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
        "focusTargets": input.focus_targets.iter().map(summarize_focus_target).collect::<Vec<_>>(),
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
            "sections": { "type": "array", "items": focus_section_schema }
        },
        "required": ["key", "title", "summary", "codebaseFit", "sections"],
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
                    "sections": { "type": "array", "items": section_schema }
                },
                "required": ["key", "title", "summary", "codebaseFit", "sections"],
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
        "Avoid emoji, markdown headings, decorative labels, code fences, and code sketches.",
        "Include one complete concrete summary paragraph that explains what changed in this stack layer, how the code behaves, and why this layer is relevant.",
        "Write the summary as factual code explanation, never as a question, instruction, checklist item, or review task.",
        "Rewrite any question-shaped draft into a declarative explanation before returning JSON.",
        "Never end a summary with an ellipsis.",
        "Match the supplied focus scope exactly. Ground intent in the code, diff, or collected context.",
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

fn symbol_position_in_document(
    document: &str,
    range: Option<LineRange>,
    symbol: &str,
) -> Option<(usize, usize)> {
    if let Some(line) = range.and_then(line_from_range) {
        if let Some(column) = column_for_symbol(document, line, symbol) {
            return Some((line, column));
        }
    }

    document
        .lines()
        .enumerate()
        .find_map(|(index, line)| identifier_column(line, symbol).map(|column| (index + 1, column)))
}

fn column_for_symbol(document: &str, line: usize, symbol: &str) -> Option<usize> {
    let line_text = document.lines().nth(line.checked_sub(1)?)?;
    identifier_column(line_text, symbol)
}

fn identifier_column(line: &str, symbol: &str) -> Option<usize> {
    let byte_index = line.find(symbol)?;
    if !identifier_bounds_match(line, byte_index, symbol.len()) {
        return None;
    }
    Some(line[..byte_index].chars().count() + 1)
}

fn line_from_range(range: LineRange) -> Option<usize> {
    usize::try_from(range.start).ok().filter(|line| *line > 0)
}

fn read_checkout_line(checkout_root: &Path, path: &str, line: usize) -> Option<String> {
    let text = fs::read_to_string(checkout_root.join(path)).ok()?;
    text.lines()
        .nth(line.checked_sub(1)?)
        .map(|line| trim_text(line, MAX_PROMPT_SNIPPET_CHARS))
}

fn clean_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
        .to_string()
}

fn is_searchable_symbol(symbol: &str) -> bool {
    symbol.len() > 2 && symbol.chars().any(|ch| ch.is_ascii_alphabetic()) && !is_keyword(symbol)
}

fn declaration_symbol(line: &str) -> Option<String> {
    let trimmed = line
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("export ")
        .trim_start_matches("default ");
    for prefix in [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "type ",
        "const ",
        "static ",
        "class ",
        "function ",
        "interface ",
        "def ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let symbol = rest
                .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
                .find(|token| is_searchable_symbol(token))?;
            return Some(symbol.to_string());
        }
    }
    None
}

fn similar_search_token(symbol: &str) -> Option<String> {
    let parts = split_symbol_parts(symbol);
    parts
        .into_iter()
        .filter(|part| part.len() >= 4 && !is_keyword(part))
        .max_by_key(|part| part.len())
}

fn split_symbol_parts(symbol: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in symbol.chars() {
        if ch == '_' || ch == ':' || ch == '-' {
            if current.len() > 1 {
                parts.push(current.to_lowercase());
            }
            current.clear();
            continue;
        }
        if ch.is_ascii_uppercase() && !current.is_empty() {
            parts.push(current.to_lowercase());
            current.clear();
        }
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        }
    }
    if current.len() > 1 {
        parts.push(current.to_lowercase());
    }
    if parts.is_empty() && symbol.len() >= 4 {
        parts.push(symbol.to_lowercase());
    }
    parts
}

fn contains_identifier(line: &str, symbol: &str) -> bool {
    let mut start = 0usize;
    while let Some(relative) = line[start..].find(symbol) {
        let index = start + relative;
        if identifier_bounds_match(line, index, symbol.len()) {
            return true;
        }
        start = index + symbol.len();
        if start >= line.len() {
            break;
        }
    }
    false
}

fn identifier_bounds_match(line: &str, byte_index: usize, len: usize) -> bool {
    let before = line[..byte_index].chars().next_back();
    let after = line[byte_index + len..].chars().next();
    !before.map(is_identifier_char).unwrap_or(false)
        && !after.map(is_identifier_char).unwrap_or(false)
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_keyword(symbol: &str) -> bool {
    matches!(
        symbol,
        "let"
            | "mut"
            | "pub"
            | "fn"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "type"
            | "const"
            | "static"
            | "self"
            | "Self"
            | "crate"
            | "super"
            | "return"
            | "async"
            | "await"
            | "function"
            | "class"
            | "interface"
            | "import"
            | "export"
            | "from"
            | "def"
    )
}

fn should_skip_directory(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".next"
            | "dist"
            | "build"
            | ".swiftpm"
            | "DerivedData"
    )
}

fn is_text_search_candidate(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension,
        "rs" | "swift"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "go"
            | "py"
            | "rb"
            | "java"
            | "kt"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "m"
            | "mm"
            | "cs"
            | "php"
            | "scala"
            | "md"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
    )
}

fn relative_path(root: &Path, path: &Path) -> String {
    normalize_repo_path(
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .as_ref(),
    )
}

fn normalize_repo_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
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
    format!("{}...", truncated.trim_end())
}

fn limit_text(value: impl Into<String>, max_length: usize) -> String {
    trim_text(&value.into(), max_length)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
