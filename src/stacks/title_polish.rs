use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    agents::{
        self,
        jsonrepair::parse_tolerant,
        prompt::{build_stack_title_polish_prompt, trim_text},
        AgentJsonPromptOptions,
    },
    cache::CacheStore,
    github::PullRequestDetail,
    review_ai::ReviewAiProvider,
};

use super::model::{
    normalize_stack_layer_title, stack_layer_title_quality_error, stack_now_ms, ChangeAtom,
    ReviewStack, ReviewStackLayer, StackKind, StackSource,
};

const STACK_TITLE_POLISH_CACHE_PREFIX: &str = "stack-title-polish-v1";
pub const STACK_TITLE_POLISH_VERSION: &str = "stack-title-polish-v1";
const MAX_TITLE_POLISH_LAYER_ATOMS: usize = 8;
const MAX_TITLE_POLISH_LAYER_FILES: usize = 10;
const MAX_TITLE_POLISH_LAYER_SYMBOLS: usize = 14;
const MAX_TITLE_POLISH_ATOM_SUMMARY_CHARS: usize = 220;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackTitlePolishDocument {
    pub version: String,
    pub provider: ReviewAiProvider,
    pub stack_id: String,
    pub code_version_key: String,
    pub titles: Vec<StackTitlePolishTitle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub prompt_bytes: usize,
    pub generated_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackTitlePolishTitle {
    pub layer_id: String,
    pub title: String,
}

pub fn stack_title_polish_cache_key(
    repository: &str,
    pr_number: i64,
    provider: ReviewAiProvider,
    code_version_key: &str,
    stack_id: &str,
) -> String {
    format!(
        "{STACK_TITLE_POLISH_CACHE_PREFIX}:{}#{}:{}:{}:{}",
        repository,
        pr_number,
        provider.slug(),
        code_version_key,
        stack_id
    )
}

pub fn polish_stack_titles_best_effort(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    stack: &ReviewStack,
    provider: ReviewAiProvider,
    code_version_key: &str,
    working_directory: &Path,
    force: bool,
) -> ReviewStack {
    if !should_polish_stack(stack) {
        return stack.clone();
    }

    if !force {
        if let Ok(Some(document)) =
            load_stack_title_polish(cache, detail, provider, code_version_key, &stack.id)
        {
            let mut polished = stack.clone();
            if apply_title_polish_document(&mut polished, &document).is_ok() {
                return polished;
            }
        }
    }

    let prompt = build_title_polish_prompt(detail, stack);
    let options = AgentJsonPromptOptions::stack_title_polish();
    if prompt.len() > options.max_prompt_bytes {
        return stack.clone();
    }

    let backend = agents::backend_for(provider);
    match backend.status() {
        Ok(status) if status.available && status.authenticated => {}
        _ => return stack.clone(),
    }

    let response = match agents::run_json_prompt_with_options(
        provider,
        working_directory.to_string_lossy().as_ref(),
        prompt,
        options,
    ) {
        Ok(response) => response,
        Err(_) => return stack.clone(),
    };

    let titles = match parse_title_polish_response(&response.text, stack) {
        Ok(titles) => titles,
        Err(_) => return stack.clone(),
    };
    let document = StackTitlePolishDocument {
        version: STACK_TITLE_POLISH_VERSION.to_string(),
        provider,
        stack_id: stack.id.clone(),
        code_version_key: code_version_key.to_string(),
        titles,
        model: response.model,
        prompt_bytes: response.prompt_bytes,
        generated_at_ms: stack_now_ms(),
    };
    let _ = save_stack_title_polish(cache, detail, &document);

    let mut polished = stack.clone();
    if apply_title_polish_document(&mut polished, &document).is_ok() {
        polished
    } else {
        stack.clone()
    }
}

fn should_polish_stack(stack: &ReviewStack) -> bool {
    stack.kind == StackKind::Virtual
        && stack.source != StackSource::VirtualAi
        && !stack.layers.is_empty()
}

fn load_stack_title_polish(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
    code_version_key: &str,
    stack_id: &str,
) -> Result<Option<StackTitlePolishDocument>, String> {
    let key = stack_title_polish_cache_key(
        &detail.repository,
        detail.number,
        provider,
        code_version_key,
        stack_id,
    );
    Ok(cache
        .get::<StackTitlePolishDocument>(&key)?
        .map(|document| document.value)
        .filter(|document| document.version == STACK_TITLE_POLISH_VERSION))
}

fn save_stack_title_polish(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    document: &StackTitlePolishDocument,
) -> Result<(), String> {
    let key = stack_title_polish_cache_key(
        &detail.repository,
        detail.number,
        document.provider,
        &document.code_version_key,
        &document.stack_id,
    );
    cache.put(&key, document, document.generated_at_ms)
}

fn build_title_polish_prompt(detail: &PullRequestDetail, stack: &ReviewStack) -> String {
    let input = build_title_polish_input(detail, stack);
    build_stack_title_polish_prompt(&input)
}

fn build_title_polish_input(detail: &PullRequestDetail, stack: &ReviewStack) -> Value {
    json!({
        "version": STACK_TITLE_POLISH_VERSION,
        "repository": stack.repository.as_str(),
        "pullRequest": {
            "number": stack.selected_pr_number,
            "title": detail.title.as_str(),
            "baseRef": detail.base_ref_name.as_str(),
            "headRef": detail.head_ref_name.as_str(),
        },
        "stack": {
            "id": stack.id.as_str(),
            "source": stack.source,
            "confidence": stack.confidence,
            "layerCount": stack.layers.len(),
        },
        "layers": stack.layers.iter().map(|layer| layer_title_polish_input(stack, layer)).collect::<Vec<_>>(),
    })
}

fn layer_title_polish_input(stack: &ReviewStack, layer: &ReviewStackLayer) -> Value {
    let atoms = stack.atoms_for_layer(layer);
    let files = limited_strings(
        layer_file_paths(layer, &atoms),
        MAX_TITLE_POLISH_LAYER_FILES,
    );
    let symbols = limited_strings(layer_symbols(&atoms), MAX_TITLE_POLISH_LAYER_SYMBOLS);
    let atom_summaries = atoms
        .iter()
        .take(MAX_TITLE_POLISH_LAYER_ATOMS)
        .map(|atom| atom_title_summary(atom))
        .collect::<Vec<_>>();

    json!({
        "id": layer.id.as_str(),
        "index": layer.index,
        "currentTitle": layer.title.as_str(),
        "role": layer.virtual_layer.as_ref().map(|virtual_layer| virtual_layer.role.label()),
        "sourceLabel": layer.virtual_layer.as_ref().map(|virtual_layer| virtual_layer.source_label.as_str()),
        "summary": trim_text(&layer.summary, MAX_TITLE_POLISH_ATOM_SUMMARY_CHARS),
        "rationale": trim_text(&layer.rationale, MAX_TITLE_POLISH_ATOM_SUMMARY_CHARS),
        "dependsOnLayerIds": &layer.depends_on_layer_ids,
        "metrics": {
            "files": layer.metrics.file_count,
            "atoms": layer.metrics.atom_count,
            "changedLines": layer.metrics.changed_lines,
            "risk": layer.metrics.risk_score,
        },
        "files": files,
        "symbols": symbols,
        "atoms": atom_summaries,
    })
}

fn layer_file_paths(layer: &ReviewStackLayer, atoms: &[&ChangeAtom]) -> Vec<String> {
    let mut files = BTreeSet::<String>::new();
    files.extend(atoms.iter().map(|atom| atom.path.clone()));
    files.extend(
        atoms
            .iter()
            .filter_map(|atom| atom.previous_path.as_ref().cloned()),
    );
    if let Some(pr) = layer.pr.as_ref() {
        files.insert(pr.head_ref_name.clone());
    }
    files.into_iter().collect()
}

fn layer_symbols(atoms: &[&ChangeAtom]) -> Vec<String> {
    let mut symbols = BTreeSet::<String>::new();
    for atom in atoms {
        if let Some(symbol) = atom.symbol_name.as_ref() {
            symbols.insert(symbol.clone());
        }
        symbols.extend(atom.defined_symbols.iter().cloned());
        symbols.extend(atom.referenced_symbols.iter().cloned());
    }
    symbols.into_iter().collect()
}

fn limited_strings(mut values: Vec<String>, limit: usize) -> Vec<String> {
    values.sort();
    values.dedup();
    values.into_iter().take(limit).collect()
}

fn atom_title_summary(atom: &ChangeAtom) -> Value {
    json!({
        "id": atom.id.as_str(),
        "path": atom.path.as_str(),
        "role": atom.role.label(),
        "semanticKind": atom.semantic_kind.as_deref(),
        "symbol": atom.symbol_name.as_deref(),
        "definedSymbols": limited_strings(atom.defined_symbols.clone(), 6),
        "referencedSymbols": limited_strings(atom.referenced_symbols.clone(), 6),
        "source": atom.source.stable_kind(),
        "changedLines": atom.additions + atom.deletions,
    })
}

fn parse_title_polish_response(
    raw: &str,
    stack: &ReviewStack,
) -> Result<Vec<StackTitlePolishTitle>, String> {
    let value = parse_tolerant::<Value>(raw).map_err(|error| error.message)?;
    let array = extract_title_polish_array(&value)
        .ok_or_else(|| "The title polish response did not contain a titles array.".to_string())?;
    let candidates = array
        .iter()
        .map(title_from_value)
        .collect::<Result<Vec<_>, _>>()?;
    validate_title_candidates(stack, candidates)
}

fn extract_title_polish_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        return Some(array);
    }
    let object = value.as_object()?;
    for key in [
        "titles",
        "titlePolishes",
        "title_polishes",
        "layers",
        "stackLayers",
        "stack_layers",
    ] {
        if let Some(array) = object.get(key).and_then(Value::as_array) {
            return Some(array);
        }
    }
    None
}

fn title_from_value(value: &Value) -> Result<StackTitlePolishTitle, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Each title polish entry must be an object.".to_string())?;
    let layer_id = string_field(object, &["layerId", "layer_id", "id"])
        .ok_or_else(|| "A title polish entry was missing layerId.".to_string())?;
    let title = string_field(object, &["title", "label"])
        .ok_or_else(|| format!("Title polish entry for '{layer_id}' was missing title."))?;
    Ok(StackTitlePolishTitle { layer_id, title })
}

fn string_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_title_candidates(
    stack: &ReviewStack,
    candidates: Vec<StackTitlePolishTitle>,
) -> Result<Vec<StackTitlePolishTitle>, String> {
    let expected_ids = stack
        .layers
        .iter()
        .map(|layer| layer.id.clone())
        .collect::<BTreeSet<_>>();
    let mut candidate_by_id = BTreeMap::<String, String>::new();
    for candidate in candidates {
        if !expected_ids.contains(&candidate.layer_id) {
            return Err(format!(
                "Title polish returned unknown layer id '{}'.",
                candidate.layer_id
            ));
        }
        if candidate_by_id
            .insert(candidate.layer_id.clone(), candidate.title)
            .is_some()
        {
            return Err(format!(
                "Title polish returned duplicate layer id '{}'.",
                candidate.layer_id
            ));
        }
    }
    if candidate_by_id.len() != expected_ids.len() {
        return Err("Title polish did not return exactly one title for every layer.".to_string());
    }

    let mut seen_titles = BTreeSet::<String>::new();
    let mut titles = Vec::new();
    for layer in &stack.layers {
        let raw = candidate_by_id
            .get(&layer.id)
            .expect("candidate count checked above");
        let title = normalize_stack_layer_title(raw, &layer.title);
        if let Some(error) = stack_layer_title_quality_error(&title) {
            return Err(format!(
                "Title polish returned invalid title '{}' for layer '{}': {}.",
                title, layer.id, error
            ));
        }
        let normalized = title.to_ascii_lowercase();
        if !seen_titles.insert(normalized) {
            return Err(format!(
                "Title polish returned duplicate title '{}'.",
                title
            ));
        }
        titles.push(StackTitlePolishTitle {
            layer_id: layer.id.clone(),
            title,
        });
    }
    Ok(titles)
}

fn apply_title_polish_document(
    stack: &mut ReviewStack,
    document: &StackTitlePolishDocument,
) -> Result<(), String> {
    if document.version != STACK_TITLE_POLISH_VERSION || document.stack_id != stack.id {
        return Err("Title polish document does not match this stack.".to_string());
    }
    let titles = validate_title_candidates(stack, document.titles.clone())?;
    for title in titles {
        if let Some(layer) = stack
            .layers
            .iter_mut()
            .find(|layer| layer.id == title.layer_id)
        {
            layer.title = title.title;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stacks::model::{
        Confidence, LayerMetrics, LayerReviewStatus, LineRange, StackProviderMetadata,
        VirtualLayerRef,
    };

    #[test]
    fn title_polish_cache_key_varies_by_provider() {
        let codex = stack_title_polish_cache_key(
            "acme/repo",
            1,
            ReviewAiProvider::Codex,
            "head-a",
            "stack-a",
        );
        let copilot = stack_title_polish_cache_key(
            "acme/repo",
            1,
            ReviewAiProvider::Copilot,
            "head-a",
            "stack-a",
        );

        assert_ne!(codex, copilot);
    }

    #[test]
    fn parses_and_applies_titles_without_changing_structure() {
        let mut stack = stack();
        let original_ids = stack.layers[0].atom_ids.clone();
        let original_deps = stack.layers[1].depends_on_layer_ids.clone();
        let raw = r#"{
            "titles": [
                {"layerId": "layer-1", "title": "File-backed context bundle"},
                {"layerId": "layer-2", "title": "Manifest prompt wiring"}
            ]
        }"#;

        let titles = parse_title_polish_response(raw, &stack).expect("titles");
        let document = StackTitlePolishDocument {
            version: STACK_TITLE_POLISH_VERSION.to_string(),
            provider: ReviewAiProvider::Codex,
            stack_id: stack.id.clone(),
            code_version_key: "head-a".to_string(),
            titles,
            model: None,
            prompt_bytes: 10,
            generated_at_ms: 1,
        };
        apply_title_polish_document(&mut stack, &document).expect("apply");

        assert_eq!(stack.layers[0].title, "File-backed context bundle");
        assert_eq!(stack.layers[1].title, "Manifest prompt wiring");
        assert_eq!(stack.layers[0].atom_ids, original_ids);
        assert_eq!(stack.layers[1].depends_on_layer_ids, original_deps);
    }

    #[test]
    fn rejects_missing_extra_duplicate_and_generic_titles() {
        let stack = stack();

        assert!(parse_title_polish_response(
            r#"{"titles":[{"layerId":"layer-1","title":"Context bundle"}]}"#,
            &stack,
        )
        .is_err());
        assert!(parse_title_polish_response(
            r#"{"titles":[
                {"layerId":"layer-1","title":"Context bundle"},
                {"layerId":"missing","title":"Missing layer"}
            ]}"#,
            &stack,
        )
        .is_err());
        assert!(parse_title_polish_response(
            r#"{"titles":[
                {"layerId":"layer-1","title":"Remaining changes"},
                {"layerId":"layer-2","title":"Manifest prompt wiring"}
            ]}"#,
            &stack,
        )
        .is_err());
    }

    #[test]
    fn title_polish_input_is_small_and_layer_scoped() {
        let detail = detail();
        let stack = stack();
        let input = build_title_polish_input(&detail, &stack);
        let prompt = build_stack_title_polish_prompt(&input);

        assert!(prompt.len() < AgentJsonPromptOptions::stack_title_polish().max_prompt_bytes);
        assert!(prompt.contains("currentTitle"));
        assert!(prompt.contains("FileContextBuilder"));
        assert!(!prompt.contains("raw_diff"));
    }

    fn detail() -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_kw".to_string(),
            repository: "acme/repo".to_string(),
            number: 1,
            title: "Improve review partner context".to_string(),
            body: String::new(),
            url: "https://github.com/acme/repo/pull/1".to_string(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: Some("base".to_string()),
            head_ref_oid: Some("head".to_string()),
            additions: 10,
            deletions: 2,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: Default::default(),
            comments: Vec::new(),
            commits: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            viewer_pending_review: None,
            files: Vec::new(),
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: Default::default(),
        }
    }

    fn stack() -> ReviewStack {
        ReviewStack {
            id: "stack-a".to_string(),
            repository: "acme/repo".to_string(),
            selected_pr_number: 1,
            source: StackSource::VirtualSemantic,
            kind: StackKind::Virtual,
            confidence: Confidence::High,
            trunk_branch: Some("main".to_string()),
            base_oid: Some("base".to_string()),
            head_oid: Some("head".to_string()),
            layers: vec![
                layer(
                    "layer-1",
                    0,
                    "Update review_partner",
                    vec!["atom-1"],
                    Vec::new(),
                ),
                layer(
                    "layer-2",
                    1,
                    "Update prompts",
                    vec!["atom-2"],
                    vec!["layer-1"],
                ),
            ],
            atoms: vec![
                atom(
                    "atom-1",
                    "src/review_partner/bundle.rs",
                    "FileContextBuilder",
                ),
                atom("atom-2", "src/agents/prompt.rs", "buildReviewPrompt"),
            ],
            warnings: Vec::new(),
            provider: Some(StackProviderMetadata {
                provider: "sem_virtual_stack".to_string(),
                raw_payload: None,
            }),
            generated_at_ms: 1,
            generator_version: "test".to_string(),
        }
    }

    fn layer(
        id: &str,
        index: usize,
        title: &str,
        atom_ids: Vec<&str>,
        deps: Vec<&str>,
    ) -> ReviewStackLayer {
        ReviewStackLayer {
            id: id.to_string(),
            index,
            title: title.to_string(),
            summary: format!("{title} summary"),
            rationale: format!("{title} rationale"),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualSemantic,
                role: super::super::model::ChangeRole::CoreLogic,
                source_label: "sem-layer".to_string(),
            }),
            base_oid: Some("base".to_string()),
            head_oid: Some("head".to_string()),
            atom_ids: atom_ids.into_iter().map(str::to_string).collect(),
            depends_on_layer_ids: deps.into_iter().map(str::to_string).collect(),
            metrics: LayerMetrics {
                file_count: 1,
                atom_count: 1,
                additions: 5,
                deletions: 1,
                changed_lines: 6,
                unresolved_thread_count: 0,
                risk_score: 2,
            },
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::High,
            warnings: Vec::new(),
        }
    }

    fn atom(id: &str, path: &str, symbol: &str) -> ChangeAtom {
        ChangeAtom {
            id: id.to_string(),
            source: super::super::model::ChangeAtomSource::SemanticSection {
                section_id: id.to_string(),
            },
            path: path.to_string(),
            previous_path: None,
            role: super::super::model::ChangeRole::CoreLogic,
            semantic_kind: Some("function".to_string()),
            symbol_name: Some(symbol.to_string()),
            defined_symbols: vec![symbol.to_string()],
            referenced_symbols: Vec::new(),
            old_range: None,
            new_range: Some(LineRange { start: 1, end: 12 }),
            hunk_headers: Vec::new(),
            hunk_indices: vec![0],
            additions: 5,
            deletions: 1,
            patch_hash: format!("hash-{id}"),
            risk_score: 2,
            review_thread_ids: Vec::new(),
            warnings: Vec::new(),
        }
    }
}
