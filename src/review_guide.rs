use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    agents::{self, jsonrepair::parse_tolerant},
    cache::CacheStore,
    code_tour::{tour_code_version_key, CodeTourProvider},
    github::PullRequestDetail,
    stacks::model::{ReviewStack, ReviewStackLayer},
    structural_evidence::{StructuralEvidencePack, StructuralEvidenceStatus},
};

pub const REVIEW_GUIDE_GENERATOR_VERSION: &str = "review-guide-v1";
const REVIEW_GUIDE_CACHE_KEY_PREFIX: &str = "review-guide-v1";
const MAX_GUIDE_LAYERS: usize = 24;
const MAX_LAYER_ATOMS: usize = 32;
const MAX_EVIDENCE_FILES: usize = 40;
const MAX_EVIDENCE_CHANGES: usize = 80;
const MAX_EVIDENCE_SNIPPET_CHARS: usize = 360;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedReviewGuide {
    pub provider: CodeTourProvider,
    #[serde(default)]
    pub model: Option<String>,
    pub generated_at_ms: i64,
    pub code_version_key: String,
    pub generator_version: String,
    pub structural_evidence_version: String,
    pub summary: String,
    pub review_focus: String,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub stack: ReviewStack,
    pub structural_evidence: StructuralEvidencePack,
    pub layers: Vec<ReviewGuideLayer>,
}

impl GeneratedReviewGuide {
    pub fn layer(&self, layer_id: &str) -> Option<&ReviewGuideLayer> {
        self.layers.iter().find(|layer| layer.layer_id == layer_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewGuideLayer {
    pub layer_id: String,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub review_question: String,
    #[serde(default)]
    pub review_points: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub risk_notes: Vec<String>,
    #[serde(default)]
    pub structural_notes: Vec<String>,
    pub structural_evidence_status: StructuralEvidenceStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateReviewGuideInput {
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewGuideResponse {
    summary: String,
    review_focus: String,
    #[serde(default)]
    open_questions: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    layers: Vec<ReviewGuideLayerResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewGuideLayerResponse {
    layer_id: String,
    summary: String,
    rationale: String,
    review_question: String,
    #[serde(default)]
    review_points: Vec<String>,
    #[serde(default)]
    open_questions: Vec<String>,
    #[serde(default)]
    risk_notes: Vec<String>,
    #[serde(default)]
    structural_notes: Vec<String>,
}

pub fn load_review_guide(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> Result<Option<GeneratedReviewGuide>, String> {
    let cache_key = review_guide_cache_key(detail, provider);
    Ok(cache
        .get::<GeneratedReviewGuide>(&cache_key)?
        .map(|document| document.value))
}

pub fn save_review_guide(cache: &CacheStore, guide: &GeneratedReviewGuide) -> Result<(), String> {
    let cache_key = review_guide_cache_key_from_parts(
        &guide.stack.repository,
        guide.stack.selected_pr_number,
        guide.provider,
        &guide.code_version_key,
        &guide.generator_version,
        &guide.structural_evidence_version,
    );
    cache.put(&cache_key, guide, now_ms())
}

pub fn generate_review_guide(
    cache: &CacheStore,
    input: GenerateReviewGuideInput,
) -> Result<GeneratedReviewGuide, String> {
    if input.working_directory.trim().is_empty() {
        return Err("Guided Review generation requires a local checkout path.".to_string());
    }

    if !Path::new(&input.working_directory).exists() {
        return Err(format!(
            "The local checkout path '{}' does not exist.",
            input.working_directory
        ));
    }

    let prompt = build_review_guide_prompt(&input);
    let response = agents::run_json_prompt(input.provider, &input.working_directory, prompt)?;
    let parsed = parse_tolerant::<ReviewGuideResponse>(&response.text)
        .map_err(|error| format!("Failed to parse review guide JSON: {}", error.message))?;
    let guide = merge_review_guide(parsed, &input, response.model)?;
    save_review_guide(cache, &guide)?;
    Ok(guide)
}

pub fn fallback_review_guide(
    input: &GenerateReviewGuideInput,
    warning: Option<String>,
) -> GeneratedReviewGuide {
    let mut warnings = input.structural_evidence.warnings.clone();
    if let Some(warning) = warning {
        warnings.push(warning);
    }

    GeneratedReviewGuide {
        provider: input.provider,
        model: None,
        generated_at_ms: now_ms(),
        code_version_key: input.code_version_key.clone(),
        generator_version: REVIEW_GUIDE_GENERATOR_VERSION.to_string(),
        structural_evidence_version: input.structural_evidence.version.clone(),
        summary: fallback_summary(&input.stack),
        review_focus: "Review each generated layer in order and use structural evidence as supporting context where available.".to_string(),
        open_questions: Vec::new(),
        warnings,
        stack: input.stack.clone(),
        structural_evidence: input.structural_evidence.clone(),
        layers: input
            .stack
            .layers
            .iter()
            .map(|layer| fallback_layer(layer, &input.structural_evidence))
            .collect(),
    }
}

pub fn build_review_guide_generation_input(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
    working_directory: &str,
    stack: ReviewStack,
    structural_evidence: StructuralEvidencePack,
) -> GenerateReviewGuideInput {
    GenerateReviewGuideInput {
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
    }
}

pub fn build_review_guide_request_key(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> String {
    format!(
        "{}:{}#{}:{}:{}:{}",
        provider.slug(),
        detail.repository,
        detail.number,
        tour_code_version_key(detail),
        REVIEW_GUIDE_GENERATOR_VERSION,
        crate::structural_evidence::STRUCTURAL_EVIDENCE_VERSION,
    )
}

pub fn review_guide_cache_key(detail: &PullRequestDetail, provider: CodeTourProvider) -> String {
    review_guide_cache_key_from_parts(
        &detail.repository,
        detail.number,
        provider,
        &tour_code_version_key(detail),
        REVIEW_GUIDE_GENERATOR_VERSION,
        crate::structural_evidence::STRUCTURAL_EVIDENCE_VERSION,
    )
}

pub fn review_guide_cache_key_from_parts(
    repository: &str,
    number: i64,
    provider: CodeTourProvider,
    code_version: &str,
    guide_version: &str,
    evidence_version: &str,
) -> String {
    format!(
        "{REVIEW_GUIDE_CACHE_KEY_PREFIX}:{}:{}:{}:{}:{}:{}",
        provider.slug(),
        repository,
        number,
        code_version,
        guide_version,
        evidence_version,
    )
}

pub fn build_review_guide_prompt(input: &GenerateReviewGuideInput) -> String {
    let context =
        serde_json::to_string_pretty(&build_prompt_context(input)).expect("context must serialize");
    let schema =
        serde_json::to_string_pretty(&review_guide_output_schema()).expect("schema must serialize");

    [
        "You are generating Guided Review layer notes for Remiss, a read-only pull request review IDE.",
        "The stack layers are already validated. Do not change the layer order, layer IDs, or atom coverage.",
        "Use structural evidence as deterministic support for explaining what changed, but do not claim it is complete when a file is partial or unavailable.",
        "Return strict JSON only. No markdown fences or prose outside JSON.",
        "Every response layer must use an existing layerId from pullRequest.stack.layers. Do not invent layer IDs or atom IDs.",
        "Keep prose short and reviewer-facing. Focus on what to verify, why the layer matters, and what structural diffs make clearer.",
        "",
        "JSON schema:",
        &schema,
        "",
        "Pull-request context:",
        &context,
    ]
    .join("\n")
}

fn merge_review_guide(
    response: ReviewGuideResponse,
    input: &GenerateReviewGuideInput,
    model: Option<String>,
) -> Result<GeneratedReviewGuide, String> {
    let valid_layer_ids = input
        .stack
        .layers
        .iter()
        .map(|layer| layer.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut seen = std::collections::BTreeSet::<String>::new();
    let mut response_layers = std::collections::BTreeMap::<String, ReviewGuideLayerResponse>::new();

    for layer in response.layers {
        if !valid_layer_ids.contains(&layer.layer_id) {
            return Err(format!(
                "Review guide response referenced unknown layer id '{}'.",
                layer.layer_id
            ));
        }
        if !seen.insert(layer.layer_id.clone()) {
            return Err(format!(
                "Review guide response duplicated layer id '{}'.",
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
                .map(|response| merge_layer(layer, response, &input.structural_evidence))
                .unwrap_or_else(|| fallback_layer(layer, &input.structural_evidence))
        })
        .collect::<Vec<_>>();

    Ok(GeneratedReviewGuide {
        provider: input.provider,
        model,
        generated_at_ms: now_ms(),
        code_version_key: input.code_version_key.clone(),
        generator_version: REVIEW_GUIDE_GENERATOR_VERSION.to_string(),
        structural_evidence_version: input.structural_evidence.version.clone(),
        summary: response.summary,
        review_focus: response.review_focus,
        open_questions: response.open_questions,
        warnings: response.warnings,
        stack: input.stack.clone(),
        structural_evidence: input.structural_evidence.clone(),
        layers,
    })
}

fn merge_layer(
    layer: &ReviewStackLayer,
    response: ReviewGuideLayerResponse,
    evidence: &StructuralEvidencePack,
) -> ReviewGuideLayer {
    ReviewGuideLayer {
        layer_id: layer.id.clone(),
        title: layer.title.clone(),
        summary: default_if_empty(response.summary, &layer.summary),
        rationale: default_if_empty(response.rationale, &layer.rationale),
        review_question: default_if_empty(response.review_question, &layer.rationale),
        review_points: response.review_points,
        open_questions: response.open_questions,
        risk_notes: response.risk_notes,
        structural_notes: response.structural_notes,
        structural_evidence_status: evidence.status_for_atom_ids(&layer.atom_ids),
    }
}

fn fallback_layer(layer: &ReviewStackLayer, evidence: &StructuralEvidencePack) -> ReviewGuideLayer {
    let status = evidence.status_for_atom_ids(&layer.atom_ids);
    ReviewGuideLayer {
        layer_id: layer.id.clone(),
        title: layer.title.clone(),
        summary: layer.summary.clone(),
        rationale: layer.rationale.clone(),
        review_question: layer.rationale.clone(),
        review_points: vec![format!("Verify {}", layer.title)],
        open_questions: Vec::new(),
        risk_notes: Vec::new(),
        structural_notes: vec![status.label().to_string()],
        structural_evidence_status: status,
    }
}

fn fallback_summary(stack: &ReviewStack) -> String {
    format!(
        "{} review layer{} prepared for this pull request.",
        stack.layers.len(),
        if stack.layers.len() == 1 { "" } else { "s" }
    )
}

fn default_if_empty(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn build_prompt_context(input: &GenerateReviewGuideInput) -> Value {
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
        "guideVersion": REVIEW_GUIDE_GENERATOR_VERSION,
        "structuralEvidenceVersion": input.structural_evidence.version,
        "stack": {
            "id": input.stack.id,
            "source": input.stack.source,
            "kind": input.stack.kind,
            "layers": input.stack.layers.iter().take(MAX_GUIDE_LAYERS).map(|layer| {
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
                    "sourceKind": atom.source.stable_kind(),
                    "role": atom.role.label(),
                    "semanticKind": atom.semantic_kind,
                    "symbolName": atom.symbol_name,
                    "oldRange": atom.old_range,
                    "newRange": atom.new_range,
                    "changedLineCount": atom.additions + atom.deletions,
                    "riskScore": atom.risk_score,
                })
            }).collect::<Vec<_>>(),
        },
        "structuralEvidence": summarize_structural_evidence(&input.structural_evidence),
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
                        "snippet": change.snippet.as_deref().map(|snippet| trim_text(snippet, MAX_EVIDENCE_SNIPPET_CHARS)),
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

fn review_guide_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "reviewFocus": { "type": "string" },
            "openQuestions": { "type": "array", "items": { "type": "string" } },
            "warnings": { "type": "array", "items": { "type": "string" } },
            "layers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "layerId": { "type": "string" },
                        "summary": { "type": "string" },
                        "rationale": { "type": "string" },
                        "reviewQuestion": { "type": "string" },
                        "reviewPoints": { "type": "array", "items": { "type": "string" } },
                        "openQuestions": { "type": "array", "items": { "type": "string" } },
                        "riskNotes": { "type": "array", "items": { "type": "string" } },
                        "structuralNotes": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["layerId", "summary", "rationale", "reviewQuestion", "reviewPoints", "openQuestions", "riskNotes", "structuralNotes"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["summary", "reviewFocus", "openQuestions", "warnings", "layers"],
        "additionalProperties": false
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stacks::model::{
        stack_now_ms, ChangeAtom, ChangeAtomSource, ChangeRole, Confidence, LayerMetrics,
        LayerReviewStatus, ReviewStackLayer, StackKind, StackSource,
    };

    #[test]
    fn guide_cache_key_includes_versions() {
        let key = review_guide_cache_key_from_parts(
            "acme/widgets",
            42,
            CodeTourProvider::Codex,
            "head-a",
            "guide-x",
            "evidence-y",
        );

        assert!(key.contains("guide-x"));
        assert!(key.contains("evidence-y"));
    }

    #[test]
    fn merge_rejects_unknown_layer_ids() {
        let input = input();
        let response = ReviewGuideResponse {
            summary: "summary".to_string(),
            review_focus: "focus".to_string(),
            open_questions: Vec::new(),
            warnings: Vec::new(),
            layers: vec![ReviewGuideLayerResponse {
                layer_id: "invented".to_string(),
                summary: "summary".to_string(),
                rationale: "rationale".to_string(),
                review_question: "question".to_string(),
                review_points: Vec::new(),
                open_questions: Vec::new(),
                risk_notes: Vec::new(),
                structural_notes: Vec::new(),
            }],
        };

        let error = merge_review_guide(response, &input, None).expect_err("unknown layer rejected");
        assert!(error.contains("unknown layer id"));
    }

    #[test]
    fn merge_fills_missing_layers_from_stack() {
        let input = input();
        let response = ReviewGuideResponse {
            summary: "summary".to_string(),
            review_focus: "focus".to_string(),
            open_questions: Vec::new(),
            warnings: Vec::new(),
            layers: Vec::new(),
        };

        let guide = merge_review_guide(response, &input, Some("model".to_string())).expect("guide");

        assert_eq!(guide.layers.len(), 1);
        assert_eq!(guide.layers[0].layer_id, "layer-1");
        assert_eq!(guide.layers[0].summary, "Layer summary");
        assert_eq!(guide.model.as_deref(), Some("model"));
    }

    fn input() -> GenerateReviewGuideInput {
        GenerateReviewGuideInput {
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
            stack: stack(),
            structural_evidence: StructuralEvidencePack::empty(),
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
            generator_version: "test".to_string(),
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
            atoms: vec![ChangeAtom {
                id: "atom-1".to_string(),
                source: ChangeAtomSource::File,
                path: "src/lib.rs".to_string(),
                previous_path: None,
                role: ChangeRole::CoreLogic,
                semantic_kind: Some("logic".to_string()),
                symbol_name: None,
                defined_symbols: Vec::new(),
                referenced_symbols: Vec::new(),
                old_range: None,
                new_range: None,
                hunk_headers: Vec::new(),
                hunk_indices: Vec::new(),
                additions: 1,
                deletions: 1,
                patch_hash: "hash".to_string(),
                risk_score: 1,
                review_thread_ids: Vec::new(),
                warnings: Vec::new(),
            }],
            warnings: Vec::new(),
            provider: None,
        }
    }
}
