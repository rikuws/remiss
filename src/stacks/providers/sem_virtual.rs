use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::json;
use sha1::{Digest, Sha1};

use crate::{
    github::PullRequestDetail,
    semantic_review::{
        summarize_semantic_review, RemissSemanticLayerSummary, RemissSemanticReview,
    },
};

use super::super::{
    atoms::extract_change_atoms,
    dependencies::{build_atom_dependencies, DependencyKind},
    model::{
        stack_now_ms, ChangeAtom, ChangeAtomId, ChangeRole, Confidence, LayerMetrics,
        LayerReviewStatus, RepoContext, ReviewStack, ReviewStackLayer, StackDiscoveryError,
        StackKind, StackProviderMetadata, StackSource, StackWarning, VirtualLayerRef,
        VirtualStackSizing, STACK_GENERATOR_VERSION,
    },
    validation::{atom_is_substantive, atom_noise_kind, requires_manual_review},
};

use super::virtual_semantic;

const SEM_VIRTUAL_PROVIDER_VERSION: &str = "sem-virtual-stack-v1";

pub fn discover(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
    sizing: &VirtualStackSizing,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    let atoms = extract_change_atoms(selected_pr);
    if atoms.is_empty() {
        return virtual_semantic::discover(selected_pr, repo_context, sizing);
    }

    let Some(semantic_review) = repo_context.semantic_review.as_ref() else {
        return fallback_stack(
            selected_pr,
            repo_context,
            sizing,
            "Sem review evidence was unavailable; used Remiss semantic fallback.",
        );
    };

    let summary = summarize_semantic_review(semantic_review);
    if summary.layers.iter().all(|layer| layer.atom_ids.is_empty()) {
        return fallback_stack(
            selected_pr,
            repo_context,
            sizing,
            "Sem produced no atom mappings; used Remiss semantic fallback.",
        );
    }

    Ok(Some(build_stack_from_semantic_review(
        selected_pr,
        atoms,
        semantic_review,
        sizing,
    )?))
}

fn fallback_stack(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
    sizing: &VirtualStackSizing,
    warning: &str,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    let mut stack = virtual_semantic::discover(selected_pr, repo_context, sizing)?;
    if let Some(stack) = stack.as_mut() {
        stack
            .warnings
            .push(StackWarning::new("sem-fallback", warning));
        stack.provider = Some(StackProviderMetadata {
            provider: "sem_virtual_stack".to_string(),
            raw_payload: Some(json!({
                "strategy": "sem_unavailable_fallback",
                "warning": warning,
            })),
        });
    }
    Ok(stack)
}

fn build_stack_from_semantic_review(
    selected_pr: &PullRequestDetail,
    atoms: Vec<ChangeAtom>,
    semantic_review: &RemissSemanticReview,
    sizing: &VirtualStackSizing,
) -> Result<ReviewStack, StackDiscoveryError> {
    let summary = summarize_semantic_review(semantic_review);
    let atoms_by_id = atoms
        .iter()
        .map(|atom| (atom.id.clone(), atom))
        .collect::<BTreeMap<_, _>>();
    let known_ids = atoms_by_id.keys().cloned().collect::<BTreeSet<_>>();
    let mut assigned = BTreeSet::<ChangeAtomId>::new();
    let mut manual_atom_ids = BTreeSet::<ChangeAtomId>::new();
    let mut pending_noise = Vec::<ChangeAtomId>::new();
    let mut warnings = semantic_review
        .warnings
        .iter()
        .cloned()
        .map(|warning| StackWarning::new("sem-warning", warning))
        .collect::<Vec<_>>();

    for atom in &atoms {
        if requires_manual_review(atom) {
            manual_atom_ids.insert(atom.id.clone());
        }
    }

    let mut semantic_layers = summary.layers.clone();
    semantic_layers.sort_by(|left, right| {
        left.index
            .cmp(&right.index)
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut groups = Vec::<SemLayerGroup>::new();
    for layer in &semantic_layers {
        let mut group_ids = Vec::<ChangeAtomId>::new();
        let mut layer_noise = Vec::<ChangeAtomId>::new();
        let mut ignored = 0usize;

        for atom_id in &layer.atom_ids {
            let Some(atom) = atoms_by_id.get(atom_id.as_str()).copied() else {
                ignored += 1;
                continue;
            };
            if manual_atom_ids.contains(&atom.id) {
                continue;
            }
            if assigned.contains(&atom.id) || group_ids.iter().any(|id| id == &atom.id) {
                ignored += 1;
                continue;
            }
            if atom_is_substantive(atom) {
                group_ids.push(atom.id.clone());
            } else {
                layer_noise.push(atom.id.clone());
            }
        }

        if group_ids.is_empty() {
            pending_noise.extend(layer_noise);
            if ignored > 0 {
                warnings.push(StackWarning::new(
                    "sem-layer-unmapped-atoms",
                    format!(
                        "Sem layer '{}' referenced {ignored} duplicate or unavailable atom{}.",
                        layer.title,
                        if ignored == 1 { "" } else { "s" }
                    ),
                ));
            }
            continue;
        }

        group_ids.extend(layer_noise);
        dedup_atom_ids(&mut group_ids);
        for atom_id in &group_ids {
            assigned.insert(atom_id.clone());
        }
        groups.push(SemLayerGroup::from_sem_layer(layer, group_ids));
    }

    assign_unmapped_substantive_atoms(
        &atoms,
        &atoms_by_id,
        &mut groups,
        &mut assigned,
        &mut manual_atom_ids,
    );
    assign_noise_atoms(
        &atoms,
        &atoms_by_id,
        &mut groups,
        &mut assigned,
        &mut pending_noise,
        &mut manual_atom_ids,
    );

    for atom in &atoms {
        if !assigned.contains(&atom.id) && !manual_atom_ids.contains(&atom.id) {
            manual_atom_ids.insert(atom.id.clone());
        }
    }

    if groups.len() > sizing.target_max_layers {
        groups = merge_excess_groups(groups, sizing.target_max_layers);
    }
    groups = order_groups(groups, &atoms);

    let mut all_layer_atom_ids = groups
        .iter()
        .flat_map(|group| group.atom_ids.iter().cloned())
        .collect::<Vec<_>>();
    all_layer_atom_ids.extend(manual_atom_ids.iter().cloned());
    validate_exact_coverage(&known_ids, &all_layer_atom_ids)?;

    let stack_id = virtual_stack_id(selected_pr);
    let final_groups = groups.clone();
    let mut layers = groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let layer_atoms = atom_refs_for_ids(&group.atom_ids, &atoms_by_id);
            let role = dominant_role(&layer_atoms);
            let metrics = metrics_for_atoms(&layer_atoms);
            let layer_id = virtual_layer_id(&stack_id, index, role, &group.atom_ids);
            ReviewStackLayer {
                id: layer_id,
                index,
                title: clean_layer_text(&group.title, "Sem review layer", 90),
                summary: clean_layer_text(&group.summary, "Sem grouped these changes.", 260),
                rationale: clean_layer_text(
                    &group.rationale,
                    "Sem grouped these atoms from entity-level diff evidence.",
                    560,
                ),
                pr: None,
                virtual_layer: Some(VirtualLayerRef {
                    source: StackSource::VirtualSemantic,
                    role,
                    source_label: group.source_label.clone(),
                }),
                base_oid: selected_pr.base_ref_oid.clone(),
                head_oid: selected_pr.head_ref_oid.clone(),
                atom_ids: group.atom_ids.clone(),
                depends_on_layer_ids: Vec::new(),
                metrics,
                status: LayerReviewStatus::NotReviewed,
                confidence: group.confidence,
                warnings: layer_atoms
                    .iter()
                    .flat_map(|atom| atom.warnings.iter().cloned())
                    .collect(),
            }
        })
        .collect::<Vec<_>>();

    attach_layer_dependencies(&mut layers, &final_groups, &atoms);

    let manual_atom_ids = manual_atom_ids.into_iter().collect::<Vec<_>>();
    if !manual_atom_ids.is_empty() {
        let index = layers.len();
        let manual_atoms = atom_refs_for_ids(&manual_atom_ids, &atoms_by_id);
        let role = dominant_role(&manual_atoms);
        let metrics = metrics_for_atoms(&manual_atoms);
        let layer_id = virtual_layer_id(&stack_id, index, role, &manual_atom_ids);
        let depends_on_layer_ids = layers
            .last()
            .map(|layer| vec![layer.id.clone()])
            .unwrap_or_default();
        layers.push(ReviewStackLayer {
            id: layer_id,
            index,
            title: "Manual review / Sem limitations".to_string(),
            summary: format!(
                "{} atom{} need a direct pass because Sem could not safely place them in a review layer.",
                manual_atom_ids.len(),
                if manual_atom_ids.len() == 1 { "" } else { "s" }
            ),
            rationale:
                "Generated, binary, unsupported, or unassigned atoms stay visible as an explicit final layer instead of being hidden inside a catch-all group."
                    .to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualSemantic,
                role,
                source_label: "sem-manual-review".to_string(),
            }),
            base_oid: selected_pr.base_ref_oid.clone(),
            head_oid: selected_pr.head_ref_oid.clone(),
            atom_ids: manual_atom_ids.clone(),
            depends_on_layer_ids,
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Low,
            warnings: manual_atoms
                .iter()
                .flat_map(|atom| atom.warnings.iter().cloned())
                .chain(std::iter::once(StackWarning::new(
                    "sem-manual-review",
                    "Sem left these atoms for manual review.",
                )))
                .collect(),
        });
    }

    warnings.extend(
        atoms
            .iter()
            .filter(|atom| requires_manual_review(atom))
            .flat_map(|atom| atom.warnings.iter().cloned()),
    );
    let confidence = stack_confidence(&layers, &warnings);

    Ok(ReviewStack {
        id: stack_id,
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::VirtualSemantic,
        kind: StackKind::Virtual,
        confidence,
        trunk_branch: Some(selected_pr.base_ref_name.clone()),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers,
        atoms,
        warnings,
        provider: Some(StackProviderMetadata {
            provider: "sem_virtual_stack".to_string(),
            raw_payload: Some(json!({
                "strategy": "sem_deterministic_stack",
                "providerVersion": SEM_VIRTUAL_PROVIDER_VERSION,
                "semApiVersion": summary.sem_api_version,
                "analysisCacheKey": summary.analysis_cache_key,
                "layerCacheKey": summary.layer_cache_key,
                "focusSummaryCount": summary.focus_summaries.len(),
                "semanticLayerCount": summary.layer_count,
            })),
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    })
}

#[derive(Clone)]
struct SemLayerGroup {
    sem_layer_id: Option<String>,
    sem_depends_on_layer_ids: Vec<String>,
    source_index: usize,
    title: String,
    summary: String,
    rationale: String,
    atom_ids: Vec<ChangeAtomId>,
    source_label: String,
    confidence: Confidence,
}

impl SemLayerGroup {
    fn from_sem_layer(layer: &RemissSemanticLayerSummary, atom_ids: Vec<ChangeAtomId>) -> Self {
        Self {
            sem_layer_id: Some(layer.id.clone()),
            sem_depends_on_layer_ids: layer.depends_on_layer_ids.clone(),
            source_index: layer.index,
            title: layer.title.clone(),
            summary: layer.summary.clone(),
            rationale: layer.rationale.clone(),
            atom_ids,
            source_label: "sem-layer".to_string(),
            confidence: Confidence::High,
        }
    }
}

fn assign_unmapped_substantive_atoms(
    atoms: &[ChangeAtom],
    atoms_by_id: &BTreeMap<ChangeAtomId, &ChangeAtom>,
    groups: &mut Vec<SemLayerGroup>,
    assigned: &mut BTreeSet<ChangeAtomId>,
    manual_atom_ids: &mut BTreeSet<ChangeAtomId>,
) {
    let mut fallback_groups = BTreeMap::<(ChangeRole, String), Vec<ChangeAtomId>>::new();
    for atom in atoms {
        if assigned.contains(&atom.id) || manual_atom_ids.contains(&atom.id) {
            continue;
        }
        if requires_manual_review(atom) {
            manual_atom_ids.insert(atom.id.clone());
            continue;
        }
        if !atom_is_substantive(atom) {
            continue;
        }
        if let Some(group_index) = best_group_for_atom(atom, groups, atoms_by_id)
            .filter(|(_, score)| *score >= 55)
            .map(|(index, _)| index)
        {
            groups[group_index].atom_ids.push(atom.id.clone());
            assigned.insert(atom.id.clone());
            continue;
        }

        fallback_groups
            .entry((atom.role, directory_label(&atom.path)))
            .or_default()
            .push(atom.id.clone());
        assigned.insert(atom.id.clone());
    }

    for ((role, directory), atom_ids) in fallback_groups {
        groups.push(SemLayerGroup {
            sem_layer_id: None,
            sem_depends_on_layer_ids: Vec::new(),
            source_index: usize::MAX.saturating_sub(role.order()),
            title: fallback_title(role, &directory),
            summary: format!(
                "{} across {}. Sem did not assign these atoms to a specific semantic layer, so Remiss kept them visible as a coverage repair group.",
                role.label(),
                directory
            ),
            rationale:
                "This repair group preserves exact atom coverage without asking the LLM to invent stack structure."
                    .to_string(),
            atom_ids,
            source_label: "sem-coverage-repair".to_string(),
            confidence: Confidence::Medium,
        });
    }
}

fn assign_noise_atoms(
    atoms: &[ChangeAtom],
    atoms_by_id: &BTreeMap<ChangeAtomId, &ChangeAtom>,
    groups: &mut [SemLayerGroup],
    assigned: &mut BTreeSet<ChangeAtomId>,
    pending_noise: &mut Vec<ChangeAtomId>,
    manual_atom_ids: &mut BTreeSet<ChangeAtomId>,
) {
    for atom in atoms {
        if assigned.contains(&atom.id) || manual_atom_ids.contains(&atom.id) {
            continue;
        }
        if !atom_is_substantive(atom) && atom_noise_kind(atom).is_some() {
            pending_noise.push(atom.id.clone());
        }
    }
    dedup_atom_ids(pending_noise);

    for atom_id in pending_noise.drain(..) {
        let Some(atom) = atoms_by_id.get(atom_id.as_str()).copied() else {
            continue;
        };
        if let Some(group_index) = best_group_for_atom(atom, groups, atoms_by_id)
            .filter(|(_, score)| *score > 0)
            .map(|(index, _)| index)
        {
            groups[group_index].atom_ids.push(atom.id.clone());
            assigned.insert(atom.id.clone());
        } else {
            manual_atom_ids.insert(atom.id.clone());
        }
    }

    for group in groups {
        dedup_atom_ids(&mut group.atom_ids);
    }
}

fn best_group_for_atom(
    atom: &ChangeAtom,
    groups: &[SemLayerGroup],
    atoms_by_id: &BTreeMap<ChangeAtomId, &ChangeAtom>,
) -> Option<(usize, i64)> {
    groups
        .iter()
        .enumerate()
        .filter_map(|(index, group)| {
            let score = group
                .atom_ids
                .iter()
                .filter_map(|atom_id| atoms_by_id.get(atom_id.as_str()).copied())
                .map(|candidate| atom_group_score(atom, candidate))
                .max()
                .unwrap_or(0);
            (score > 0).then_some((index, score))
        })
        .max_by(|(left_index, left_score), (right_index, right_score)| {
            left_score
                .cmp(right_score)
                .then_with(|| right_index.cmp(left_index))
        })
}

fn atom_group_score(atom: &ChangeAtom, candidate: &ChangeAtom) -> i64 {
    let mut score = 0i64;
    if atom.path == candidate.path {
        score += 50;
        if hunk_indices_overlap(atom, candidate) {
            score += 50;
        }
    }
    if atom
        .previous_path
        .as_deref()
        .is_some_and(|previous| candidate.previous_path.as_deref() == Some(previous))
    {
        score += 25;
    }
    if normalized_module_stem(&atom.path) == normalized_module_stem(&candidate.path) {
        score += 20;
    }
    if symbols_overlap(atom, candidate) {
        score += 45;
    }
    if atom.role == candidate.role {
        score += 10;
    }
    if atom.role == ChangeRole::Tests
        && candidate.role != ChangeRole::Tests
        && (symbols_overlap(atom, candidate)
            || normalized_module_stem(&atom.path) == normalized_module_stem(&candidate.path))
    {
        score += 60;
    }
    score
}

fn order_groups(mut groups: Vec<SemLayerGroup>, atoms: &[ChangeAtom]) -> Vec<SemLayerGroup> {
    groups.sort_by(|left, right| {
        left.source_index
            .cmp(&right.source_index)
            .then_with(|| {
                dominant_role_for_ids(&left.atom_ids, atoms)
                    .order()
                    .cmp(&dominant_role_for_ids(&right.atom_ids, atoms).order())
            })
            .then_with(|| left.title.cmp(&right.title))
    });
    let original_order = groups
        .iter()
        .enumerate()
        .map(|(index, group)| (group_key(group, index), index))
        .collect::<BTreeMap<_, _>>();
    let atom_to_group = atom_to_group_map(&groups);
    let mut incoming = vec![0usize; groups.len()];
    let mut outgoing = vec![BTreeSet::<usize>::new(); groups.len()];

    for dependency in build_atom_dependencies(atoms) {
        if dependency.kind == DependencyKind::PathLocality {
            continue;
        }
        let Some(from) = atom_to_group.get(&dependency.from_atom_id).copied() else {
            continue;
        };
        let Some(to) = atom_to_group.get(&dependency.to_atom_id).copied() else {
            continue;
        };
        if from != to && outgoing[from].insert(to) {
            incoming[to] += 1;
        }
    }

    for (index, group) in groups.iter().enumerate() {
        for dep_id in &group.sem_depends_on_layer_ids {
            let Some(from) = groups
                .iter()
                .position(|candidate| candidate.sem_layer_id.as_deref() == Some(dep_id.as_str()))
            else {
                continue;
            };
            if from != index && outgoing[from].insert(index) {
                incoming[index] += 1;
            }
        }
    }

    let mut ready = incoming
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<VecDeque<_>>();
    let mut emitted = BTreeSet::<usize>::new();
    let mut ordered = Vec::new();
    while let Some(index) = ready.pop_front() {
        if !emitted.insert(index) {
            continue;
        }
        ordered.push(groups[index].clone());
        for next in outgoing[index].clone() {
            incoming[next] = incoming[next].saturating_sub(1);
            if incoming[next] == 0 {
                ready.push_back(next);
            }
        }
    }

    if ordered.len() < groups.len() {
        let mut remaining = groups
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !emitted.contains(index))
            .collect::<Vec<_>>();
        remaining.sort_by_key(|(index, group)| {
            original_order
                .get(&group_key(group, *index))
                .copied()
                .unwrap_or(*index)
        });
        ordered.extend(remaining.into_iter().map(|(_, group)| group));
    }

    ordered
}

fn attach_layer_dependencies(
    layers: &mut [ReviewStackLayer],
    groups: &[SemLayerGroup],
    atoms: &[ChangeAtom],
) {
    let atom_to_group = atom_to_group_map(groups);
    let mut deps_by_group = vec![BTreeSet::<usize>::new(); groups.len()];
    for dependency in build_atom_dependencies(atoms) {
        if dependency.kind == DependencyKind::PathLocality {
            continue;
        }
        let Some(from) = atom_to_group.get(&dependency.from_atom_id).copied() else {
            continue;
        };
        let Some(to) = atom_to_group.get(&dependency.to_atom_id).copied() else {
            continue;
        };
        if from < to {
            deps_by_group[to].insert(from);
        }
    }

    for index in 0..layers.len() {
        let mut deps = deps_by_group
            .get(index)
            .into_iter()
            .flat_map(|deps| deps.iter())
            .filter_map(|dep_index| layers.get(*dep_index).map(|layer| layer.id.clone()))
            .collect::<Vec<_>>();
        if deps.is_empty() && index > 0 {
            let role = layers[index]
                .virtual_layer
                .as_ref()
                .map(|layer| layer.role)
                .unwrap_or(ChangeRole::Unknown);
            if !matches!(role, ChangeRole::Foundation | ChangeRole::Config) {
                deps.push(layers[index - 1].id.clone());
            }
        }
        layers[index].depends_on_layer_ids = deps;
    }
}

fn merge_excess_groups(
    mut groups: Vec<SemLayerGroup>,
    target_max_layers: usize,
) -> Vec<SemLayerGroup> {
    if target_max_layers == 0 {
        return groups;
    }
    while groups.len() > target_max_layers {
        let Some(last) = groups.pop() else {
            break;
        };
        if let Some(previous) = groups.last_mut() {
            previous.atom_ids.extend(last.atom_ids);
            previous.title = format!("{} + {}", previous.title, last.title);
            previous.summary = format!("{} {}", previous.summary, last.summary);
            previous.rationale =
                "Merged Sem groups to stay within the configured Guided Review layer budget."
                    .to_string();
            previous.confidence = previous.confidence.min(last.confidence);
            dedup_atom_ids(&mut previous.atom_ids);
        } else {
            groups.push(last);
            break;
        }
    }
    groups
}

fn validate_exact_coverage(
    known_ids: &BTreeSet<ChangeAtomId>,
    assigned_ids: &[ChangeAtomId],
) -> Result<(), StackDiscoveryError> {
    let mut seen = BTreeSet::<ChangeAtomId>::new();
    for atom_id in assigned_ids {
        if !known_ids.contains(atom_id) {
            return Err(StackDiscoveryError::new(format!(
                "Sem stack referenced unknown atom id '{atom_id}'."
            )));
        }
        if !seen.insert(atom_id.clone()) {
            return Err(StackDiscoveryError::new(format!(
                "Sem stack assigned atom '{atom_id}' more than once."
            )));
        }
    }
    let missing = known_ids.difference(&seen).collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(StackDiscoveryError::new(format!(
            "Sem stack omitted {} atom{}.",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

fn stack_confidence(layers: &[ReviewStackLayer], warnings: &[StackWarning]) -> Confidence {
    if layers
        .iter()
        .any(|layer| layer.confidence == Confidence::Low)
    {
        Confidence::Low
    } else if warnings.is_empty()
        && layers
            .iter()
            .all(|layer| layer.confidence == Confidence::High)
    {
        Confidence::High
    } else {
        Confidence::Medium
    }
}

fn atom_to_group_map(groups: &[SemLayerGroup]) -> BTreeMap<ChangeAtomId, usize> {
    groups
        .iter()
        .enumerate()
        .flat_map(|(index, group)| {
            group
                .atom_ids
                .iter()
                .cloned()
                .map(move |atom_id| (atom_id, index))
        })
        .collect()
}

fn atom_refs_for_ids<'a>(
    atom_ids: &[ChangeAtomId],
    atoms_by_id: &'a BTreeMap<ChangeAtomId, &'a ChangeAtom>,
) -> Vec<&'a ChangeAtom> {
    atom_ids
        .iter()
        .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
        .collect()
}

fn metrics_for_atoms(atoms: &[&ChangeAtom]) -> LayerMetrics {
    let file_count = atoms
        .iter()
        .map(|atom| atom.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    LayerMetrics {
        file_count,
        atom_count: atoms.len(),
        additions: atoms.iter().map(|atom| atom.additions).sum(),
        deletions: atoms.iter().map(|atom| atom.deletions).sum(),
        changed_lines: atoms
            .iter()
            .map(|atom| atom.additions + atom.deletions)
            .sum(),
        unresolved_thread_count: atoms.iter().map(|atom| atom.review_thread_ids.len()).sum(),
        risk_score: atoms.iter().map(|atom| atom.risk_score).sum(),
    }
}

fn dominant_role(atoms: &[&ChangeAtom]) -> ChangeRole {
    let mut counts = BTreeMap::<ChangeRole, usize>::new();
    for atom in atoms {
        *counts.entry(atom.role).or_default() += atom.additions + atom.deletions + 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(role, _)| role)
        .unwrap_or(ChangeRole::Unknown)
}

fn dominant_role_for_ids(atom_ids: &[ChangeAtomId], atoms: &[ChangeAtom]) -> ChangeRole {
    let atoms_by_id = atoms
        .iter()
        .map(|atom| (atom.id.as_str(), atom))
        .collect::<BTreeMap<_, _>>();
    dominant_role(
        &atom_ids
            .iter()
            .filter_map(|atom_id| atoms_by_id.get(atom_id.as_str()).copied())
            .collect::<Vec<_>>(),
    )
}

fn fallback_title(role: ChangeRole, directory: &str) -> String {
    match role {
        ChangeRole::Foundation | ChangeRole::Config => format!("Foundation: {directory}"),
        ChangeRole::CoreLogic => format!("Core behavior: {directory}"),
        ChangeRole::Integration => format!("Integration: {directory}"),
        ChangeRole::Presentation => format!("Presentation: {directory}"),
        ChangeRole::Tests => format!("Validation: {directory}"),
        ChangeRole::Docs => format!("Docs: {directory}"),
        ChangeRole::Generated => format!("Generated review: {directory}"),
        ChangeRole::Unknown => format!("Unclassified review: {directory}"),
    }
}

fn directory_label(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .filter(|dir| !dir.is_empty())
        .unwrap_or_else(|| "root".to_string())
}

fn normalized_module_stem(path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    file_name
        .split('.')
        .next()
        .unwrap_or(file_name)
        .trim_end_matches("_test")
        .trim_end_matches("_tests")
        .trim_end_matches("_spec")
        .trim_end_matches("_specs")
        .trim_end_matches(".test")
        .trim_end_matches(".spec")
        .to_ascii_lowercase()
}

fn symbols_overlap(left: &ChangeAtom, right: &ChangeAtom) -> bool {
    let left_symbols = atom_symbols(left);
    if left_symbols.is_empty() {
        return false;
    }
    atom_symbols(right)
        .into_iter()
        .any(|symbol| left_symbols.contains(&symbol))
}

fn atom_symbols(atom: &ChangeAtom) -> BTreeSet<String> {
    atom.symbol_name
        .iter()
        .chain(atom.defined_symbols.iter())
        .chain(atom.referenced_symbols.iter())
        .map(|symbol| normalize_symbol(symbol))
        .filter(|symbol| !symbol.is_empty())
        .collect()
}

fn normalize_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .to_ascii_lowercase()
}

fn hunk_indices_overlap(left: &ChangeAtom, right: &ChangeAtom) -> bool {
    !left.hunk_indices.is_empty()
        && !right.hunk_indices.is_empty()
        && left
            .hunk_indices
            .iter()
            .any(|index| right.hunk_indices.contains(index))
}

fn dedup_atom_ids(atom_ids: &mut Vec<ChangeAtomId>) {
    let mut seen = BTreeSet::<ChangeAtomId>::new();
    atom_ids.retain(|atom_id| seen.insert(atom_id.clone()));
}

fn clean_layer_text(value: &str, fallback: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let truncated = trimmed
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    format!("{}...", truncated.trim_end())
}

fn virtual_stack_id(selected_pr: &PullRequestDetail) -> String {
    let mut hasher = Sha1::new();
    let head_identity = if crate::local_review::is_local_review_detail(selected_pr) {
        selected_pr.id.as_str()
    } else {
        selected_pr.head_ref_oid.as_deref().unwrap_or_default()
    };
    for part in [
        selected_pr.repository.as_str(),
        &selected_pr.number.to_string(),
        selected_pr.base_ref_oid.as_deref().unwrap_or_default(),
        head_identity,
        StackSource::VirtualSemantic.label(),
        SEM_VIRTUAL_PROVIDER_VERSION,
        STACK_GENERATOR_VERSION,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("stack-{:x}", hasher.finalize())
}

fn virtual_layer_id(
    stack_id: &str,
    index: usize,
    role: ChangeRole,
    atom_ids: &[ChangeAtomId],
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(stack_id.as_bytes());
    hasher.update(index.to_string().as_bytes());
    hasher.update(role.label().as_bytes());
    for atom_id in atom_ids {
        hasher.update(atom_id.as_bytes());
    }
    format!("sem-virtual-layer-{}-{:x}", index, hasher.finalize())
}

fn group_key(group: &SemLayerGroup, index: usize) -> String {
    format!(
        "{}:{}:{}",
        group.sem_layer_id.as_deref().unwrap_or("repair"),
        group.title,
        index
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        github::{PullRequestDataCompleteness, PullRequestFile},
        semantic_review::RemissSemLayerAtomMapping,
        stacks::model::{ChangeAtomSource, LineRange},
    };
    use sem_core::embedded::{
        analyze_file_changes, SemEmbeddedOptions, SemFileChange, SemReviewLayer, SemReviewLayerPlan,
    };

    #[test]
    fn sem_mappings_are_the_primary_stack_plan() {
        let atoms = vec![
            atom(
                "core",
                "src/service.rs",
                ChangeRole::CoreLogic,
                Some("function"),
                80,
            ),
            atom(
                "ui",
                "src/view.rs",
                ChangeRole::Presentation,
                Some("function"),
                40,
            ),
            manual_atom("generated", "dist/generated.js", 900),
        ];
        let review = semantic_review(
            vec![
                sem_layer(
                    "sem-core",
                    0,
                    "Service behavior",
                    vec!["src/service.rs"],
                    vec![0],
                ),
                sem_layer(
                    "sem-ui",
                    1,
                    "Render service state",
                    vec!["src/view.rs"],
                    vec![0],
                ),
            ],
            vec![
                mapping("sem-core", vec!["core"], vec!["src/service.rs"]),
                mapping("sem-ui", vec!["ui"], vec!["src/view.rs"]),
            ],
        );

        let stack = build_stack_from_semantic_review(
            &detail(),
            atoms,
            &review,
            &VirtualStackSizing::default(),
        )
        .expect("sem stack");

        assert_eq!(stack.source, StackSource::VirtualSemantic);
        assert_eq!(
            stack.provider.as_ref().unwrap().provider,
            "sem_virtual_stack"
        );
        assert_eq!(stack.layers[0].title, "Service behavior");
        assert_eq!(stack.layers[0].atom_ids, vec!["core".to_string()]);
        assert_eq!(stack.layers[1].atom_ids, vec!["ui".to_string()]);
        assert_eq!(
            stack.layers.last().unwrap().title,
            "Manual review / Sem limitations"
        );
        assert_exact_stack_coverage(&stack);
    }

    #[test]
    fn sem_stack_repairs_unmapped_related_atoms_without_ai_planning() {
        let mut core = atom(
            "core",
            "src/service.rs",
            ChangeRole::CoreLogic,
            Some("function"),
            80,
        );
        core.defined_symbols = vec!["load_user".to_string()];
        let mut imports = atom(
            "imports",
            "src/service.rs",
            ChangeRole::CoreLogic,
            Some("imports"),
            2,
        );
        imports.referenced_symbols = vec!["load_user".to_string()];
        let mut tests = atom(
            "tests",
            "tests/service_tests.rs",
            ChangeRole::Tests,
            Some("function"),
            20,
        );
        tests.referenced_symbols = vec!["load_user".to_string()];
        let atoms = vec![core, imports, tests];
        let review = semantic_review(
            vec![sem_layer(
                "sem-core",
                0,
                "Service behavior",
                vec!["src/service.rs"],
                vec![0],
            )],
            vec![mapping("sem-core", vec!["core"], vec!["src/service.rs"])],
        );

        let stack = build_stack_from_semantic_review(
            &detail(),
            atoms,
            &review,
            &VirtualStackSizing::default(),
        )
        .expect("sem stack");

        assert_eq!(stack.layers.len(), 1);
        assert_eq!(
            stack.layers[0]
                .atom_ids
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            ["core", "imports", "tests"]
                .into_iter()
                .map(str::to_string)
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            stack
                .provider
                .as_ref()
                .unwrap()
                .raw_payload
                .as_ref()
                .unwrap()["strategy"],
            "sem_deterministic_stack"
        );
        assert_exact_stack_coverage(&stack);
    }

    fn assert_exact_stack_coverage(stack: &ReviewStack) {
        let known = stack
            .atoms
            .iter()
            .map(|atom| atom.id.clone())
            .collect::<BTreeSet<_>>();
        let assigned = stack
            .layers
            .iter()
            .flat_map(|layer| layer.atom_ids.iter().cloned())
            .collect::<Vec<_>>();
        assert_eq!(assigned.len(), known.len());
        assert_eq!(assigned.into_iter().collect::<BTreeSet<_>>(), known);
    }

    fn semantic_review(
        layers: Vec<SemReviewLayer>,
        layer_atom_mappings: Vec<RemissSemLayerAtomMapping>,
    ) -> RemissSemanticReview {
        let options = SemEmbeddedOptions::default();
        let changes = Vec::<SemFileChange>::new();
        RemissSemanticReview {
            version: crate::semantic_review::REMISS_SEMANTIC_REVIEW_VERSION.to_string(),
            sem_api_version: sem_core::embedded::SEM_EMBEDDED_API_VERSION.to_string(),
            code_version_key: "code".to_string(),
            analysis: analyze_file_changes(&changes, &options),
            layers: SemReviewLayerPlan {
                api_version: sem_core::embedded::SEM_EMBEDDED_API_VERSION.to_string(),
                cache_key: "layers".to_string(),
                layers,
                manual_review_change_indices: Vec::new(),
                warnings: Vec::new(),
            },
            layer_atom_mappings,
            focus_summaries: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn sem_layer(
        id: &str,
        index: usize,
        title: &str,
        file_paths: Vec<&str>,
        hunk_indices: Vec<usize>,
    ) -> SemReviewLayer {
        SemReviewLayer {
            id: id.to_string(),
            index,
            title: title.to_string(),
            summary: format!("{title} summary"),
            rationale: format!("{title} rationale"),
            depends_on_layer_ids: Vec::new(),
            change_indices: vec![index],
            file_paths: file_paths.into_iter().map(str::to_string).collect(),
            hunk_indices,
            entity_names: vec![title.to_string()],
        }
    }

    fn mapping(
        layer_id: &str,
        atom_ids: Vec<&str>,
        file_paths: Vec<&str>,
    ) -> RemissSemLayerAtomMapping {
        RemissSemLayerAtomMapping {
            layer_id: layer_id.to_string(),
            atom_ids: atom_ids.into_iter().map(str::to_string).collect(),
            file_paths: file_paths.into_iter().map(str::to_string).collect(),
            hunk_indices: vec![0],
            entity_names: Vec::new(),
        }
    }

    fn atom(
        id: &str,
        path: &str,
        role: ChangeRole,
        semantic_kind: Option<&str>,
        changed_lines: usize,
    ) -> ChangeAtom {
        ChangeAtom {
            id: id.to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 0 },
            path: path.to_string(),
            previous_path: None,
            role,
            semantic_kind: semantic_kind.map(str::to_string),
            symbol_name: Some(id.to_string()),
            defined_symbols: vec![id.to_string()],
            referenced_symbols: Vec::new(),
            old_range: Some(LineRange { start: 1, end: 2 }),
            new_range: Some(LineRange { start: 1, end: 3 }),
            hunk_headers: Vec::new(),
            hunk_indices: vec![0],
            additions: changed_lines,
            deletions: 0,
            patch_hash: format!("hash-{id}"),
            risk_score: changed_lines as i64,
            review_thread_ids: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn manual_atom(id: &str, path: &str, changed_lines: usize) -> ChangeAtom {
        let mut atom = atom(id, path, ChangeRole::Generated, None, changed_lines);
        atom.source = ChangeAtomSource::GeneratedPlaceholder;
        atom.warnings = vec![StackWarning::new("manual-review", "Generated file.")];
        atom
    }

    fn detail() -> PullRequestDetail {
        PullRequestDetail {
            id: "pr".to_string(),
            repository: "acme/repo".to_string(),
            number: 1,
            title: "PR".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: Some("base".to_string()),
            head_ref_oid: Some("head".to_string()),
            additions: 100,
            deletions: 0,
            changed_files: 2,
            comments_count: 0,
            commits_count: 1,
            created_at: String::new(),
            updated_at: "now".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: Default::default(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            viewer_pending_review: None,
            files: vec![PullRequestFile {
                path: "src/service.rs".to_string(),
                additions: 1,
                deletions: 0,
                change_type: "MODIFIED".to_string(),
            }],
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }
}
