use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use sem_core::{
    embedded::{
        analyze_file_changes, build_impact_context_from_graph, build_repo_graph,
        discover_repo_files, generate_review_layers, resolve_focus_target, SemDiffAnalysis,
        SemEmbeddedChange, SemEmbeddedOptions, SemEntityTarget, SemFileChange, SemFocusResolution,
        SemFocusTarget, SemFocusedEntity, SemHunk, SemHunkTarget, SemImpactRequest,
        SemLayerGenerationOptions, SemLineRange, SemLocationTarget, SemRepoScanOptions,
        SemReviewLayerPlan, SemSide, SEM_EMBEDDED_API_VERSION,
    },
    git::types::FileStatus,
    parser::graph::EntityInfo,
};
use serde::{Deserialize, Serialize};

use crate::{
    cache::CacheStore,
    code_tour::tour_code_version_key,
    diff::{ParsedDiffFile, ParsedDiffHunk},
    github::{PullRequestDetail, PullRequestFile},
    local_documents,
    stacks::model::{ChangeAtom, LineRange},
    structural_diff::{build_structural_diff_request, StructuralDiffSideRequest},
};

pub const REMISS_SEMANTIC_REVIEW_VERSION: &str = "remiss-semantic-review-v2";
const SEMANTIC_REVIEW_CACHE_PREFIX: &str = "semantic-review-v2";
const MAX_SEMANTIC_FOCUS_SUMMARIES: usize = 128;
const MAX_SEMANTIC_IMPACT_SUMMARIES: usize = 48;
const MAX_SEMANTIC_ENTITY_SUMMARIES: usize = 12;
const MAX_SEMANTIC_CHANGE_SUMMARIES: usize = 12;
const MAX_SEMANTIC_CONTEXT_ENTRIES: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemissSemFileContents {
    pub path: String,
    pub previous_path: Option<String>,
    pub before_content: Option<String>,
    pub after_content: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemissSemanticReview {
    pub version: String,
    pub sem_api_version: String,
    pub code_version_key: String,
    pub analysis: SemDiffAnalysis,
    pub layers: SemReviewLayerPlan,
    pub layer_atom_mappings: Vec<RemissSemLayerAtomMapping>,
    #[serde(default)]
    pub focus_summaries: Vec<RemissSemanticFocusSummary>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemissSemLayerAtomMapping {
    pub layer_id: String,
    pub atom_ids: Vec<String>,
    pub file_paths: Vec<String>,
    pub hunk_indices: Vec<usize>,
    pub entity_names: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticFocusSummary {
    pub atom_id: String,
    pub cache_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_entity: Option<RemissSemanticEntitySummary>,
    #[serde(default)]
    pub overlapping_entities: Vec<RemissSemanticEntitySummary>,
    #[serde(default)]
    pub matching_changes: Vec<RemissSemanticChangeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<RemissSemanticImpactSummary>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticEntitySummary {
    pub name: String,
    pub entity_type: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub side: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticChangeSummary {
    pub entity_name: String,
    pub entity_type: String,
    pub change_type: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_range: Option<RemissSemanticRangeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_range: Option<RemissSemanticRangeSummary>,
    #[serde(default)]
    pub hunk_indices: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticRangeSummary {
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticImpactSummary {
    pub cache_key: String,
    #[serde(default)]
    pub dependencies: Vec<RemissSemanticEntitySummary>,
    #[serde(default)]
    pub dependents: Vec<RemissSemanticEntitySummary>,
    #[serde(default)]
    pub impact: Vec<RemissSemanticEntitySummary>,
    #[serde(default)]
    pub tests: Vec<RemissSemanticEntitySummary>,
    #[serde(default)]
    pub context: Vec<RemissSemanticContextEntrySummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticContextEntrySummary {
    pub entity_name: String,
    pub entity_type: String,
    pub file_path: String,
    pub role: String,
    pub content: String,
    pub estimated_tokens: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticReviewSummary {
    pub version: String,
    pub sem_api_version: String,
    pub code_version_key: String,
    pub analysis_cache_key: String,
    pub layer_cache_key: String,
    pub file_count: usize,
    pub added_count: usize,
    pub modified_count: usize,
    pub deleted_count: usize,
    pub moved_count: usize,
    pub renamed_count: usize,
    pub reordered_count: usize,
    pub orphan_count: usize,
    pub change_count: usize,
    pub layer_count: usize,
    #[serde(default)]
    pub layers: Vec<RemissSemanticLayerSummary>,
    #[serde(default)]
    pub focus_summaries: Vec<RemissSemanticFocusSummary>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemissSemanticLayerSummary {
    pub id: String,
    pub index: usize,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    #[serde(default)]
    pub depends_on_layer_ids: Vec<String>,
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

pub fn build_semantic_review_from_contents(
    detail: &PullRequestDetail,
    atoms: &[ChangeAtom],
    contents: &[RemissSemFileContents],
    options: &SemEmbeddedOptions,
) -> RemissSemanticReview {
    let mut warnings = Vec::new();
    let changes = sem_file_changes_for_detail(detail, contents, &mut warnings);
    let mut review = build_semantic_review_from_changes(detail, atoms, &changes, options);
    enrich_semantic_focus_summaries(&mut review, atoms, &changes, None, options);
    review.warnings.extend(warnings);
    review.warnings.sort();
    review.warnings.dedup();
    review
}

fn build_semantic_review_from_changes(
    detail: &PullRequestDetail,
    atoms: &[ChangeAtom],
    changes: &[SemFileChange],
    options: &SemEmbeddedOptions,
) -> RemissSemanticReview {
    let analysis = analyze_file_changes(&changes, options);
    let layers = generate_review_layers(&changes, &SemLayerGenerationOptions::default(), options);
    let layer_atom_mappings =
        map_sem_layers_to_atoms_with_analysis(&layers, Some(&analysis), atoms);
    RemissSemanticReview {
        version: REMISS_SEMANTIC_REVIEW_VERSION.to_string(),
        sem_api_version: SEM_EMBEDDED_API_VERSION.to_string(),
        code_version_key: tour_code_version_key(detail),
        analysis,
        layers,
        layer_atom_mappings,
        focus_summaries: Vec::new(),
        warnings: Vec::new(),
    }
}

pub fn semantic_review_version_key() -> String {
    format!("{REMISS_SEMANTIC_REVIEW_VERSION}:{SEM_EMBEDDED_API_VERSION}")
}

pub fn semantic_review_cache_key(
    detail: &PullRequestDetail,
    head_oid: Option<&str>,
) -> Option<String> {
    let head_identity = semantic_review_head_identity(detail, head_oid)?;
    Some(format!(
        "{SEMANTIC_REVIEW_CACHE_PREFIX}:{}#{}:{}:{}:{}",
        detail.repository,
        detail.number,
        tour_code_version_key(detail),
        semantic_review_version_key(),
        head_identity,
    ))
}

pub fn load_semantic_review(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    head_oid: Option<&str>,
) -> Result<Option<RemissSemanticReview>, String> {
    let Some(cache_key) = semantic_review_cache_key(detail, head_oid) else {
        return Ok(None);
    };

    Ok(cache
        .get::<RemissSemanticReview>(&cache_key)?
        .map(|document| document.value)
        .filter(|review| semantic_review_matches_current(review, detail)))
}

pub fn save_semantic_review(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    head_oid: Option<&str>,
    review: &RemissSemanticReview,
) -> Result<(), String> {
    if !semantic_review_matches_current(review, detail) {
        return Ok(());
    }

    let Some(cache_key) = semantic_review_cache_key(detail, head_oid) else {
        return Ok(());
    };
    cache.put(&cache_key, review, now_ms())
}

pub fn build_and_cache_semantic_review(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    atoms: &[ChangeAtom],
    repository: &str,
    checkout_root: &Path,
    head_oid: Option<&str>,
    force: bool,
) -> Option<RemissSemanticReview> {
    if !force {
        if let Ok(Some(review)) = load_semantic_review(cache, detail, head_oid) {
            return Some(review);
        }
    }

    let Some(head_oid) = head_oid else {
        return Some(unavailable_semantic_review(
            detail,
            "Semantic evidence could not be built because checkout head was unavailable.",
        ));
    };

    let contents = load_semantic_file_contents(cache, detail, repository, checkout_root, head_oid);
    let options = SemEmbeddedOptions::with_root(checkout_root);
    let mut warnings = Vec::new();
    let changes = sem_file_changes_for_detail(detail, &contents.contents, &mut warnings);
    let mut review = build_semantic_review_from_changes(detail, atoms, &changes, &options);
    enrich_semantic_focus_summaries(&mut review, atoms, &changes, Some(checkout_root), &options);
    review.warnings.extend(warnings);
    review.warnings.extend(contents.warnings);
    review.warnings.sort();
    review.warnings.dedup();
    let _ = save_semantic_review(cache, detail, Some(head_oid), &review);
    Some(review)
}

#[derive(Clone, Debug)]
pub struct LoadedSemanticFileContents {
    pub contents: Vec<RemissSemFileContents>,
    pub warnings: Vec<String>,
}

pub fn load_semantic_file_contents(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    repository: &str,
    checkout_root: &Path,
    head_oid: &str,
) -> LoadedSemanticFileContents {
    let mut contents = Vec::new();
    let mut warnings = Vec::new();

    for file in &detail.files {
        let parsed = crate::code_tour::find_parsed_diff_file(&detail.parsed_diff, &file.path);
        if parsed.map(|parsed| parsed.is_binary).unwrap_or(false) {
            warnings.push(format!(
                "Semantic evidence is unavailable for binary file {}.",
                file.path
            ));
            contents.push(RemissSemFileContents {
                path: file.path.clone(),
                previous_path: parsed.and_then(|parsed| parsed.previous_path.clone()),
                before_content: None,
                after_content: None,
            });
            continue;
        }

        let Some(request) = build_structural_diff_request(detail, file, parsed, head_oid) else {
            warnings.push(format!(
                "No semantic file-content request for {}.",
                file.path
            ));
            contents.push(RemissSemFileContents {
                path: file.path.clone(),
                previous_path: parsed.and_then(|parsed| parsed.previous_path.clone()),
                before_content: None,
                after_content: None,
            });
            continue;
        };

        let before_content = load_semantic_side_text(
            cache,
            repository,
            checkout_root,
            &request.old_side,
            &mut warnings,
        );
        let after_content = load_semantic_side_text(
            cache,
            repository,
            checkout_root,
            &request.new_side,
            &mut warnings,
        );
        contents.push(RemissSemFileContents {
            path: request.path,
            previous_path: request.previous_path,
            before_content,
            after_content,
        });
    }

    LoadedSemanticFileContents { contents, warnings }
}

pub fn summarize_semantic_review(review: &RemissSemanticReview) -> RemissSemanticReviewSummary {
    let mappings_by_layer = review
        .layer_atom_mappings
        .iter()
        .map(|mapping| (mapping.layer_id.as_str(), mapping))
        .collect::<BTreeMap<_, _>>();
    let layers = review
        .layers
        .layers
        .iter()
        .map(|layer| {
            let mapping = mappings_by_layer.get(layer.id.as_str());
            RemissSemanticLayerSummary {
                id: layer.id.clone(),
                index: layer.index,
                title: layer.title.clone(),
                summary: layer.summary.clone(),
                rationale: layer.rationale.clone(),
                depends_on_layer_ids: layer.depends_on_layer_ids.clone(),
                atom_ids: mapping
                    .map(|mapping| mapping.atom_ids.clone())
                    .unwrap_or_default(),
                file_paths: layer.file_paths.clone(),
                hunk_indices: layer.hunk_indices.clone(),
                entity_names: layer.entity_names.clone(),
                change_count: layer.change_indices.len(),
            }
        })
        .collect::<Vec<_>>();

    RemissSemanticReviewSummary {
        version: review.version.clone(),
        sem_api_version: review.sem_api_version.clone(),
        code_version_key: review.code_version_key.clone(),
        analysis_cache_key: review.analysis.cache_key.clone(),
        layer_cache_key: review.layers.cache_key.clone(),
        file_count: review.analysis.summary.file_count,
        added_count: review.analysis.summary.added_count,
        modified_count: review.analysis.summary.modified_count,
        deleted_count: review.analysis.summary.deleted_count,
        moved_count: review.analysis.summary.moved_count,
        renamed_count: review.analysis.summary.renamed_count,
        reordered_count: review.analysis.summary.reordered_count,
        orphan_count: review.analysis.summary.orphan_count,
        change_count: review.analysis.changes.len(),
        layer_count: review.layers.layers.len(),
        layers,
        focus_summaries: review.focus_summaries.clone(),
        warnings: review
            .warnings
            .iter()
            .chain(review.layers.warnings.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

pub fn sem_file_changes_for_detail(
    detail: &PullRequestDetail,
    contents: &[RemissSemFileContents],
    warnings: &mut Vec<String>,
) -> Vec<SemFileChange> {
    let parsed_by_path = detail
        .parsed_diff
        .iter()
        .map(|parsed| (parsed.path.as_str(), parsed))
        .collect::<BTreeMap<_, _>>();
    let contents_by_path = contents
        .iter()
        .map(|content| (content.path.as_str(), content))
        .collect::<BTreeMap<_, _>>();

    detail
        .files
        .iter()
        .filter_map(|file| {
            let Some(content) = contents_by_path.get(file.path.as_str()) else {
                warnings.push(format!("No semantic contents supplied for {}.", file.path));
                return None;
            };
            Some(sem_file_change_from_remiss(
                file,
                parsed_by_path.get(file.path.as_str()).copied(),
                content,
            ))
        })
        .collect()
}

pub fn sem_file_change_from_remiss(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    content: &RemissSemFileContents,
) -> SemFileChange {
    SemFileChange {
        file_path: file.path.clone(),
        status: file_status_from_github(file.change_type.as_str()),
        old_file_path: content
            .previous_path
            .clone()
            .or_else(|| parsed.and_then(|parsed| parsed.previous_path.clone())),
        before_content: content.before_content.clone(),
        after_content: content.after_content.clone(),
        hunks: parsed.map(sem_hunks_from_parsed).unwrap_or_default(),
    }
}

pub fn sem_focus_target_for_atom(atom: &ChangeAtom) -> SemFocusTarget {
    let side = if atom.new_range.is_some() {
        SemSide::After
    } else {
        SemSide::Before
    };
    let location = atom_range_for_side(atom, side).and_then(|range| {
        usize::try_from(range.start)
            .ok()
            .map(|line| SemLocationTarget {
                file_path: file_path_for_side(atom, side),
                line,
                side,
            })
    });

    SemFocusTarget {
        entity: None,
        location,
        hunk: Some(SemHunkTarget {
            file_path: atom.path.clone(),
            old_file_path: atom.previous_path.clone(),
            hunk_index: atom.hunk_indices.first().copied(),
            hunk_header: atom.hunk_headers.first().cloned(),
            side,
            old_range: atom.old_range.and_then(sem_line_range_from_remiss),
            new_range: atom.new_range.and_then(sem_line_range_from_remiss),
        }),
    }
}

pub fn sem_focus_for_atom(
    changes: &[SemFileChange],
    atom: &ChangeAtom,
    options: &SemEmbeddedOptions,
) -> Result<SemFocusResolution, sem_core::embedded::SemError> {
    resolve_focus_target(changes, &sem_focus_target_for_atom(atom), options)
}

fn enrich_semantic_focus_summaries(
    review: &mut RemissSemanticReview,
    atoms: &[ChangeAtom],
    changes: &[SemFileChange],
    checkout_root: Option<&Path>,
    options: &SemEmbeddedOptions,
) {
    let mut focus_summaries = Vec::new();
    let mut warnings = Vec::new();

    let graph = checkout_root.and_then(|root| {
        let scan_options = SemRepoScanOptions {
            max_files: 2_000,
            max_file_bytes: 512_000,
            max_depth: 12,
            include_hidden: false,
            extra_skip_dirs: Vec::new(),
        };
        match discover_repo_files(root, &scan_options, options)
            .and_then(|discovered| build_repo_graph(root, &discovered.files, options))
        {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                warnings.push(format!(
                    "Semantic impact context could not build repository graph: {error}"
                ));
                None
            }
        }
    });

    for atom in atoms.iter().take(MAX_SEMANTIC_FOCUS_SUMMARIES) {
        match sem_focus_for_atom(changes, atom, options) {
            Ok(focus) => {
                let impact = if focus_summaries.len() < MAX_SEMANTIC_IMPACT_SUMMARIES {
                    graph
                        .as_ref()
                        .and_then(|snapshot| impact_summary_for_focus(snapshot, &focus, atom))
                } else {
                    None
                };
                focus_summaries.push(focus_summary_for_atom(atom, focus, impact));
            }
            Err(error) => warnings.push(format!(
                "Semantic focus could not resolve atom {}: {error}",
                atom.id
            )),
        }
    }

    if atoms.len() > MAX_SEMANTIC_FOCUS_SUMMARIES {
        warnings.push(format!(
            "Semantic focus context was capped at {} of {} atoms.",
            MAX_SEMANTIC_FOCUS_SUMMARIES,
            atoms.len()
        ));
    }

    review.focus_summaries = focus_summaries;
    review.warnings.extend(warnings);
}

fn impact_summary_for_focus(
    snapshot: &sem_core::embedded::SemGraphSnapshot,
    focus: &SemFocusResolution,
    atom: &ChangeAtom,
) -> Option<RemissSemanticImpactSummary> {
    let entity = focus.target_entity.as_ref()?;
    let request = SemImpactRequest {
        token_budget: 2_048,
        max_depth: 2,
    };
    let target = SemEntityTarget {
        entity_id: Some(entity.entity.id.clone()),
        entity_name: Some(entity.entity.name.clone()),
        file_path: Some(entity.entity.file_path.clone()),
    };
    let context = build_impact_context_from_graph(snapshot, &target, &request).ok()?;
    Some(RemissSemanticImpactSummary {
        cache_key: format!("{}:{}", atom.id, context.cache_key),
        dependencies: context
            .dependencies
            .iter()
            .take(MAX_SEMANTIC_ENTITY_SUMMARIES)
            .map(entity_info_summary)
            .collect(),
        dependents: context
            .dependents
            .iter()
            .take(MAX_SEMANTIC_ENTITY_SUMMARIES)
            .map(entity_info_summary)
            .collect(),
        impact: context
            .impact
            .iter()
            .take(MAX_SEMANTIC_ENTITY_SUMMARIES)
            .map(|impact| entity_info_summary(&impact.entity))
            .collect(),
        tests: context
            .tests
            .iter()
            .take(MAX_SEMANTIC_ENTITY_SUMMARIES)
            .map(entity_info_summary)
            .collect(),
        context: context
            .context
            .iter()
            .take(MAX_SEMANTIC_CONTEXT_ENTRIES)
            .map(|entry| RemissSemanticContextEntrySummary {
                entity_name: entry.entity_name.clone(),
                entity_type: entry.entity_type.clone(),
                file_path: entry.file_path.clone(),
                role: entry.role.clone(),
                content: crate::agents::prompt::trim_text(&entry.content, 800),
                estimated_tokens: entry.estimated_tokens,
            })
            .collect(),
    })
}

fn focus_summary_for_atom(
    atom: &ChangeAtom,
    focus: SemFocusResolution,
    impact: Option<RemissSemanticImpactSummary>,
) -> RemissSemanticFocusSummary {
    RemissSemanticFocusSummary {
        atom_id: atom.id.clone(),
        cache_key: focus.cache_key,
        target_entity: focus.target_entity.as_ref().map(focused_entity_summary),
        overlapping_entities: focus
            .overlapping_entities
            .iter()
            .take(MAX_SEMANTIC_ENTITY_SUMMARIES)
            .map(focused_entity_summary)
            .collect(),
        matching_changes: focus
            .matching_changes
            .iter()
            .take(MAX_SEMANTIC_CHANGE_SUMMARIES)
            .map(semantic_change_summary)
            .collect(),
        impact,
        warnings: focus.warnings,
    }
}

fn focused_entity_summary(entity: &SemFocusedEntity) -> RemissSemanticEntitySummary {
    RemissSemanticEntitySummary {
        name: entity.entity.name.clone(),
        entity_type: entity.entity.entity_type.clone(),
        file_path: entity.entity.file_path.clone(),
        start_line: entity.entity.start_line,
        end_line: entity.entity.end_line,
        side: sem_side_label(entity.side).to_string(),
    }
}

fn entity_info_summary(entity: &EntityInfo) -> RemissSemanticEntitySummary {
    RemissSemanticEntitySummary {
        name: entity.name.clone(),
        entity_type: entity.entity_type.clone(),
        file_path: entity.file_path.clone(),
        start_line: entity.start_line,
        end_line: entity.end_line,
        side: "after".to_string(),
    }
}

fn semantic_change_summary(change: &SemEmbeddedChange) -> RemissSemanticChangeSummary {
    RemissSemanticChangeSummary {
        entity_name: change.change.entity_name.clone(),
        entity_type: change.change.entity_type.clone(),
        change_type: change.change.change_type.to_string(),
        file_path: change.change.file_path.clone(),
        before_range: change.before_range.as_ref().map(semantic_range_summary),
        after_range: change.after_range.as_ref().map(semantic_range_summary),
        hunk_indices: change
            .hunk_overlaps
            .iter()
            .map(|overlap| overlap.hunk_index)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn semantic_range_summary(
    range: &sem_core::embedded::SemEntityRange,
) -> RemissSemanticRangeSummary {
    RemissSemanticRangeSummary {
        start_line: range.start_line,
        end_line: range.end_line,
    }
}

fn sem_side_label(side: SemSide) -> &'static str {
    match side {
        SemSide::Before => "before",
        SemSide::After => "after",
    }
}

pub fn map_sem_layers_to_atoms(
    layers: &SemReviewLayerPlan,
    atoms: &[ChangeAtom],
) -> Vec<RemissSemLayerAtomMapping> {
    map_sem_layers_to_atoms_with_analysis(layers, None, atoms)
}

fn map_sem_layers_to_atoms_with_analysis(
    layers: &SemReviewLayerPlan,
    analysis: Option<&SemDiffAnalysis>,
    atoms: &[ChangeAtom],
) -> Vec<RemissSemLayerAtomMapping> {
    layers
        .layers
        .iter()
        .map(|layer| {
            let layer_changes = analysis
                .map(|analysis| {
                    layer
                        .change_indices
                        .iter()
                        .filter_map(|index| analysis.changes.get(*index))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let atom_ids = atoms
                .iter()
                .filter(|atom| sem_layer_matches_atom(layer, &layer_changes, atom))
                .map(|atom| atom.id.clone())
                .collect::<Vec<_>>();
            RemissSemLayerAtomMapping {
                layer_id: layer.id.clone(),
                atom_ids,
                file_paths: layer.file_paths.clone(),
                hunk_indices: layer.hunk_indices.clone(),
                entity_names: layer.entity_names.clone(),
            }
        })
        .collect()
}

fn sem_layer_matches_atom(
    layer: &sem_core::embedded::SemReviewLayer,
    changes: &[&SemEmbeddedChange],
    atom: &ChangeAtom,
) -> bool {
    changes
        .iter()
        .any(|change| sem_change_matches_atom(change, atom))
        || sem_layer_hunks_match_atom(layer, atom)
}

fn sem_change_matches_atom(change: &SemEmbeddedChange, atom: &ChangeAtom) -> bool {
    if !sem_change_path_matches_atom(change, atom) {
        return false;
    }

    if sem_change_hunks_match_atom(change, atom)
        || sem_change_ranges_match_atom(change, atom)
        || sem_change_symbols_match_atom(change, atom)
    {
        return true;
    }

    atom.hunk_indices.is_empty() && atom.old_range.is_none() && atom.new_range.is_none()
}

fn sem_layer_hunks_match_atom(
    layer: &sem_core::embedded::SemReviewLayer,
    atom: &ChangeAtom,
) -> bool {
    let file_paths = layer.file_paths.iter().cloned().collect::<BTreeSet<_>>();
    if !atom_matches_file_paths(atom, &file_paths) {
        return false;
    }
    let hunk_indices = layer.hunk_indices.iter().copied().collect::<BTreeSet<_>>();
    hunk_indices.is_empty()
        || atom
            .hunk_indices
            .iter()
            .any(|index| hunk_indices.contains(index))
}

fn sem_change_path_matches_atom(change: &SemEmbeddedChange, atom: &ChangeAtom) -> bool {
    sem_change_paths(change)
        .iter()
        .any(|path| atom_matches_file_path(atom, path))
}

fn sem_change_paths(change: &SemEmbeddedChange) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    paths.insert(change.change.file_path.clone());
    if let Some(path) = change.change.old_file_path.clone() {
        paths.insert(path);
    }
    if let Some(range) = change.before_range.as_ref() {
        paths.insert(range.file_path.clone());
    }
    if let Some(range) = change.after_range.as_ref() {
        paths.insert(range.file_path.clone());
    }
    for overlap in &change.hunk_overlaps {
        paths.insert(overlap.file_path.clone());
        if let Some(path) = overlap.old_file_path.clone() {
            paths.insert(path);
        }
    }
    paths
}

fn sem_change_hunks_match_atom(change: &SemEmbeddedChange, atom: &ChangeAtom) -> bool {
    !atom.hunk_indices.is_empty()
        && change.hunk_overlaps.iter().any(|overlap| {
            atom.hunk_indices.contains(&overlap.hunk_index)
                && (atom_matches_file_path(atom, &overlap.file_path)
                    || overlap
                        .old_file_path
                        .as_ref()
                        .is_some_and(|path| atom_matches_file_path(atom, path)))
        })
}

fn sem_change_ranges_match_atom(change: &SemEmbeddedChange, atom: &ChangeAtom) -> bool {
    change.after_range.as_ref().is_some_and(|range| {
        repo_paths_equal(&atom.path, &range.file_path)
            && atom
                .new_range
                .is_some_and(|atom_range| sem_ranges_overlap(atom_range, range))
    }) || change.before_range.as_ref().is_some_and(|range| {
        atom_matches_file_path(atom, &range.file_path)
            && atom
                .old_range
                .is_some_and(|atom_range| sem_ranges_overlap(atom_range, range))
    })
}

fn sem_change_symbols_match_atom(change: &SemEmbeddedChange, atom: &ChangeAtom) -> bool {
    let mut sem_names = BTreeSet::new();
    sem_names.insert(normalize_sem_symbol(&change.change.entity_name));
    if let Some(name) = change.change.old_entity_name.as_deref() {
        sem_names.insert(normalize_sem_symbol(name));
    }
    atom.symbol_name
        .iter()
        .chain(atom.defined_symbols.iter())
        .map(|symbol| normalize_sem_symbol(symbol))
        .any(|symbol| !symbol.is_empty() && sem_names.contains(&symbol))
}

fn atom_matches_file_paths(atom: &ChangeAtom, file_paths: &BTreeSet<String>) -> bool {
    file_paths
        .iter()
        .any(|path| atom_matches_file_path(atom, path))
}

fn atom_matches_file_path(atom: &ChangeAtom, file_path: &str) -> bool {
    repo_paths_equal(&atom.path, file_path)
        || atom
            .previous_path
            .as_deref()
            .is_some_and(|path| repo_paths_equal(path, file_path))
}

fn repo_paths_equal(left: &str, right: &str) -> bool {
    normalize_repo_path(left) == normalize_repo_path(right)
}

fn normalize_repo_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn sem_ranges_overlap(
    atom_range: LineRange,
    sem_range: &sem_core::embedded::SemEntityRange,
) -> bool {
    let Ok(atom_start) = usize::try_from(atom_range.start) else {
        return false;
    };
    let Ok(atom_end) = usize::try_from(atom_range.end) else {
        return false;
    };
    atom_start <= sem_range.end_line && sem_range.start_line <= atom_end
}

fn normalize_sem_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .to_ascii_lowercase()
}

fn sem_hunks_from_parsed(parsed: &ParsedDiffFile) -> Vec<SemHunk> {
    parsed
        .hunks
        .iter()
        .enumerate()
        .map(|(index, hunk)| SemHunk {
            hunk_id: Some(format!("{}:{}", parsed.path, index)),
            hunk_index: index,
            hunk_header: Some(hunk.header.clone()),
            old_range: line_range_for_hunk(hunk, false).and_then(sem_line_range_from_remiss),
            new_range: line_range_for_hunk(hunk, true).and_then(sem_line_range_from_remiss),
        })
        .collect()
}

fn file_status_from_github(change_type: &str) -> FileStatus {
    match change_type {
        "ADDED" => FileStatus::Added,
        "DELETED" => FileStatus::Deleted,
        "RENAMED" => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

fn atom_range_for_side(atom: &ChangeAtom, side: SemSide) -> Option<LineRange> {
    match side {
        SemSide::Before => atom.old_range,
        SemSide::After => atom.new_range,
    }
}

fn file_path_for_side(atom: &ChangeAtom, side: SemSide) -> String {
    match side {
        SemSide::Before => atom
            .previous_path
            .clone()
            .unwrap_or_else(|| atom.path.clone()),
        SemSide::After => atom.path.clone(),
    }
}

fn sem_line_range_from_remiss(range: LineRange) -> Option<SemLineRange> {
    Some(SemLineRange {
        start_line: usize::try_from(range.start).ok()?,
        end_line: usize::try_from(range.end).ok()?,
    })
}

fn line_range_for_hunk(hunk: &ParsedDiffHunk, right_side: bool) -> Option<LineRange> {
    let numbers = hunk
        .lines
        .iter()
        .filter_map(|line| {
            if right_side {
                line.right_line_number
            } else {
                line.left_line_number
            }
        })
        .collect::<Vec<_>>();
    Some(LineRange {
        start: numbers.iter().copied().min()?,
        end: numbers.iter().copied().max()?,
    })
}

fn unavailable_semantic_review(
    detail: &PullRequestDetail,
    warning: impl Into<String>,
) -> RemissSemanticReview {
    let options = SemEmbeddedOptions::default();
    let changes = Vec::<SemFileChange>::new();
    RemissSemanticReview {
        version: REMISS_SEMANTIC_REVIEW_VERSION.to_string(),
        sem_api_version: SEM_EMBEDDED_API_VERSION.to_string(),
        code_version_key: tour_code_version_key(detail),
        analysis: analyze_file_changes(&changes, &options),
        layers: generate_review_layers(&changes, &SemLayerGenerationOptions::default(), &options),
        layer_atom_mappings: Vec::new(),
        focus_summaries: Vec::new(),
        warnings: vec![warning.into()],
    }
}

fn semantic_review_matches_current(
    review: &RemissSemanticReview,
    detail: &PullRequestDetail,
) -> bool {
    review.version == REMISS_SEMANTIC_REVIEW_VERSION
        && review.sem_api_version == SEM_EMBEDDED_API_VERSION
        && review.code_version_key == tour_code_version_key(detail)
}

fn semantic_review_head_identity(
    detail: &PullRequestDetail,
    head_oid: Option<&str>,
) -> Option<String> {
    if crate::local_review::is_local_review_detail(detail) {
        return Some(detail.id.clone());
    }

    head_oid
        .or(detail.head_ref_oid.as_deref())
        .map(str::trim)
        .filter(|head| !head.is_empty())
        .map(str::to_string)
}

fn load_semantic_side_text(
    cache: &CacheStore,
    repository: &str,
    checkout_root: &Path,
    side: &StructuralDiffSideRequest,
    warnings: &mut Vec<String>,
) -> Option<String> {
    if !side.fetch {
        return None;
    }

    match local_documents::load_local_repository_file_content(
        cache,
        repository,
        checkout_root,
        &side.reference,
        &side.path,
        side.prefer_worktree,
    ) {
        Ok(document) if document.is_binary => {
            warnings.push(format!(
                "Semantic evidence is unavailable for binary file {}.",
                side.path
            ));
            None
        }
        Ok(document) => document.content,
        Err(error) => {
            warnings.push(format!(
                "Semantic evidence could not load {} at {}: {error}",
                side.path, side.reference
            ));
            None
        }
    }
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
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        cache::CacheStore,
        diff::{DiffLineKind, ParsedDiffLine},
        github::PullRequestDataCompleteness,
        stacks::model::{
            ChangeAtomSource, ChangeRole, LayerMetrics, LayerReviewStatus, ReviewStackLayer,
            StackWarning,
        },
    };

    #[test]
    fn remiss_adapter_builds_sem_file_change_with_hunks() {
        let file = PullRequestFile {
            path: "src/lib.rs".to_string(),
            additions: 1,
            deletions: 1,
            change_type: "MODIFIED".to_string(),
        };
        let parsed = ParsedDiffFile {
            path: "src/lib.rs".to_string(),
            previous_path: None,
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,1 +1,1 @@ fn demo".to_string(),
                lines: vec![
                    line(
                        DiffLineKind::Deletion,
                        Some(1),
                        None,
                        "fn demo() -> i32 { 1 }",
                    ),
                    line(
                        DiffLineKind::Addition,
                        None,
                        Some(1),
                        "fn demo() -> i32 { 2 }",
                    ),
                ],
            }],
        };
        let content = RemissSemFileContents {
            path: "src/lib.rs".to_string(),
            previous_path: None,
            before_content: Some("fn demo() -> i32 { 1 }\n".to_string()),
            after_content: Some("fn demo() -> i32 { 2 }\n".to_string()),
        };

        let change = sem_file_change_from_remiss(&file, Some(&parsed), &content);

        assert_eq!(change.file_path, "src/lib.rs");
        assert_eq!(change.hunks.len(), 1);
        assert_eq!(change.hunks[0].new_range.unwrap().start_line, 1);
    }

    #[test]
    fn sem_path_matching_accepts_windows_separators() {
        let atom = ChangeAtom {
            id: "atom-path".to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 0 },
            path: "src/lib.rs".to_string(),
            previous_path: None,
            role: ChangeRole::CoreLogic,
            semantic_kind: None,
            symbol_name: None,
            defined_symbols: Vec::new(),
            referenced_symbols: Vec::new(),
            old_range: Some(LineRange { start: 1, end: 1 }),
            new_range: Some(LineRange { start: 1, end: 1 }),
            hunk_headers: Vec::new(),
            hunk_indices: vec![0],
            additions: 1,
            deletions: 1,
            patch_hash: "hash".to_string(),
            risk_score: 1,
            review_thread_ids: Vec::new(),
            warnings: Vec::<StackWarning>::new(),
        };

        assert!(atom_matches_file_path(&atom, r"src\lib.rs"));
    }

    #[test]
    fn remiss_adapter_resolves_focus_for_atom() {
        let file = PullRequestFile {
            path: "src/lib.rs".to_string(),
            additions: 1,
            deletions: 1,
            change_type: "MODIFIED".to_string(),
        };
        let parsed = ParsedDiffFile {
            path: "src/lib.rs".to_string(),
            previous_path: None,
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,3 +1,3 @@ fn demo".to_string(),
                lines: vec![
                    line(
                        DiffLineKind::Context,
                        Some(1),
                        Some(1),
                        "fn demo() -> i32 {",
                    ),
                    line(DiffLineKind::Deletion, Some(2), None, "    1"),
                    line(DiffLineKind::Addition, None, Some(2), "    2"),
                    line(DiffLineKind::Context, Some(3), Some(3), "}"),
                ],
            }],
        };
        let content = RemissSemFileContents {
            path: "src/lib.rs".to_string(),
            previous_path: None,
            before_content: Some("fn demo() -> i32 {\n    1\n}\n".to_string()),
            after_content: Some("fn demo() -> i32 {\n    2\n}\n".to_string()),
        };
        let change = sem_file_change_from_remiss(&file, Some(&parsed), &content);
        let atom = ChangeAtom {
            id: "atom-demo".to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 0 },
            path: "src/lib.rs".to_string(),
            previous_path: None,
            role: ChangeRole::CoreLogic,
            semantic_kind: Some("logic".to_string()),
            symbol_name: Some("demo".to_string()),
            defined_symbols: vec!["demo".to_string()],
            referenced_symbols: Vec::new(),
            old_range: Some(LineRange { start: 1, end: 3 }),
            new_range: Some(LineRange { start: 1, end: 3 }),
            hunk_headers: vec!["@@ -1,3 +1,3 @@ fn demo".to_string()],
            hunk_indices: vec![0],
            additions: 1,
            deletions: 1,
            patch_hash: "hash".to_string(),
            risk_score: 1,
            review_thread_ids: Vec::new(),
            warnings: Vec::<StackWarning>::new(),
        };

        let focus =
            sem_focus_for_atom(&[change], &atom, &SemEmbeddedOptions::default()).expect("focus");

        assert_eq!(
            focus
                .target_entity
                .as_ref()
                .map(|entity| entity.entity.name.as_str()),
            Some("demo")
        );
    }

    #[test]
    fn maps_sem_layers_to_atoms_by_file_and_hunk() {
        let atom = ChangeAtom {
            id: "atom-1".to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 2 },
            path: "src/lib.rs".to_string(),
            previous_path: None,
            role: ChangeRole::CoreLogic,
            semantic_kind: None,
            symbol_name: None,
            defined_symbols: Vec::new(),
            referenced_symbols: Vec::new(),
            old_range: None,
            new_range: None,
            hunk_headers: Vec::new(),
            hunk_indices: vec![2],
            additions: 1,
            deletions: 0,
            patch_hash: "hash".to_string(),
            risk_score: 1,
            review_thread_ids: Vec::new(),
            warnings: Vec::<StackWarning>::new(),
        };
        let layers = SemReviewLayerPlan {
            api_version: "test".to_string(),
            cache_key: "key".to_string(),
            layers: vec![sem_core::embedded::SemReviewLayer {
                id: "layer-1".to_string(),
                index: 0,
                title: "Update demo".to_string(),
                summary: String::new(),
                rationale: String::new(),
                depends_on_layer_ids: Vec::new(),
                change_indices: vec![0],
                file_paths: vec!["src/lib.rs".to_string()],
                hunk_indices: vec![2],
                entity_names: vec!["demo".to_string()],
            }],
            manual_review_change_indices: Vec::new(),
            warnings: Vec::new(),
        };

        let mappings = map_sem_layers_to_atoms(&layers, &[atom]);

        assert_eq!(mappings[0].atom_ids, vec!["atom-1".to_string()]);
    }

    #[test]
    fn maps_sem_layers_to_atoms_by_sem_analysis_range() {
        let atom = ChangeAtom {
            id: "atom-range".to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 0 },
            path: "src/lib.rs".to_string(),
            previous_path: None,
            role: ChangeRole::CoreLogic,
            semantic_kind: None,
            symbol_name: Some("demo".to_string()),
            defined_symbols: vec!["demo".to_string()],
            referenced_symbols: Vec::new(),
            old_range: None,
            new_range: Some(LineRange { start: 12, end: 13 }),
            hunk_headers: Vec::new(),
            hunk_indices: vec![0],
            additions: 1,
            deletions: 0,
            patch_hash: "hash".to_string(),
            risk_score: 1,
            review_thread_ids: Vec::new(),
            warnings: Vec::<StackWarning>::new(),
        };
        let layers = SemReviewLayerPlan {
            api_version: "test".to_string(),
            cache_key: "key".to_string(),
            layers: vec![sem_core::embedded::SemReviewLayer {
                id: "layer-1".to_string(),
                index: 0,
                title: "Update demo".to_string(),
                summary: String::new(),
                rationale: String::new(),
                depends_on_layer_ids: Vec::new(),
                change_indices: vec![0],
                file_paths: vec!["src/lib.rs".to_string()],
                hunk_indices: vec![99],
                entity_names: vec!["demo".to_string()],
            }],
            manual_review_change_indices: Vec::new(),
            warnings: Vec::new(),
        };
        let semantic_change = serde_json::from_value(serde_json::json!({
            "id": "change-demo",
            "entityId": "src/lib.rs::demo",
            "changeType": "modified",
            "entityType": "function",
            "entityName": "demo",
            "entityLine": 12,
            "filePath": "src/lib.rs"
        }))
        .expect("semantic change");
        let analysis = SemDiffAnalysis {
            api_version: "test".to_string(),
            cache_key: "key".to_string(),
            summary: sem_core::embedded::SemDiffSummary {
                file_count: 1,
                added_count: 0,
                modified_count: 1,
                deleted_count: 0,
                moved_count: 0,
                renamed_count: 0,
                reordered_count: 0,
                orphan_count: 0,
            },
            changes: vec![SemEmbeddedChange {
                change: semantic_change,
                before_range: None,
                after_range: Some(sem_core::embedded::SemEntityRange {
                    file_path: "src/lib.rs".to_string(),
                    start_line: 10,
                    end_line: 20,
                }),
                hunk_overlaps: Vec::new(),
            }],
        };

        let mappings = map_sem_layers_to_atoms_with_analysis(&layers, Some(&analysis), &[atom]);

        assert_eq!(mappings[0].atom_ids, vec!["atom-range".to_string()]);
    }

    #[test]
    fn loads_semantic_contents_for_standard_file_statuses() {
        let root = git_repo("semantic-review-statuses");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/lib.rs"), "fn changed() -> i32 { 1 }\n").expect("lib");
        fs::write(root.join("src/deleted.rs"), "fn removed() {}\n").expect("deleted");
        fs::write(root.join("src/old.rs"), "fn renamed() -> i32 { 1 }\n").expect("old");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "base"]);
        let base = git_stdout(&root, &["rev-parse", "HEAD"]);

        fs::write(root.join("src/lib.rs"), "fn changed() -> i32 { 2 }\n").expect("lib");
        fs::write(root.join("src/new.rs"), "fn added() {}\n").expect("new");
        fs::remove_file(root.join("src/deleted.rs")).expect("remove");
        git(&root, &["mv", "src/old.rs", "src/renamed.rs"]);
        fs::write(root.join("src/renamed.rs"), "fn renamed() -> i32 { 2 }\n").expect("renamed");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "head"]);
        let head = git_stdout(&root, &["rev-parse", "HEAD"]);

        let detail = detail(
            "pr-1",
            Some(base),
            Some(head.clone()),
            vec![
                file("src/lib.rs", "MODIFIED"),
                file("src/new.rs", "ADDED"),
                file("src/deleted.rs", "DELETED"),
                file("src/renamed.rs", "RENAMED"),
            ],
            vec![
                parsed("src/lib.rs", None, false),
                parsed("src/new.rs", None, false),
                parsed("src/deleted.rs", None, false),
                parsed("src/renamed.rs", Some("src/old.rs"), false),
            ],
        );
        let cache = temp_cache("semantic-review-statuses");

        let loaded =
            load_semantic_file_contents(&cache, &detail, &detail.repository, &root, head.as_str());

        let by_path = loaded
            .contents
            .iter()
            .map(|content| (content.path.as_str(), content))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            by_path["src/lib.rs"].before_content.as_deref(),
            Some("fn changed() -> i32 { 1 }\n")
        );
        assert_eq!(
            by_path["src/lib.rs"].after_content.as_deref(),
            Some("fn changed() -> i32 { 2 }\n")
        );
        assert!(by_path["src/new.rs"].before_content.is_none());
        assert_eq!(
            by_path["src/new.rs"].after_content.as_deref(),
            Some("fn added() {}\n")
        );
        assert_eq!(
            by_path["src/deleted.rs"].before_content.as_deref(),
            Some("fn removed() {}\n")
        );
        assert!(by_path["src/deleted.rs"].after_content.is_none());
        assert_eq!(
            by_path["src/renamed.rs"].previous_path.as_deref(),
            Some("src/old.rs")
        );
        assert_eq!(
            by_path["src/renamed.rs"].before_content.as_deref(),
            Some("fn renamed() -> i32 { 1 }\n")
        );
        assert_eq!(
            by_path["src/renamed.rs"].after_content.as_deref(),
            Some("fn renamed() -> i32 { 2 }\n")
        );
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn loads_local_review_after_content_from_worktree() {
        let root = git_repo("semantic-review-local");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/lib.rs"), "fn local() -> i32 { 1 }\n").expect("lib");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "base"]);
        let head = git_stdout(&root, &["rev-parse", "HEAD"]);
        fs::write(root.join("src/lib.rs"), "fn local() -> i32 { 2 }\n").expect("worktree");
        let detail = detail(
            "local:acme/widgets:main",
            Some(head.clone()),
            Some(head.clone()),
            vec![file("src/lib.rs", "MODIFIED")],
            vec![parsed("src/lib.rs", None, false)],
        );
        let cache = temp_cache("semantic-review-local");

        let loaded =
            load_semantic_file_contents(&cache, &detail, &detail.repository, &root, head.as_str());

        assert_eq!(
            loaded.contents[0].before_content.as_deref(),
            Some("fn local() -> i32 { 1 }\n")
        );
        assert_eq!(
            loaded.contents[0].after_content.as_deref(),
            Some("fn local() -> i32 { 2 }\n")
        );
    }

    #[test]
    fn semantic_content_loading_records_binary_and_missing_warnings() {
        let root = git_repo("semantic-review-warnings");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/lib.rs"), "fn demo() {}\n").expect("lib");
        fs::write(root.join("src/bin.dat"), b"\0binary").expect("binary");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "base"]);
        let head = git_stdout(&root, &["rev-parse", "HEAD"]);
        let detail = detail(
            "pr-1",
            Some(head.clone()),
            Some(head.clone()),
            vec![
                file("src/bin.dat", "MODIFIED"),
                file("src/missing.rs", "MODIFIED"),
            ],
            vec![
                parsed("src/bin.dat", None, true),
                parsed("src/missing.rs", None, false),
            ],
        );
        let cache = temp_cache("semantic-review-warnings");

        let loaded =
            load_semantic_file_contents(&cache, &detail, &detail.repository, &root, head.as_str());

        assert!(loaded
            .warnings
            .iter()
            .any(|warning| warning.contains("binary file src/bin.dat")));
        assert!(loaded
            .warnings
            .iter()
            .any(|warning| warning.contains("src/missing.rs")));
    }

    #[test]
    fn semantic_review_cache_key_includes_versions_and_head_identity() {
        let pr_detail = detail(
            "pr-1",
            Some("base".to_string()),
            Some("head-a".to_string()),
            vec![file("src/lib.rs", "MODIFIED")],
            vec![parsed("src/lib.rs", None, false)],
        );

        let key_a = semantic_review_cache_key(&pr_detail, Some("head-a")).expect("key");
        let key_b = semantic_review_cache_key(&pr_detail, Some("head-b")).expect("key");
        assert_ne!(key_a, key_b);
        assert!(key_a.contains(REMISS_SEMANTIC_REVIEW_VERSION));
        assert!(key_a.contains(SEM_EMBEDDED_API_VERSION));
        assert!(key_a.contains(&tour_code_version_key(&pr_detail)));

        let local_detail = detail(
            "local:acme/widgets:dirty",
            Some("base".to_string()),
            Some("head-a".to_string()),
            vec![file("src/lib.rs", "MODIFIED")],
            vec![parsed("src/lib.rs", None, false)],
        );
        let local_key_a = semantic_review_cache_key(&local_detail, Some("head-a")).expect("key");
        let local_key_b = semantic_review_cache_key(&local_detail, Some("head-b")).expect("key");
        assert_eq!(local_key_a, local_key_b);
        assert!(local_key_a.contains("local:acme/widgets:dirty"));
    }

    fn line(
        kind: DiffLineKind,
        left_line_number: Option<i64>,
        right_line_number: Option<i64>,
        content: &str,
    ) -> ParsedDiffLine {
        ParsedDiffLine {
            kind,
            prefix: String::new(),
            left_line_number,
            right_line_number,
            content: content.to_string(),
        }
    }

    fn detail(
        id: &str,
        base_ref_oid: Option<String>,
        head_ref_oid: Option<String>,
        files: Vec<PullRequestFile>,
        parsed_diff: Vec<ParsedDiffFile>,
    ) -> PullRequestDetail {
        PullRequestDetail {
            id: id.to_string(),
            repository: "acme/widgets".to_string(),
            number: 42,
            title: "Change widgets".to_string(),
            body: String::new(),
            url: "https://github.com/acme/widgets/pull/42".to_string(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid,
            head_ref_oid,
            additions: 1,
            deletions: 1,
            changed_files: files.len() as i64,
            comments_count: 0,
            commits_count: 1,
            created_at: String::new(),
            updated_at: String::new(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            viewer_pending_review: None,
            files,
            raw_diff: String::new(),
            parsed_diff,
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }

    fn file(path: &str, change_type: &str) -> PullRequestFile {
        PullRequestFile {
            path: path.to_string(),
            additions: 1,
            deletions: 1,
            change_type: change_type.to_string(),
        }
    }

    fn parsed(path: &str, previous_path: Option<&str>, is_binary: bool) -> ParsedDiffFile {
        ParsedDiffFile {
            path: path.to_string(),
            previous_path: previous_path.map(str::to_string),
            is_binary,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,1 +1,1 @@".to_string(),
                lines: vec![line(DiffLineKind::Context, Some(1), Some(1), "")],
            }],
        }
    }

    fn temp_cache(name: &str) -> CacheStore {
        let path = unique_test_directory(name).join("cache.sqlite");
        CacheStore::new(path).expect("cache")
    }

    fn git_repo(name: &str) -> PathBuf {
        let root = unique_test_directory(name);
        fs::create_dir_all(&root).expect("repo dir");
        git(&root, &["init"]);
        root
    }

    fn unique_test_directory(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("remiss-{name}-{suffix}"));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=Remiss Test",
                "-c",
                "user.email=test@example.com",
            ])
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git command");
        assert!(output.status.success(), "git {:?} failed", args);
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }

    #[allow(dead_code)]
    fn _layer() -> ReviewStackLayer {
        ReviewStackLayer {
            id: "layer".to_string(),
            index: 0,
            title: String::new(),
            summary: String::new(),
            rationale: String::new(),
            pr: None,
            virtual_layer: None,
            base_oid: None,
            head_oid: None,
            atom_ids: Vec::new(),
            depends_on_layer_ids: Vec::new(),
            metrics: LayerMetrics::default(),
            status: LayerReviewStatus::NotReviewed,
            confidence: crate::stacks::model::Confidence::High,
            warnings: Vec::new(),
        }
    }
}
