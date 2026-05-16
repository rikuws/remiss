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

fn collect_review_partner_context(
    detail: &PullRequestDetail,
    stack: &ReviewStack,
    checkout_root: &Path,
    semantic_review: Option<&RemissSemanticReviewSummary>,
    lsp_session_manager: Option<&LspSessionManager>,
) -> ReviewPartnerContextPack {
    if !checkout_root.exists() {
        return ReviewPartnerContextPack {
            version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
            layers: Vec::new(),
            warnings: vec![format!(
                "Review Partner context could not inspect '{}'.",
                checkout_root.display()
            )],
        };
    }

    let atoms_by_id = stack
        .atoms
        .iter()
        .map(|atom| (atom.id.clone(), atom))
        .collect::<BTreeMap<_, _>>();
    let mut warnings = Vec::new();
    if let Some(semantic_review) = semantic_review {
        warnings.extend(
            semantic_review
                .warnings
                .iter()
                .take(MAX_SECTION_ITEMS)
                .map(|warning| format!("Semantic evidence: {warning}")),
        );
    }
    let layers = stack
        .layers
        .iter()
        .take(MAX_PARTNER_LAYERS)
        .map(|layer| {
            let atoms = layer
                .atom_ids
                .iter()
                .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
                .take(MAX_LAYER_ATOMS)
                .collect::<Vec<_>>();
            collect_layer_context(
                detail,
                layer,
                &atoms,
                checkout_root,
                semantic_review,
                lsp_session_manager,
                &mut warnings,
            )
        })
        .collect::<Vec<_>>();

    ReviewPartnerContextPack {
        version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
        layers,
        warnings,
    }
}

fn collect_layer_context(
    detail: &PullRequestDetail,
    layer: &ReviewStackLayer,
    atoms: &[&ChangeAtom],
    checkout_root: &Path,
    semantic_review: Option<&RemissSemanticReviewSummary>,
    lsp_session_manager: Option<&LspSessionManager>,
    warnings: &mut Vec<String>,
) -> ReviewPartnerCollectedLayer {
    let semantic_layers = semantic_layers_for_layer(layer, semantic_review);
    let semantic_focus = semantic_focus_for_layer(layer, semantic_review);
    let changed_symbols =
        collect_changed_symbols(layer, atoms, checkout_root, lsp_session_manager, warnings);
    let removed_symbols = collect_removed_symbols(detail, atoms, checkout_root, warnings);
    let similar_locations = collect_similar_locations(
        &changed_symbols,
        checkout_root,
        MAX_SIMILAR_LOCATIONS_PER_LAYER,
    );
    let style_notes = collect_style_notes(atoms, checkout_root);

    let mut limitations = Vec::new();
    for symbol in changed_symbols.iter().chain(removed_symbols.iter()) {
        if symbol.reference_count > symbol.references.len() {
            limitations.push(format!(
                "{} has {} matching locations; showing {} representative locations.",
                symbol.symbol,
                symbol.reference_count,
                symbol.references.len()
            ));
        } else if symbol.search_strategy.contains("tree-sitter") {
            limitations.push(format!(
                "{} occurrences came from a bounded tree-sitter syntax scan.",
                symbol.symbol
            ));
        } else if symbol.search_strategy.contains("rg") {
            limitations.push(format!(
                "{} references came from a bounded text search.",
                symbol.symbol
            ));
        }
    }
    limitations.sort();
    limitations.dedup();

    ReviewPartnerCollectedLayer {
        layer_id: layer.id.clone(),
        semantic_layers,
        semantic_focus,
        changed_symbols,
        removed_symbols,
        similar_locations,
        style_notes,
        limitations: limitations.into_iter().take(MAX_SECTION_ITEMS).collect(),
    }
}

fn semantic_layers_for_layer(
    layer: &ReviewStackLayer,
    semantic_review: Option<&RemissSemanticReviewSummary>,
) -> Vec<ReviewPartnerSemanticLayer> {
    let Some(semantic_review) = semantic_review else {
        return Vec::new();
    };
    let layer_atom_ids = layer.atom_ids.iter().cloned().collect::<BTreeSet<_>>();
    semantic_review
        .layers
        .iter()
        .filter(|semantic_layer| {
            semantic_layer
                .atom_ids
                .iter()
                .any(|atom_id| layer_atom_ids.contains(atom_id))
        })
        .take(MAX_SECTION_ITEMS)
        .map(review_partner_semantic_layer)
        .collect()
}

fn semantic_focus_for_layer(
    layer: &ReviewStackLayer,
    semantic_review: Option<&RemissSemanticReviewSummary>,
) -> Vec<RemissSemanticFocusSummary> {
    let Some(semantic_review) = semantic_review else {
        return Vec::new();
    };
    let layer_atom_ids = layer.atom_ids.iter().cloned().collect::<BTreeSet<_>>();
    semantic_review
        .focus_summaries
        .iter()
        .filter(|focus| layer_atom_ids.contains(&focus.atom_id))
        .take(MAX_SECTION_ITEMS)
        .cloned()
        .collect()
}

fn review_partner_semantic_layer(layer: &RemissSemanticLayerSummary) -> ReviewPartnerSemanticLayer {
    ReviewPartnerSemanticLayer {
        id: layer.id.clone(),
        title: limit_text(layer.title.clone(), MAX_ITEM_TEXT_CHARS),
        summary: limit_text(layer.summary.clone(), MAX_ITEM_TEXT_CHARS),
        rationale: limit_text(layer.rationale.clone(), MAX_ITEM_TEXT_CHARS),
        atom_ids: layer
            .atom_ids
            .iter()
            .take(MAX_LAYER_ATOMS)
            .cloned()
            .collect(),
        file_paths: layer
            .file_paths
            .iter()
            .take(MAX_SECTION_ITEMS)
            .cloned()
            .collect(),
        hunk_indices: layer.hunk_indices.clone(),
        entity_names: layer
            .entity_names
            .iter()
            .take(MAX_SECTION_ITEMS)
            .cloned()
            .collect(),
        change_count: layer.change_count,
    }
}

fn collect_changed_symbols(
    layer: &ReviewStackLayer,
    atoms: &[&ChangeAtom],
    checkout_root: &Path,
    lsp_session_manager: Option<&LspSessionManager>,
    warnings: &mut Vec<String>,
) -> Vec<ReviewPartnerCollectedSymbol> {
    let mut seen = BTreeSet::<String>::new();
    let mut symbols = Vec::new();

    for atom in atoms {
        let mut candidates = atom.defined_symbols.clone();
        if let Some(symbol) = &atom.symbol_name {
            candidates.push(symbol.clone());
        }

        for symbol in candidates {
            let symbol = clean_symbol(&symbol);
            if !is_searchable_symbol(&symbol) || !seen.insert(symbol.clone()) {
                continue;
            }

            let tree_sitter_references =
                search_tree_sitter_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS);
            let (locations, reference_count, strategy) = match tree_sitter_references {
                Some(result) if !result.locations.is_empty() => {
                    (result.locations, result.reference_count, result.strategy)
                }
                _ => match references_for_symbol(
                    checkout_root,
                    lsp_session_manager,
                    atom,
                    &symbol,
                    MAX_REFERENCES_PER_SYMBOL,
                ) {
                    Ok(result) if !result.locations.is_empty() => (
                        result.locations,
                        result.reference_count,
                        result.strategy.to_string(),
                    ),
                    _ => {
                        let result =
                            search_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS);
                        if let Some(error) = result.warning {
                            warnings.push(format!("{}: {error}", layer.title));
                        }
                        (result.locations, result.reference_count, result.strategy)
                    }
                },
            };

            symbols.push(ReviewPartnerCollectedSymbol {
                symbol,
                path: atom.path.clone(),
                line: atom.new_range.and_then(line_from_range),
                atom_ids: vec![atom.id.clone()],
                search_strategy: strategy,
                reference_count,
                references: locations
                    .into_iter()
                    .take(MAX_REFERENCES_PER_SYMBOL)
                    .collect(),
            });

            if symbols.len() >= MAX_CONTEXT_SYMBOLS_PER_LAYER {
                return symbols;
            }
        }
    }

    symbols
}

fn collect_removed_symbols(
    detail: &PullRequestDetail,
    atoms: &[&ChangeAtom],
    checkout_root: &Path,
    warnings: &mut Vec<String>,
) -> Vec<ReviewPartnerCollectedSymbol> {
    let mut removed = Vec::new();
    let mut seen = BTreeSet::<String>::new();

    for atom in atoms {
        let Some(parsed) = find_parsed_diff_file(&detail.parsed_diff, &atom.path) else {
            continue;
        };
        for (symbol, line) in removed_declarations_for_atom(parsed, atom) {
            let key = format!("{}:{}:{}", atom.path, symbol, line.unwrap_or_default());
            if !seen.insert(key) {
                continue;
            }
            let result =
                search_tree_sitter_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS)
                    .unwrap_or_else(|| {
                        search_symbol_locations(checkout_root, &symbol, MAX_RG_LOCATIONS)
                    });
            if let Some(error) = result.warning {
                warnings.push(format!("{}: {error}", atom.path));
            }
            removed.push(ReviewPartnerCollectedSymbol {
                symbol,
                path: atom.path.clone(),
                line,
                atom_ids: vec![atom.id.clone()],
                search_strategy: result.strategy,
                reference_count: result.reference_count,
                references: result
                    .locations
                    .into_iter()
                    .take(MAX_REFERENCES_PER_SYMBOL)
                    .collect(),
            });
            if removed.len() >= MAX_CONTEXT_SYMBOLS_PER_LAYER {
                return removed;
            }
        }
    }

    removed
}

fn removed_declarations_for_atom(
    parsed: &ParsedDiffFile,
    atom: &ChangeAtom,
) -> Vec<(String, Option<usize>)> {
    let mut removed = Vec::new();
    let mut hunk_indices = atom.hunk_indices.iter().copied().collect::<BTreeSet<_>>();
    if hunk_indices.is_empty() {
        hunk_indices.extend(0..parsed.hunks.len());
    }

    for (index, hunk) in parsed.hunks.iter().enumerate() {
        if !hunk_indices.contains(&index) {
            continue;
        }
        for line in &hunk.lines {
            if line.kind != DiffLineKind::Deletion {
                continue;
            }
            if let Some(symbol) = declaration_symbol(&line.content) {
                removed.push((
                    symbol,
                    line.left_line_number
                        .and_then(|line| usize::try_from(line).ok()),
                ));
            }
        }
    }

    removed
}

struct SymbolReferenceResult {
    locations: Vec<ReviewPartnerLocation>,
    reference_count: usize,
    strategy: &'static str,
}

fn references_for_symbol(
    checkout_root: &Path,
    lsp_session_manager: Option<&LspSessionManager>,
    atom: &ChangeAtom,
    symbol: &str,
    limit: usize,
) -> Result<SymbolReferenceResult, String> {
    let Some(lsp_session_manager) = lsp_session_manager else {
        return Err("LSP unavailable".to_string());
    };
    let document_path = checkout_root.join(&atom.path);
    let document = fs::read_to_string(&document_path)
        .map_err(|error| format!("Failed to read {}: {error}", atom.path))?;
    let Some((line, column)) = symbol_position_in_document(&document, atom.new_range, symbol)
    else {
        return Err(format!("Could not locate {symbol} in {}", atom.path));
    };
    let document_text: Arc<str> = Arc::from(document.as_str());
    let request = LspTextDocumentRequest {
        file_path: atom.path.clone(),
        document_text,
        line,
        column,
    };
    let details = lsp_session_manager.symbol_details(checkout_root, &request)?;
    let reference_count = details.reference_targets.len();
    let locations = details
        .reference_targets
        .into_iter()
        .take(limit)
        .map(|target| {
            let snippet = read_checkout_line(checkout_root, &target.path, target.line);
            ReviewPartnerLocation {
                path: target.path,
                line: target.line,
                snippet,
            }
        })
        .collect::<Vec<_>>();

    Ok(SymbolReferenceResult {
        locations,
        reference_count,
        strategy: "lsp references",
    })
}

struct SearchResult {
    locations: Vec<ReviewPartnerLocation>,
    reference_count: usize,
    strategy: String,
    warning: Option<String>,
}

#[derive(Clone, Copy)]
enum SearchMode {
    Identifier,
    Text,
}

fn search_tree_sitter_symbol_locations(
    checkout_root: &Path,
    symbol: &str,
    limit: usize,
) -> Option<SearchResult> {
    if !is_searchable_symbol(symbol) {
        return None;
    }

    let mut parser = Parser::new();
    let language = tree_sitter_rust_orchard::LANGUAGE.into();
    parser.set_language(&language).ok()?;

    let mut queue = VecDeque::from([(checkout_root.to_path_buf(), 0usize)]);
    let mut scanned_files = 0usize;
    let mut locations = Vec::new();
    let mut seen = BTreeSet::<String>::new();
    let mut reference_count = 0usize;

    while let Some((path, depth)) = queue.pop_front() {
        if depth > MAX_SCAN_DEPTH || scanned_files >= MAX_SCAN_FILES {
            break;
        }
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                if should_skip_directory(file_name) {
                    continue;
                }
                queue.push_back((path, depth + 1));
                continue;
            }
            if scanned_files >= MAX_SCAN_FILES || !is_tree_sitter_rust_candidate(&path) {
                continue;
            }
            scanned_files += 1;
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_SCAN_FILE_BYTES {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Some(tree) = parser.parse(&text, None) else {
                continue;
            };
            let lines = text.lines().collect::<Vec<_>>();
            collect_tree_sitter_symbol_locations(
                tree.root_node(),
                &text,
                &lines,
                checkout_root,
                &path,
                symbol,
                limit,
                &mut locations,
                &mut seen,
                &mut reference_count,
            );
        }
    }

    Some(SearchResult {
        locations,
        reference_count,
        strategy: "tree-sitter rust identifier scan".to_string(),
        warning: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_tree_sitter_symbol_locations(
    node: Node<'_>,
    text: &str,
    lines: &[&str],
    checkout_root: &Path,
    path: &Path,
    symbol: &str,
    limit: usize,
    locations: &mut Vec<ReviewPartnerLocation>,
    seen: &mut BTreeSet<String>,
    reference_count: &mut usize,
) {
    if tree_sitter_node_matches_symbol(node, text, symbol) {
        let line = node.start_position().row + 1;
        let relative = relative_path(checkout_root, path);
        let key = format!("{relative}:{line}");
        if seen.insert(key) {
            *reference_count += 1;
            if locations.len() < limit {
                locations.push(ReviewPartnerLocation {
                    path: relative,
                    line,
                    snippet: lines
                        .get(line.saturating_sub(1))
                        .map(|line| trim_text(line, MAX_PROMPT_SNIPPET_CHARS)),
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tree_sitter_symbol_locations(
            child,
            text,
            lines,
            checkout_root,
            path,
            symbol,
            limit,
            locations,
            seen,
            reference_count,
        );
    }
}

fn tree_sitter_node_matches_symbol(node: Node<'_>, text: &str, symbol: &str) -> bool {
    let kind = node.kind();
    if kind != "identifier" && !kind.ends_with("_identifier") {
        return false;
    }
    node.utf8_text(text.as_bytes())
        .map(|value| value == symbol)
        .unwrap_or(false)
}

fn is_tree_sitter_rust_candidate(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension == "rs")
        .unwrap_or(false)
}

fn search_symbol_locations(checkout_root: &Path, symbol: &str, limit: usize) -> SearchResult {
    search_locations_in_scope(checkout_root, symbol, limit, None, SearchMode::Identifier)
}

fn search_similar_locations_in_scope(
    checkout_root: &Path,
    token: &str,
    limit: usize,
    relative_scope: Option<&Path>,
) -> SearchResult {
    search_locations_in_scope(
        checkout_root,
        token,
        limit,
        relative_scope,
        SearchMode::Text,
    )
}

fn search_locations_in_scope(
    checkout_root: &Path,
    symbol: &str,
    limit: usize,
    relative_scope: Option<&Path>,
    mode: SearchMode,
) -> SearchResult {
    match rg_symbol_locations(checkout_root, symbol, limit, relative_scope, mode) {
        Ok(result) => result,
        Err(error) => {
            let start = relative_scope
                .filter(|scope| !scope.as_os_str().is_empty())
                .map(|scope| checkout_root.join(scope))
                .unwrap_or_else(|| checkout_root.to_path_buf());
            let mut result = scan_symbol_locations_from(checkout_root, &start, symbol, limit, mode);
            result.warning = Some(format!("rg search unavailable, used bounded scan: {error}"));
            result
        }
    }
}

fn rg_symbol_locations(
    checkout_root: &Path,
    symbol: &str,
    limit: usize,
    relative_scope: Option<&Path>,
    mode: SearchMode,
) -> Result<SearchResult, String> {
    let search_path = relative_scope
        .filter(|scope| !scope.as_os_str().is_empty())
        .and_then(|scope| scope.to_str())
        .unwrap_or(".");
    let mut command = Command::new("rg");
    command
        .arg("--line-number")
        .arg("--fixed-strings")
        .arg("--color")
        .arg("never")
        .arg("--max-count")
        .arg("5")
        .arg("--glob")
        .arg("!.git");
    if matches!(mode, SearchMode::Text) {
        command.arg("--ignore-case");
    }
    let output = command
        .arg("--")
        .arg(symbol)
        .arg(search_path)
        .current_dir(checkout_root)
        .output()
        .map_err(|error| error.to_string())?;

    if !output.status.success() && output.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut locations = Vec::new();
    let mut reference_count = 0usize;
    for line in stdout.lines() {
        let Some(location) = parse_rg_line(line) else {
            continue;
        };
        if !location_matches_search(&location, symbol, mode) {
            continue;
        }
        reference_count += 1;
        if locations.len() < limit {
            locations.push(location);
        }
    }

    Ok(SearchResult {
        locations,
        reference_count,
        strategy: "rg exact text search".to_string(),
        warning: None,
    })
}

fn scan_symbol_locations(checkout_root: &Path, symbol: &str, limit: usize) -> SearchResult {
    scan_symbol_locations_from(
        checkout_root,
        checkout_root,
        symbol,
        limit,
        SearchMode::Identifier,
    )
}

fn scan_symbol_locations_from(
    checkout_root: &Path,
    start_path: &Path,
    symbol: &str,
    limit: usize,
    mode: SearchMode,
) -> SearchResult {
    let mut queue = VecDeque::from([(start_path.to_path_buf(), 0usize)]);
    let mut scanned_files = 0usize;
    let mut locations = Vec::new();
    let mut reference_count = 0usize;

    while let Some((path, depth)) = queue.pop_front() {
        if depth > MAX_SCAN_DEPTH || scanned_files >= MAX_SCAN_FILES {
            break;
        }
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                if should_skip_directory(file_name) {
                    continue;
                }
                queue.push_back((path, depth + 1));
                continue;
            }
            if scanned_files >= MAX_SCAN_FILES || !is_text_search_candidate(&path) {
                continue;
            }
            scanned_files += 1;
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_SCAN_FILE_BYTES {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if is_comment_only_line(line) || !line_matches_search(line, symbol, mode) {
                    continue;
                }
                reference_count += 1;
                if locations.len() < limit {
                    locations.push(ReviewPartnerLocation {
                        path: relative_path(checkout_root, &path),
                        line: index + 1,
                        snippet: Some(trim_text(line, MAX_PROMPT_SNIPPET_CHARS)),
                    });
                }
            }
        }
    }

    SearchResult {
        locations,
        reference_count,
        strategy: "bounded file scan".to_string(),
        warning: None,
    }
}

fn parse_rg_line(line: &str) -> Option<ReviewPartnerLocation> {
    let mut parts = line.splitn(3, ':');
    let path = parts.next()?.trim_start_matches("./").to_string();
    let line_number = parts.next()?.parse::<usize>().ok()?;
    let snippet = parts
        .next()
        .map(|value| trim_text(value, MAX_PROMPT_SNIPPET_CHARS));
    Some(ReviewPartnerLocation {
        path,
        line: line_number,
        snippet,
    })
}

fn location_matches_search(
    location: &ReviewPartnerLocation,
    symbol: &str,
    mode: SearchMode,
) -> bool {
    location
        .snippet
        .as_deref()
        .map(|snippet| !is_comment_only_line(snippet) && line_matches_search(snippet, symbol, mode))
        .unwrap_or(false)
}

fn is_comment_only_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with("--")
        || trimmed.starts_with("<!--")
        || trimmed.starts_with("# ")
}

fn line_matches_search(line: &str, symbol: &str, mode: SearchMode) -> bool {
    match mode {
        SearchMode::Identifier => contains_identifier(line, symbol),
        SearchMode::Text => line
            .to_ascii_lowercase()
            .contains(&symbol.to_ascii_lowercase()),
    }
}

fn collect_similar_locations(
    symbols: &[ReviewPartnerCollectedSymbol],
    checkout_root: &Path,
    limit: usize,
) -> Vec<ReviewPartnerLocation> {
    let mut locations = Vec::new();
    let mut seen = BTreeSet::<String>::new();
    for symbol in symbols {
        let Some(token) = similar_search_token(&symbol.symbol) else {
            continue;
        };

        let module_scope = Path::new(&symbol.path)
            .parent()
            .filter(|scope| !scope.as_os_str().is_empty());
        let mut scoped_results = Vec::new();
        if let Some(scope) = module_scope {
            scoped_results.push(search_similar_locations_in_scope(
                checkout_root,
                &token,
                limit,
                Some(scope),
            ));
        }
        scoped_results.push(search_similar_locations_in_scope(
            checkout_root,
            &token,
            limit,
            None,
        ));

        for result in scoped_results {
            for location in result.locations {
                if location.path == symbol.path && Some(location.line) == symbol.line {
                    continue;
                }
                let key = format!("{}:{}", location.path, location.line);
                if seen.insert(key) {
                    locations.push(location);
                    if locations.len() >= limit {
                        return locations;
                    }
                }
            }
        }
    }
    locations
}

fn collect_style_notes(atoms: &[&ChangeAtom], checkout_root: &Path) -> Vec<ReviewPartnerItem> {
    let mut notes = Vec::new();
    let mut seen = BTreeSet::<String>::new();
    for atom in atoms {
        if let Some(note) = nearby_style_note(atom, checkout_root) {
            if seen.insert(note.title.clone()) {
                notes.push(note);
                if notes.len() >= MAX_STYLE_NOTES_PER_LAYER {
                    break;
                }
            }
        }
    }
    notes
}

fn nearby_style_note(atom: &ChangeAtom, checkout_root: &Path) -> Option<ReviewPartnerItem> {
    let path = Path::new(&atom.path);
    let directory = path.parent()?;
    let checkout_directory = checkout_root.join(directory);
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let entries = fs::read_dir(&checkout_directory).ok()?;
    let mut siblings = entries
        .flatten()
        .filter_map(|entry| {
            let candidate = entry.path();
            if candidate.is_dir() || candidate == checkout_root.join(&atom.path) {
                return None;
            }
            let same_extension = candidate
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext == extension)
                .unwrap_or(false);
            same_extension.then(|| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string()
            })
        })
        .take(5)
        .collect::<Vec<_>>();
    siblings.sort();
    if siblings.is_empty() {
        return None;
    }
    Some(ReviewPartnerItem::new(
        format!("Nearby {}", directory.display()),
        format!(
            "Sibling files for style comparison: {}.",
            siblings.join(", ")
        ),
        Some(atom.path.clone()),
        atom.new_range.and_then(line_from_range),
    ))
}

fn items_from_changed_symbols(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .changed_symbols
        .iter()
        .map(|symbol| {
            ReviewPartnerItem::new(
                symbol.symbol.clone(),
                format!(
                    "Changed in {}{}; {} reference{} surfaced via {}.",
                    symbol.path,
                    symbol
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default(),
                    symbol.reference_count,
                    if symbol.reference_count == 1 { "" } else { "s" },
                    symbol.search_strategy,
                ),
                Some(symbol.path.clone()),
                symbol.line,
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn items_from_semantic_layers(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .semantic_layers
        .iter()
        .map(|layer| {
            ReviewPartnerItem::new(
                layer.title.clone(),
                default_if_empty(layer.summary.clone(), &layer.rationale),
                layer.file_paths.first().cloned(),
                None,
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn items_from_semantic_focus(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .semantic_focus
        .iter()
        .filter_map(|focus| {
            let entity = focus
                .target_entity
                .as_ref()
                .or_else(|| focus.overlapping_entities.first())?;
            let impact_detail = focus.impact.as_ref().map(|impact| {
                format!(
                    " Sem found {} dependenc{}, {} dependent{}, and {} test target{}.",
                    impact.dependencies.len(),
                    if impact.dependencies.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    },
                    impact.dependents.len(),
                    if impact.dependents.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    impact.tests.len(),
                    if impact.tests.len() == 1 { "" } else { "s" }
                )
            });
            Some(ReviewPartnerItem::new(
                entity.name.clone(),
                format!(
                    "Sem resolved this layer through the {} `{}` in {}:{}-{}.{}",
                    entity.entity_type,
                    entity.name,
                    entity.file_path,
                    entity.start_line,
                    entity.end_line,
                    impact_detail.unwrap_or_default()
                ),
                Some(entity.file_path.clone()),
                Some(entity.start_line),
            ))
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn items_from_removed_symbols(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .removed_symbols
        .iter()
        .map(|symbol| {
            let detail = if symbol.reference_count == 0 {
                format!(
                    "Removed from {}{}; no remaining references surfaced in the bounded scan.",
                    symbol.path,
                    symbol
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default()
                )
            } else {
                format!(
                    "Removed from {}{}; {} remaining match{} surfaced.",
                    symbol.path,
                    symbol
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default(),
                    symbol.reference_count,
                    if symbol.reference_count == 1 {
                        ""
                    } else {
                        "es"
                    },
                )
            };
            ReviewPartnerItem::new(
                symbol.symbol.clone(),
                detail,
                Some(symbol.path.clone()),
                symbol.line,
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn items_from_usages(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .changed_symbols
        .iter()
        .chain(context.removed_symbols.iter())
        .flat_map(|symbol| {
            symbol.references.iter().map(move |location| {
                ReviewPartnerItem::new(
                    symbol.symbol.clone(),
                    location
                        .snippet
                        .clone()
                        .unwrap_or_else(|| format!("Reference in {}", location.path)),
                    Some(location.path.clone()),
                    Some(location.line),
                )
            })
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn items_from_similar_locations(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .similar_locations
        .iter()
        .map(|location| {
            ReviewPartnerItem::new(
                format!("{}:{}", location.path, location.line),
                location
                    .snippet
                    .clone()
                    .unwrap_or_else(|| "Similar symbol context.".to_string()),
                Some(location.path.clone()),
                Some(location.line),
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
}

fn items_from_style_notes(context: &ReviewPartnerCollectedLayer) -> Vec<ReviewPartnerItem> {
    context
        .style_notes
        .iter()
        .take(MAX_SECTION_ITEMS)
        .cloned()
        .collect()
}

fn items_from_layer_atoms(layer: &ReviewStackLayer, stack: &ReviewStack) -> Vec<ReviewPartnerItem> {
    stack
        .atoms_for_layer(layer)
        .into_iter()
        .map(|atom| {
            ReviewPartnerItem::new(
                atom.symbol_name
                    .clone()
                    .unwrap_or_else(|| atom.path.clone()),
                format!(
                    "{} changed line{} in {}.",
                    atom.additions + atom.deletions,
                    if atom.additions + atom.deletions == 1 {
                        ""
                    } else {
                        "s"
                    },
                    atom.path
                ),
                Some(atom.path.clone()),
                atom.new_range.and_then(line_from_range),
            )
        })
        .take(MAX_SECTION_ITEMS)
        .collect()
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
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, path::PathBuf};

    use crate::diff::{ParsedDiffHunk, ParsedDiffLine};
    use crate::semantic_review::{build_semantic_review_from_contents, RemissSemFileContents};
    use crate::stacks::model::{
        stack_now_ms, ChangeAtomSource, ChangeRole, Confidence, LayerMetrics, LayerReviewStatus,
        StackKind, StackSource,
    };
    use crate::structural_evidence::{StructuralEvidenceChange, StructuralEvidenceFile};

    #[test]
    fn partner_cache_key_includes_versions() {
        let key = review_partner_cache_key_from_parts(
            "acme/widgets",
            42,
            CodeTourProvider::Codex,
            "head-a",
            "stack-x",
            "context-y",
        );

        assert!(key.starts_with("review-partner-v15:"));
        assert!(key.contains("stack-x"));
        assert!(key.contains("context-y"));
    }

    #[test]
    fn review_partner_prompt_requires_concrete_summary_copy() {
        let prompt = build_review_partner_prompt(&input(ReviewPartnerContextPack::empty()));

        assert!(prompt.contains("The goal is explaining the scoped code"));
        assert!(prompt.contains("factual code explanation"));
        assert!(prompt.contains("never as a question"));
        assert!(prompt.contains("Never end a summary with an ellipsis"));
        assert!(prompt.contains("Match the supplied focus scope exactly"));
        assert!(!prompt.contains("Act like a strong reviewer"));
        assert!(!prompt.contains("Do not"));
    }

    #[test]
    fn review_partner_prompt_and_focus_records_include_semantic_context() {
        let stack = stack();
        let semantic_review = semantic_review_for_stack(&stack);
        let input = build_review_partner_generation_input(
            &detail_with_deleted_symbol(),
            CodeTourProvider::Codex,
            "/tmp",
            stack,
            StructuralEvidencePack::empty(),
            Some(semantic_review),
            None,
        );

        assert!(input.semantic_review.is_some());
        assert!(input
            .context
            .layer("layer-1")
            .map(|layer| !layer.semantic_layers.is_empty())
            .unwrap_or(false));
        assert!(input
            .context
            .layer("layer-1")
            .map(|layer| !layer.semantic_focus.is_empty())
            .unwrap_or(false));

        let prompt = build_review_partner_prompt(&input);
        assert!(prompt.contains("semanticEvidence"));
        assert!(prompt.contains("semanticLayers"));
        assert!(prompt.contains("semanticFocus"));

        let partner = fallback_review_partner_context(&input, Some("Codex timed out".to_string()));
        assert_eq!(partner.fallback_reason.as_deref(), Some("Codex timed out"));
        assert!(partner.semantic_review.is_some());
        assert!(partner
            .context
            .layer("layer-1")
            .map(|layer| !layer.semantic_layers.is_empty())
            .unwrap_or(false));
        assert!(partner
            .context
            .layer("layer-1")
            .map(|layer| !layer.semantic_focus.is_empty())
            .unwrap_or(false));
        assert!(partner
            .layer("layer-1")
            .map(|layer| {
                layer.brief.contains("Sem") && !layer.brief.contains("semantic changes across")
            })
            .unwrap_or(false));

        let focus_prompt = build_focus_record_prompt(&partner, &partner.focus_targets[0]);
        assert!(focus_prompt.contains("semanticEvidence"));
    }

    #[test]
    fn partner_request_key_includes_generator_and_context_versions() {
        let detail = detail_with_deleted_symbol();
        let key = build_review_partner_request_key(&detail, CodeTourProvider::Codex);

        assert!(key.contains(CodeTourProvider::Codex.slug()));
        assert!(key.contains(&detail.repository));
        assert!(key.contains(detail.head_ref_oid.as_deref().unwrap_or_default()));
        assert!(key.contains(REVIEW_PARTNER_GENERATOR_VERSION));
        assert!(key.contains(STACK_GENERATOR_VERSION));
        assert!(key.contains(REVIEW_PARTNER_CONTEXT_VERSION));
    }

    #[test]
    fn review_partner_schema_requires_focus_records() {
        let schema = review_partner_output_schema();

        assert!(schema["properties"].get("focusRecords").is_some());
        assert!(schema["required"]
            .as_array()
            .expect("required array")
            .iter()
            .any(|value| value.as_str() == Some("focusRecords")));
        assert!(schema["properties"].get("limitations").is_none());
        let focus_record = &schema["properties"]["focusRecords"]["items"];
        assert!(focus_record["properties"].get("usageContext").is_none());
        assert!(focus_record["properties"].get("codebaseFit").is_some());
        assert!(focus_record["properties"].get("limitations").is_none());
        let single_focus_schema = focus_record_output_schema();
        let single_focus_record = &single_focus_schema["properties"]["record"];
        assert!(single_focus_record["properties"]
            .get("usageContext")
            .is_none());
        assert!(single_focus_record["properties"]
            .get("codebaseFit")
            .is_some());
        assert!(single_focus_record["properties"]
            .get("limitations")
            .is_none());
    }

    #[test]
    fn generation_input_caps_upfront_focus_records() {
        let mut stack = stack();
        stack.layers = (0..MAX_FOCUS_RECORDS + 7)
            .map(|index| {
                let atom_id = format!("atom-{index}");
                ReviewStackLayer {
                    id: format!("layer-{index}"),
                    index,
                    title: format!("Layer {index}"),
                    summary: "Layer summary".to_string(),
                    rationale: "Layer rationale".to_string(),
                    pr: None,
                    virtual_layer: None,
                    base_oid: None,
                    head_oid: None,
                    atom_ids: vec![atom_id],
                    depends_on_layer_ids: Vec::new(),
                    metrics: LayerMetrics::default(),
                    status: LayerReviewStatus::NotReviewed,
                    confidence: Confidence::Medium,
                    warnings: Vec::new(),
                }
            })
            .collect();
        stack.atoms = (0..MAX_FOCUS_RECORDS + 7)
            .map(|index| {
                atom(
                    &format!("atom-{index}"),
                    "src/lib.rs",
                    index as i64 + 1,
                    index as i64 + 1,
                )
            })
            .collect();

        let input = build_review_partner_generation_input(
            &detail_with_deleted_symbol(),
            CodeTourProvider::Codex,
            "/tmp/remiss-review-partner-missing-checkout",
            stack,
            StructuralEvidencePack::empty(),
            None,
            None,
        );

        assert_eq!(input.focus_targets.len(), MAX_FOCUS_RECORDS);
        assert_eq!(input.focus_targets[0].key, "layer:layer-0");
        assert_eq!(
            input.focus_targets[0].match_kind,
            ReviewPartnerFocusMatchKind::Layer
        );
    }

    #[test]
    fn focus_target_matches_atom_range() {
        let document = partner_document(stack(), StructuralEvidencePack::empty());

        let target =
            focus_target_for_diff_focus(&document, "src/lib.rs", Some(1), Some("RIGHT"), None);

        assert_eq!(target.key, "atom:atom-1");
        assert_eq!(target.match_kind, ReviewPartnerFocusMatchKind::AtomRange);
        assert_eq!(target.line, Some(1));
    }

    #[test]
    fn focus_target_matches_atom_hunk_without_line() {
        let mut stack = stack();
        stack.atoms[0].hunk_headers = vec!["@@ -10,2 +10,2 @@ fn render".to_string()];
        let document = partner_document(stack, StructuralEvidencePack::empty());

        let target = focus_target_for_diff_focus(
            &document,
            "src/lib.rs",
            None,
            Some("RIGHT"),
            Some("@@ -10,2 +10,2 @@ fn render"),
        );

        assert_eq!(target.key, "atom:atom-1");
        assert_eq!(target.match_kind, ReviewPartnerFocusMatchKind::AtomHunk);
    }

    #[test]
    fn focus_target_prefers_tightest_atom_range() {
        let mut stack = stack();
        let mut broad = atom("broad", "src/lib.rs", 10, 30);
        broad.symbol_name = Some("broad_change".to_string());
        broad.additions = 20;
        let mut tight = atom("tight", "src/lib.rs", 14, 15);
        tight.symbol_name = Some("tight_change".to_string());
        tight.additions = 2;
        stack.atoms = vec![broad, tight];
        stack.layers[0].atom_ids = vec!["broad".to_string(), "tight".to_string()];
        let document = partner_document(stack, StructuralEvidencePack::empty());

        let target =
            focus_target_for_diff_focus(&document, "src/lib.rs", Some(14), Some("RIGHT"), None);

        assert_eq!(target.key, "atom:tight");
        assert_eq!(target.title, "tight_change");
    }

    #[test]
    fn focus_target_falls_back_to_structural_hunk() {
        let evidence = structural_evidence_with_hunk(
            "src/lib.rs",
            "@@ -40,2 +40,2 @@ fn focused",
            3,
            40,
            vec!["atom-1".to_string()],
        );
        let document = partner_document(stack(), evidence);

        let target =
            focus_target_for_diff_focus(&document, "src/lib.rs", Some(40), Some("RIGHT"), None);

        assert_eq!(target.match_kind, ReviewPartnerFocusMatchKind::Hunk);
        assert_eq!(target.hunk_index, Some(3));
        assert_eq!(target.atom_ids, vec!["atom-1".to_string()]);
    }

    #[test]
    fn focus_target_falls_back_to_file_context() {
        let document = partner_document(stack(), StructuralEvidencePack::empty());

        let target =
            focus_target_for_diff_focus(&document, "src/other.rs", Some(22), Some("RIGHT"), None);

        assert!(target.key.starts_with("file:"));
        assert_eq!(target.match_kind, ReviewPartnerFocusMatchKind::File);
        assert_eq!(target.file_path, "src/other.rs");
        assert_eq!(target.line, Some(22));
    }

    #[test]
    fn upsert_focus_record_adds_overflow_target() {
        let mut document = partner_document(stack(), StructuralEvidencePack::empty());
        document.focus_targets.clear();
        document.focus_records.clear();
        let target = focus_target_from_file(
            "src/overflow.rs".to_string(),
            Some(9),
            Some("RIGHT".to_string()),
            None,
        );
        let record = ReviewPartnerFocusRecord {
            key: target.key.clone(),
            title: target.title.clone(),
            subtitle: target.subtitle.clone(),
            target: target.clone(),
            summary: "Generated after the focus key moved beyond the upfront cap.".to_string(),
            usage_context: Vec::new(),
            codebase_fit: ReviewPartnerCodebaseFit::default(),
            sections: vec![ReviewPartnerFocusSection {
                title: "Concerns".to_string(),
                items: vec![ReviewPartnerItem::new(
                    "overflow".to_string(),
                    "Generated after the focus key moved beyond the upfront cap.".to_string(),
                    Some("src/overflow.rs".to_string()),
                    Some(9),
                )],
            }],
            limitations: Vec::new(),
            generated_at_ms: 1,
        };

        upsert_focus_record(&mut document, target.clone(), record);

        assert!(document.focus_target(&target.key).is_some());
        assert!(document.focus_record(&target.key).is_some());
    }

    #[test]
    fn merge_rejects_unknown_layer_ids() {
        let input = input(ReviewPartnerContextPack::empty());
        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: vec![ReviewPartnerLayerResponse {
                layer_id: "invented".to_string(),
                brief: "changed".to_string(),
                changed_items: Vec::new(),
                removed_items: Vec::new(),
                usage_context: Vec::new(),
                similar_code: Vec::new(),
                codebase_fit: Vec::new(),
                concerns: Vec::new(),
                limitations: Vec::new(),
            }],
            focus_records: Vec::new(),
        };

        let error =
            merge_review_partner(response, &input, None).expect_err("unknown layer rejected");
        assert!(error.contains("unknown layer id"));
    }

    #[test]
    fn merge_preserves_stack_order_and_clips_items() {
        let input = input(ReviewPartnerContextPack::empty());
        let many_items = (0..12)
            .map(|index| ReviewPartnerItemResponse {
                title: format!("item-{index}"),
                detail: "detail".to_string(),
                path: None,
                line: None,
            })
            .collect::<Vec<_>>();
        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: vec![ReviewPartnerLayerResponse {
                layer_id: "layer-1".to_string(),
                brief: "partner brief".to_string(),
                changed_items: many_items,
                removed_items: Vec::new(),
                usage_context: Vec::new(),
                similar_code: Vec::new(),
                codebase_fit: Vec::new(),
                concerns: Vec::new(),
                limitations: Vec::new(),
            }],
            focus_records: vec![ReviewPartnerFocusRecordResponse {
                key: "layer:layer-1".to_string(),
                title: "Focus record".to_string(),
                subtitle: None,
                summary: Some("Review the focused usage contract.".to_string()),
                usage_context: vec![ReviewPartnerUsageGroupResponse {
                    symbol: "usage".to_string(),
                    summary: "One usage surfaced.".to_string(),
                    usages: vec![ReviewPartnerItemResponse {
                        title: "usage".to_string(),
                        detail: "detail".to_string(),
                        path: None,
                        line: None,
                    }],
                }],
                codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                    follows: true,
                    summary: "follows codebase style".to_string(),
                    evidence: Vec::new(),
                }),
                sections: vec![ReviewPartnerFocusSectionResponse {
                    title: "Concerns".to_string(),
                    items: vec![ReviewPartnerItemResponse {
                        title: "risk".to_string(),
                        detail: "detail".to_string(),
                        path: None,
                        line: None,
                    }],
                }],
                limitations: Vec::new(),
            }],
        };

        let partner = merge_review_partner(response, &input, Some("model".to_string()))
            .expect("partner context");

        assert_eq!(partner.layers[0].layer_id, "layer-1");
        assert_eq!(partner.layers[0].brief, "partner brief");
        assert_eq!(partner.layers[0].changed_items.len(), MAX_SECTION_ITEMS);
        assert_eq!(partner.focus_records.len(), 1);
        assert_eq!(partner.model.as_deref(), Some("model"));
    }

    #[test]
    fn merge_uses_collected_tree_sitter_usages_instead_of_llm_usages() {
        let input = input(ReviewPartnerContextPack {
            version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
            layers: vec![ReviewPartnerCollectedLayer {
                layer_id: "layer-1".to_string(),
                semantic_layers: Vec::new(),
                semantic_focus: Vec::new(),
                changed_symbols: vec![ReviewPartnerCollectedSymbol {
                    symbol: "render_review".to_string(),
                    path: "src/lib.rs".to_string(),
                    line: Some(1),
                    atom_ids: vec!["atom-1".to_string()],
                    search_strategy: "tree-sitter rust identifier scan".to_string(),
                    reference_count: 2,
                    references: vec![
                        ReviewPartnerLocation {
                            path: "src/lib.rs".to_string(),
                            line: 4,
                            snippet: Some("fn caller_one() { render_review(); }".to_string()),
                        },
                        ReviewPartnerLocation {
                            path: "src/lib.rs".to_string(),
                            line: 9,
                            snippet: Some("fn caller_two() { render_review(); }".to_string()),
                        },
                    ],
                }],
                removed_symbols: Vec::new(),
                similar_locations: Vec::new(),
                style_notes: Vec::new(),
                limitations: Vec::new(),
            }],
            warnings: Vec::new(),
        });
        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: Vec::new(),
            focus_records: vec![ReviewPartnerFocusRecordResponse {
                key: "layer:layer-1".to_string(),
                title: "Focus record".to_string(),
                subtitle: None,
                summary: Some("Grouped usage summary.".to_string()),
                usage_context: vec![ReviewPartnerUsageGroupResponse {
                    symbol: "llm_usage".to_string(),
                    summary: "LLM usage should be ignored.".to_string(),
                    usages: vec![ReviewPartnerItemResponse {
                        title: "llm_usage".to_string(),
                        detail: "call".to_string(),
                        path: Some("src/other.rs".to_string()),
                        line: Some(20),
                    }],
                }],
                codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                    follows: true,
                    summary: "follows codebase style".to_string(),
                    evidence: Vec::new(),
                }),
                sections: Vec::new(),
                limitations: Vec::new(),
            }],
        };

        let partner = merge_review_partner(response, &input, None).expect("partner context");
        let record = &partner.focus_records[0];

        assert_eq!(record.summary, "Grouped usage summary.");
        assert_eq!(record.usage_context.len(), 1);
        assert_eq!(record.usage_context[0].symbol, "render_review");
        assert_eq!(record.usage_context[0].usages.len(), 2);
        assert!(record.usage_context[0].summary.contains("tree-sitter"));
        assert!(record.codebase_fit.follows);
    }

    #[test]
    fn focus_summary_preserves_complete_explanation_above_item_limit() {
        let input = input(ReviewPartnerContextPack::empty());
        let summary = "This focused change routes normalized diff text through the public helper so callers get CLI-equivalent behavior without repeating path setup and without forcing each consumer to mirror the command-line normalization rules. ".repeat(6);
        let expected_summary = summary.trim().to_string();
        assert!(expected_summary.len() > MAX_ITEM_TEXT_CHARS);
        assert!(expected_summary.len() > 520);

        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: Vec::new(),
            focus_records: vec![ReviewPartnerFocusRecordResponse {
                key: "layer:layer-1".to_string(),
                title: "Focus record".to_string(),
                subtitle: None,
                summary: Some(summary),
                usage_context: Vec::new(),
                codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                    follows: true,
                    summary: "follows codebase style".to_string(),
                    evidence: Vec::new(),
                }),
                sections: Vec::new(),
                limitations: Vec::new(),
            }],
        };

        let partner = merge_review_partner(response, &input, None).expect("partner context");

        assert_eq!(partner.focus_records[0].summary, expected_summary);
        assert!(!partner.focus_records[0].summary.ends_with("..."));
    }

    #[test]
    fn question_led_focus_summary_keeps_concrete_remainder() {
        let input = input(ReviewPartnerContextPack::empty());
        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: Vec::new(),
            focus_records: vec![ReviewPartnerFocusRecordResponse {
                key: "layer:layer-1".to_string(),
                title: "Focus record".to_string(),
                subtitle: None,
                summary: Some(
                    "Does the public API now match CLI behavior? Normalizes text before diffing."
                        .to_string(),
                ),
                usage_context: Vec::new(),
                codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                    follows: true,
                    summary: "follows codebase style".to_string(),
                    evidence: Vec::new(),
                }),
                sections: Vec::new(),
                limitations: Vec::new(),
            }],
        };

        let partner = merge_review_partner(response, &input, None).expect("partner context");

        assert_eq!(
            partner.focus_records[0].summary,
            "Normalizes text before diffing."
        );
    }

    #[test]
    fn do_led_focus_summary_keeps_concrete_remainder() {
        let input = input(ReviewPartnerContextPack::empty());
        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: Vec::new(),
            focus_records: vec![ReviewPartnerFocusRecordResponse {
                key: "layer:layer-1".to_string(),
                title: "Focus record".to_string(),
                subtitle: None,
                summary: Some(
                    "Do the helpers define the platform contract? Adds portable asset lookup."
                        .to_string(),
                ),
                usage_context: Vec::new(),
                codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                    follows: true,
                    summary: "follows codebase style".to_string(),
                    evidence: Vec::new(),
                }),
                sections: Vec::new(),
                limitations: Vec::new(),
            }],
        };

        let partner = merge_review_partner(response, &input, None).expect("partner context");

        assert_eq!(
            partner.focus_records[0].summary,
            "Adds portable asset lookup."
        );
    }

    #[test]
    fn fallback_focus_summary_rewrites_question_brief() {
        let target = ReviewPartnerFocusTarget {
            key: "atom:atom-1".to_string(),
            file_path: "README.md".to_string(),
            hunk_header: None,
            hunk_index: None,
            line: Some(11),
            side: Some("new".to_string()),
            atom_ids: vec!["atom-1".to_string()],
            layer_id: Some("layer-1".to_string()),
            title: "README.md".to_string(),
            subtitle: "Focused change".to_string(),
            match_kind: ReviewPartnerFocusMatchKind::AtomRange,
        };
        let layer = ReviewPartnerLayer {
            layer_id: "layer-1".to_string(),
            title: "Docs".to_string(),
            brief: "Does the README describe the Windows alpha path? Updates requirements and packaging guidance.".to_string(),
            changed_items: Vec::new(),
            removed_items: Vec::new(),
            usage_context: Vec::new(),
            similar_code: Vec::new(),
            codebase_fit: Vec::new(),
            concerns: Vec::new(),
            limitations: Vec::new(),
            structural_evidence_status: StructuralEvidenceStatus::Unavailable,
        };

        assert_eq!(
            fallback_focus_summary(&target, Some(&layer)),
            "Updates requirements and packaging guidance."
        );
    }

    #[test]
    fn ungrounded_codebase_fit_mismatch_becomes_follows() {
        let input = input(ReviewPartnerContextPack::empty());
        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: Vec::new(),
            focus_records: vec![ReviewPartnerFocusRecordResponse {
                key: "layer:layer-1".to_string(),
                title: "Focus record".to_string(),
                subtitle: None,
                summary: Some("Style verdict summary.".to_string()),
                usage_context: Vec::new(),
                codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                    follows: false,
                    summary: "This does not match local style.".to_string(),
                    evidence: vec![ReviewPartnerItemResponse {
                        title: "unsupported".to_string(),
                        detail: "No linked evidence.".to_string(),
                        path: None,
                        line: None,
                    }],
                }),
                sections: Vec::new(),
                limitations: Vec::new(),
            }],
        };

        let partner = merge_review_partner(response, &input, None).expect("partner context");
        let fit = &partner.focus_records[0].codebase_fit;

        assert!(fit.follows);
        assert_eq!(fit.summary, "follows codebase style");
        assert!(fit.evidence.is_empty());
    }

    #[test]
    fn grounded_codebase_fit_mismatch_keeps_evidence() {
        let input = input(ReviewPartnerContextPack::empty());
        let response = ReviewPartnerResponse {
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: Vec::new(),
            focus_records: vec![ReviewPartnerFocusRecordResponse {
                key: "layer:layer-1".to_string(),
                title: "Focus record".to_string(),
                subtitle: None,
                summary: Some("Style verdict summary.".to_string()),
                usage_context: Vec::new(),
                codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                    follows: false,
                    summary: "This uses a different row structure than nearby panels.".to_string(),
                    evidence: vec![ReviewPartnerItemResponse {
                        title: "Existing panel row".to_string(),
                        detail: "Nearby rows use a compact icon and title before details."
                            .to_string(),
                        path: Some("src/panel.rs".to_string()),
                        line: Some(12),
                    }],
                }),
                sections: Vec::new(),
                limitations: Vec::new(),
            }],
        };

        let partner = merge_review_partner(response, &input, None).expect("partner context");
        let fit = &partner.focus_records[0].codebase_fit;

        assert!(!fit.follows);
        assert_eq!(fit.evidence.len(), 1);
        assert_eq!(fit.evidence[0].path.as_deref(), Some("src/panel.rs"));
    }

    #[test]
    fn merge_preserves_complete_review_briefs() {
        let input = input(ReviewPartnerContextPack::empty());
        let long_brief = "This complete review partner brief keeps the surrounding context visible without replacing the final clause with an abbreviation. ".repeat(4);
        let expected_brief = long_brief.trim().to_string();
        assert!(expected_brief.len() > MAX_ITEM_TEXT_CHARS);

        let response = ReviewPartnerResponse {
            stack_brief: long_brief.clone(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            layers: vec![ReviewPartnerLayerResponse {
                layer_id: "layer-1".to_string(),
                brief: long_brief,
                changed_items: Vec::new(),
                removed_items: Vec::new(),
                usage_context: Vec::new(),
                similar_code: Vec::new(),
                codebase_fit: Vec::new(),
                concerns: Vec::new(),
                limitations: Vec::new(),
            }],
            focus_records: Vec::new(),
        };

        let partner = merge_review_partner(response, &input, None).expect("partner context");

        assert_eq!(partner.stack_brief, expected_brief);
        assert_eq!(partner.layers[0].brief, expected_brief);
    }

    #[test]
    fn removed_symbols_find_remaining_reference() {
        let root = unique_test_directory("review-partner-reference");
        fs::create_dir_all(root.join("src")).expect("dir");
        fs::write(
            root.join("src/lib.rs"),
            "fn caller() { removed_helper(); }\nfn other() {}\n",
        )
        .expect("write");

        let detail = detail_with_deleted_symbol();
        let atom = atom("atom-1", "src/lib.rs", 1, 1);
        let removed = collect_removed_symbols(&detail, &[&atom], &root, &mut Vec::new());

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].symbol, "removed_helper");
        assert!(removed[0].reference_count >= 1);
        assert_eq!(removed[0].references[0].path, "src/lib.rs");
    }

    #[test]
    fn removed_symbols_report_no_remaining_reference() {
        let root = unique_test_directory("review-partner-no-reference");
        fs::create_dir_all(root.join("src")).expect("dir");
        fs::write(root.join("src/lib.rs"), "fn caller() { other(); }\n").expect("write");

        let detail = detail_with_deleted_symbol();
        let atom = atom("atom-1", "src/lib.rs", 1, 1);
        let removed = collect_removed_symbols(&detail, &[&atom], &root, &mut Vec::new());

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].symbol, "removed_helper");
        assert_eq!(removed[0].reference_count, 0);
        assert!(removed[0].references.is_empty());
    }

    #[test]
    fn lsp_unavailable_usage_context_uses_tree_sitter() {
        let root = unique_test_directory("review-partner-tree-sitter-usage");
        fs::create_dir_all(root.join("src")).expect("dir");
        fs::write(
            root.join("src/lib.rs"),
            [
                "fn changed_helper() {}",
                "fn caller_one() { changed_helper(); }",
                "fn caller_two() { changed_helper(); }",
                "fn caller_three() { changed_helper(); }",
            ]
            .join("\n"),
        )
        .expect("write");

        let mut atom = atom("atom-1", "src/lib.rs", 1, 1);
        atom.symbol_name = Some("changed_helper".to_string());
        atom.defined_symbols = vec!["changed_helper".to_string()];
        let layer = ReviewStackLayer {
            id: "layer-1".to_string(),
            index: 0,
            title: "Changed helper".to_string(),
            summary: "Layer summary".to_string(),
            rationale: "Layer rationale".to_string(),
            pr: None,
            virtual_layer: None,
            base_oid: None,
            head_oid: None,
            atom_ids: vec![atom.id.clone()],
            depends_on_layer_ids: Vec::new(),
            metrics: LayerMetrics::default(),
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Medium,
            warnings: Vec::new(),
        };
        let collected = collect_layer_context(
            &detail_with_deleted_symbol(),
            &layer,
            &[&atom],
            &root,
            None,
            None,
            &mut Vec::new(),
        );

        assert_eq!(collected.changed_symbols.len(), 1);
        assert_eq!(
            collected.changed_symbols[0].search_strategy,
            "tree-sitter rust identifier scan"
        );
        assert!(collected.changed_symbols[0].references.len() <= MAX_REFERENCES_PER_SYMBOL);
        assert!(collected.changed_symbols[0].references.len() >= 3);
        let target = focus_target_from_layer(
            &ReviewStack {
                id: "stack".to_string(),
                repository: "acme/widgets".to_string(),
                selected_pr_number: 42,
                source: StackSource::VirtualAi,
                kind: StackKind::Virtual,
                confidence: Confidence::Medium,
                trunk_branch: Some("main".to_string()),
                base_oid: None,
                head_oid: None,
                generated_at_ms: stack_now_ms(),
                generator_version: STACK_GENERATOR_VERSION.to_string(),
                layers: vec![layer.clone()],
                atoms: vec![atom.clone()],
                warnings: Vec::new(),
                provider: None,
            },
            &layer,
        );
        let context = ReviewPartnerContextPack {
            version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
            layers: vec![collected],
            warnings: Vec::new(),
        };
        let usage = usage_groups_for_target(&target, &context);
        assert_eq!(usage.len(), 1);
        assert!(usage[0].summary.contains("tree-sitter"));
    }

    #[test]
    fn similar_locations_search_same_module_first() {
        let root = unique_test_directory("review-partner-similar-scope");
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::create_dir_all(root.join("tests")).expect("tests dir");
        fs::write(root.join("src/lib.rs"), "fn render_checkout_prompt() {}\n").expect("write");
        fs::write(root.join("src/sidebar.rs"), "fn checkout_row() {}\n").expect("write");
        fs::write(root.join("tests/checkout.rs"), "fn checkout_test() {}\n").expect("write");

        let symbol = ReviewPartnerCollectedSymbol {
            symbol: "render_checkout_prompt".to_string(),
            path: "src/lib.rs".to_string(),
            line: Some(1),
            atom_ids: vec!["atom-1".to_string()],
            search_strategy: "test".to_string(),
            reference_count: 0,
            references: Vec::new(),
        };

        let locations = collect_similar_locations(&[symbol], &root, 4);

        assert_eq!(
            locations.first().map(|location| location.path.as_str()),
            Some("src/sidebar.rs")
        );
    }

    #[test]
    fn similar_locations_skip_comment_only_matches() {
        let root = unique_test_directory("review-partner-similar-comments");
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::write(root.join("src/lib.rs"), "fn render_checkout_prompt() {}\n").expect("write");
        fs::write(
            root.join("src/comment.rs"),
            "// checkout only appears here\n",
        )
        .expect("write");
        fs::write(root.join("src/real.rs"), "fn checkout_row() {}\n").expect("write");

        let symbol = ReviewPartnerCollectedSymbol {
            symbol: "render_checkout_prompt".to_string(),
            path: "src/lib.rs".to_string(),
            line: Some(1),
            atom_ids: vec!["atom-1".to_string()],
            search_strategy: "test".to_string(),
            reference_count: 0,
            references: Vec::new(),
        };

        let locations = collect_similar_locations(&[symbol], &root, 4);

        assert!(locations
            .iter()
            .any(|location| location.path == "src/real.rs"));
        assert!(locations
            .iter()
            .all(|location| location.path != "src/comment.rs"));
    }

    fn input(context: ReviewPartnerContextPack) -> GenerateReviewPartnerInput {
        let stack = stack();
        let structural_evidence = StructuralEvidencePack::empty();
        let focus_targets = build_review_partner_focus_targets(&stack, &structural_evidence);

        GenerateReviewPartnerInput {
            provider: CodeTourProvider::Codex,
            working_directory: "/tmp".to_string(),
            repository: "acme/widgets".to_string(),
            number: 42,
            code_version_key: "head-a".to_string(),
            title: "Improve widgets".to_string(),
            body: String::new(),
            url: "https://github.com/acme/widgets/pull/42".to_string(),
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            stack,
            structural_evidence,
            semantic_review: None,
            context,
            focus_targets,
        }
    }

    fn stack() -> ReviewStack {
        ReviewStack {
            id: "stack".to_string(),
            repository: "acme/widgets".to_string(),
            selected_pr_number: 42,
            source: StackSource::VirtualAi,
            kind: StackKind::Virtual,
            confidence: Confidence::Medium,
            trunk_branch: Some("main".to_string()),
            base_oid: None,
            head_oid: None,
            generated_at_ms: stack_now_ms(),
            generator_version: STACK_GENERATOR_VERSION.to_string(),
            layers: vec![ReviewStackLayer {
                id: "layer-1".to_string(),
                index: 0,
                title: "Review behavior".to_string(),
                summary: "Layer summary".to_string(),
                rationale: "Layer rationale".to_string(),
                pr: None,
                virtual_layer: None,
                base_oid: None,
                head_oid: None,
                atom_ids: vec!["atom-1".to_string()],
                depends_on_layer_ids: Vec::new(),
                metrics: LayerMetrics::default(),
                status: LayerReviewStatus::NotReviewed,
                confidence: Confidence::Medium,
                warnings: Vec::new(),
            }],
            atoms: vec![atom("atom-1", "src/lib.rs", 1, 1)],
            warnings: Vec::new(),
            provider: None,
        }
    }

    fn semantic_review_for_stack(stack: &ReviewStack) -> RemissSemanticReview {
        build_semantic_review_from_contents(
            &detail_with_deleted_symbol(),
            &stack.atoms,
            &[RemissSemFileContents {
                path: "src/lib.rs".to_string(),
                previous_path: None,
                before_content: Some("fn removed_helper() -> i32 { 1 }\n".to_string()),
                after_content: Some("fn removed_helper() -> i32 { 2 }\n".to_string()),
            }],
            &sem_core::embedded::SemEmbeddedOptions::default(),
        )
    }

    fn partner_document(
        stack: ReviewStack,
        structural_evidence: StructuralEvidencePack,
    ) -> GeneratedReviewPartnerContext {
        let focus_targets = build_review_partner_focus_targets(&stack, &structural_evidence);
        GeneratedReviewPartnerContext {
            provider: CodeTourProvider::Codex,
            model: None,
            generated_at_ms: 1,
            code_version_key: "head-a".to_string(),
            generator_version: REVIEW_PARTNER_GENERATOR_VERSION.to_string(),
            context_version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
            structural_evidence_version: structural_evidence.version.clone(),
            stack_brief: "brief".to_string(),
            stack_concerns: Vec::new(),
            limitations: Vec::new(),
            warnings: Vec::new(),
            fallback_reason: None,
            stack,
            structural_evidence,
            semantic_review: None,
            context: ReviewPartnerContextPack::empty(),
            layers: Vec::new(),
            focus_targets,
            focus_records: Vec::new(),
        }
    }

    fn structural_evidence_with_hunk(
        path: &str,
        hunk_header: &str,
        hunk_index: usize,
        line: i64,
        atom_ids: Vec<String>,
    ) -> StructuralEvidencePack {
        StructuralEvidencePack {
            version: crate::structural_evidence::STRUCTURAL_EVIDENCE_VERSION.to_string(),
            files: vec![StructuralEvidenceFile {
                path: path.to_string(),
                previous_path: None,
                status: StructuralEvidenceStatus::Full,
                message: None,
                changes: vec![StructuralEvidenceChange {
                    hunk_index,
                    hunk_header: hunk_header.to_string(),
                    old_range: None,
                    new_range: Some(LineRange {
                        start: line,
                        end: line + 2,
                    }),
                    atom_ids: atom_ids.clone(),
                    changed_line_count: 2,
                    snippet: None,
                }],
                matched_atom_ids: atom_ids,
                unmatched_hunk_count: 0,
            }],
            warnings: Vec::new(),
        }
    }

    fn atom(id: &str, path: &str, start: i64, end: i64) -> ChangeAtom {
        ChangeAtom {
            id: id.to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 0 },
            path: path.to_string(),
            previous_path: None,
            role: ChangeRole::CoreLogic,
            semantic_kind: Some("logic".to_string()),
            symbol_name: Some("removed_helper".to_string()),
            defined_symbols: vec!["removed_helper".to_string()],
            referenced_symbols: Vec::new(),
            old_range: Some(LineRange { start, end }),
            new_range: Some(LineRange { start, end }),
            hunk_headers: Vec::new(),
            hunk_indices: vec![0],
            additions: 1,
            deletions: 1,
            patch_hash: "hash".to_string(),
            risk_score: 1,
            review_thread_ids: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn detail_with_deleted_symbol() -> PullRequestDetail {
        let parsed_diff = vec![ParsedDiffFile {
            path: "src/lib.rs".to_string(),
            previous_path: None,
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,1 +0,0 @@ fn removed_helper".to_string(),
                lines: vec![ParsedDiffLine {
                    kind: DiffLineKind::Deletion,
                    prefix: "-".to_string(),
                    left_line_number: Some(1),
                    right_line_number: None,
                    content: "fn removed_helper() {}".to_string(),
                }],
            }],
        }];

        PullRequestDetail {
            id: "PR_kwDO123".to_string(),
            repository: "acme/widgets".to_string(),
            number: 42,
            title: "Remove helper".to_string(),
            body: String::new(),
            url: "https://github.com/acme/widgets/pull/42".to_string(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: None,
            head_ref_oid: Some("head".to_string()),
            additions: 0,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-05-15T00:00:00Z".to_string(),
            updated_at: "2026-05-15T00:00:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            viewer_pending_review: None,
            files: vec![crate::github::PullRequestFile {
                path: "src/lib.rs".to_string(),
                additions: 0,
                deletions: 1,
                change_type: "modified".to_string(),
            }],
            raw_diff: String::new(),
            parsed_diff,
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        }
    }

    fn unique_test_directory(prefix: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), now_ms()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
