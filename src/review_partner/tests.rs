use super::*;
use std::{collections::BTreeMap, path::PathBuf};

use crate::diff::{ParsedDiffHunk, ParsedDiffLine};
use crate::review_memory::{
    ReviewMemoryConfidence, ReviewMemoryKind, ReviewMemoryOrigin, ReviewMemoryReadingLevel,
    ReviewMemoryScope, ReviewMemorySignal, ReviewMemoryStatus,
};
use crate::semantic_review::{
    build_semantic_review_from_contents, RemissSemFileContents, RemissSemanticContextEntrySummary,
    RemissSemanticFocusSummary, RemissSemanticImpactSummary, RemissSemanticReviewSummary,
};
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
        ReviewAiProvider::Codex,
        "head-a",
        "stack-x",
        "context-y",
    );

    assert!(key.starts_with("review-partner-v24:"));
    assert!(key.contains("stack-x"));
    assert!(key.contains("context-y"));
}

#[test]
fn review_partner_prompt_requires_concrete_summary_copy() {
    let input = input(ReviewPartnerContextPack::empty());
    let prompt = prompt_for_input(&input);

    assert!(prompt.contains("The goal is explaining the scoped code"));
    assert!(prompt.contains("factual code explanation"));
    assert!(prompt.contains("never as a question"));
    assert!(prompt.contains("Never end a summary with an ellipsis"));
    assert!(prompt.contains("Match the supplied focus scope exactly"));
    assert!(prompt.contains("what changed, how the code behaves"));
    assert!(prompt.contains("Do not name changed files in the summary"));
    assert!(prompt.contains("Never write placeholder scaffolding"));
    assert!(prompt.contains("Never mention Sem"));
    assert!(prompt.contains("understanding checkpoints"));
    assert!(prompt.contains("understandingCheckpoints"));
    assert!(prompt.contains("historySignals"));
    assert!(prompt.contains("\"reviewMemory\": \"review-memory.json\""));
    assert!(prompt.contains("exact matching focusTargets[].key"));
    assert!(prompt.contains("human verification surface"));
    assert!(!prompt.contains("Act like a strong reviewer"));
    assert!(!prompt.contains("Request changes if"));
}

#[test]
fn review_partner_prompt_puts_focus_targets_before_large_context() {
    let input = input(ReviewPartnerContextPack::empty());
    let bundle = bundle_for_input(&input);
    let prompt = build_review_partner_prompt(&bundle);

    assert!(prompt.contains("Context manifest:"));
    assert!(prompt.contains("\"focusTargets\": \"focus-targets.json\""));
    assert!(prompt.contains("\"collectedContext\": \"collected-context.json\""));
    assert!(!prompt.contains("\"stack\": {"));
    assert!(bundle.workspace_root.join("focus-targets.json").is_file());
    assert!(bundle
        .workspace_root
        .join("collected-context.json")
        .is_file());
}

#[test]
fn review_partner_prompt_excludes_removed_generated_tour_schema() {
    let input = input(ReviewPartnerContextPack::empty());
    let prompt = prompt_for_input(&input);

    assert!(!prompt.contains("GeneratedGuidedReview"));
    assert!(!prompt.contains("Guided Review walkthrough"));
    assert!(!prompt.contains("reviewFocus"));
    assert!(!prompt.contains("candidateSteps"));
    assert!(!prompt.contains("sectionCategoryCatalog"));
    assert!(!prompt.contains("stepIds"));
}

#[test]
fn review_partner_prompt_budget_fails_without_tail_truncation() {
    let error = ensure_prompt_budget(
        "Review Partner context",
        MAX_REVIEW_PARTNER_PROMPT_BYTES + 1,
        MAX_REVIEW_PARTNER_PROMPT_BYTES,
    )
    .expect_err("over-budget prompts should fail explicitly");

    assert!(error.contains("exceeded the explicit budget"));
    assert!(error.contains("will not tail-truncate"));
    assert!(error.contains("deterministic context to files"));
}

#[test]
fn review_partner_bundle_writes_context_files_outside_checkout() {
    let checkout = unique_test_directory("review-partner-checkout");
    fs::write(checkout.join("sentinel.txt"), "checkout data").expect("sentinel");
    let checkout_entries_before = fs::read_dir(&checkout).expect("checkout read").count();
    let mut input = input(ReviewPartnerContextPack::empty());
    input.working_directory = checkout.to_string_lossy().to_string();

    let bundle = write_review_partner_context_bundle_at(
        &input,
        &unique_test_directory("review-partner-bundle-root"),
    )
    .expect("bundle");
    let manifest_text =
        fs::read_to_string(bundle.workspace_root.join("manifest.json")).expect("manifest");
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text).expect("manifest json");

    assert_eq!(
        manifest["bundleVersion"],
        REVIEW_PARTNER_CONTEXT_BUNDLE_VERSION
    );
    assert_eq!(manifest["files"]["stack"], "stack.json");
    assert_eq!(manifest["files"]["focusTargets"], "focus-targets.json");
    assert!(bundle.workspace_root.join("stack.json").is_file());
    assert!(bundle.workspace_root.join("focus-targets.json").is_file());
    assert!(bundle.workspace_root.join("layers").is_dir());
    assert!(bundle.workspace_root.join("checkout-path.txt").is_file());
    assert!(!checkout.join("manifest.json").exists());
    assert!(checkout.join("sentinel.txt").is_file());
    assert_eq!(
        fs::read_dir(&checkout).expect("checkout reread").count(),
        checkout_entries_before
    );
}

#[test]
fn review_partner_manifest_prompt_excludes_oversized_semantic_context() {
    let marker = "OVERSIZED_SEMANTIC_CONTEXT_MARKER";
    let large_context = marker.repeat(8_000);
    let semantic_focus = RemissSemanticFocusSummary {
        atom_id: "atom-1".to_string(),
        cache_key: "focus-cache".to_string(),
        target_entity: None,
        overlapping_entities: Vec::new(),
        matching_changes: Vec::new(),
        impact: Some(RemissSemanticImpactSummary {
            cache_key: "impact-cache".to_string(),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            impact: Vec::new(),
            tests: Vec::new(),
            context: vec![RemissSemanticContextEntrySummary {
                entity_name: "render_review".to_string(),
                entity_type: "function".to_string(),
                file_path: "src/lib.rs".to_string(),
                role: "context".to_string(),
                content: large_context,
                estimated_tokens: 32_000,
            }],
        }),
        warnings: Vec::new(),
    };
    let semantic_review = RemissSemanticReviewSummary {
        version: "semantic-review-test".to_string(),
        sem_api_version: "sem-test".to_string(),
        code_version_key: "head-a".to_string(),
        analysis_cache_key: "analysis-cache".to_string(),
        layer_cache_key: "layer-cache".to_string(),
        file_count: 1,
        added_count: 0,
        modified_count: 1,
        deleted_count: 0,
        moved_count: 0,
        renamed_count: 0,
        reordered_count: 0,
        orphan_count: 0,
        change_count: 1,
        layer_count: 1,
        layers: Vec::new(),
        focus_summaries: vec![semantic_focus.clone()],
        warnings: Vec::new(),
    };
    let mut input = input(ReviewPartnerContextPack {
        version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
        layers: vec![ReviewPartnerCollectedLayer {
            layer_id: "layer-1".to_string(),
            semantic_layers: Vec::new(),
            semantic_focus: vec![semantic_focus],
            changed_symbols: Vec::new(),
            removed_symbols: Vec::new(),
            similar_locations: Vec::new(),
            style_notes: Vec::new(),
            limitations: Vec::new(),
        }],
        warnings: Vec::new(),
    });
    input.semantic_review = Some(semantic_review);

    let bundle = bundle_for_input(&input);
    let prompt = build_review_partner_prompt(&bundle);

    assert!(prompt.len() < MAX_REVIEW_PARTNER_PROMPT_BYTES);
    assert!(prompt.contains("semantic-evidence.json"));
    assert!(!prompt.contains(marker));
    assert!(
        fs::read_to_string(bundle.workspace_root.join("semantic-evidence.json"))
            .expect("semantic evidence")
            .contains(marker)
    );
    assert!(
        fs::read_to_string(bundle.workspace_root.join("collected-context.json"))
            .expect("collected context")
            .contains(marker)
    );
}

#[test]
fn review_partner_agent_cwd_is_bundle_workspace_not_checkout() {
    let checkout = unique_test_directory("review-partner-agent-checkout");
    let mut input = input(ReviewPartnerContextPack::empty());
    input.working_directory = checkout.to_string_lossy().to_string();

    let bundle = bundle_for_input(&input);

    assert_eq!(
        bundle.agent_working_directory(),
        bundle.workspace_root.as_path()
    );
    assert_ne!(bundle.agent_working_directory(), checkout.as_path());
    assert_eq!(
        fs::read_to_string(bundle.workspace_root.join("checkout-path.txt")).expect("checkout path"),
        checkout.to_string_lossy()
    );
}

#[test]
fn review_partner_cache_requires_checkout_inspection_metadata() {
    let detail = detail_with_deleted_symbol();
    let mut document = partner_document(stack(), StructuralEvidencePack::empty());
    document.code_version_key = review_code_version_key(&detail);
    document.used_checkout_context = true;

    assert!(review_partner_document_matches_current(
        &document,
        &detail,
        ReviewAiProvider::Codex
    ));

    document.used_checkout_context = false;
    assert!(!review_partner_document_matches_current(
        &document,
        &detail,
        ReviewAiProvider::Codex
    ));
}

#[test]
fn fallback_focus_record_includes_review_memory_history_signal() {
    let mut input = input(ReviewPartnerContextPack::empty());
    input.review_memory = ReviewMemoryPromptContext {
        signals: vec![ReviewMemorySignal {
            id: "memory-1".to_string(),
            kind: ReviewMemoryKind::ReviewConcern,
            reading_level: ReviewMemoryReadingLevel::History,
            origin: ReviewMemoryOrigin::Deterministic,
            scope: ReviewMemoryScope::Symbol {
                path: "src/lib.rs".to_string(),
                name: "removed_helper".to_string(),
                kind: "function".to_string(),
            },
            statement: format!(
                "{} FINAL_HISTORY_MARKER",
                "Prior review asked whether removed_helper should stay explanation-only. "
                    .repeat(8)
            ),
            why_useful_next_time: Some(
                "Future reviewers should see the complete history note without premature clipping."
                    .to_string(),
            ),
            evidence_summary: "PR #12 review thread on src/lib.rs:1, resolved".to_string(),
            confidence: ReviewMemoryConfidence::High,
            status: ReviewMemoryStatus::Resolved,
            first_seen_pr: Some(12),
            last_seen_pr: Some(12),
            path: Some("src/lib.rs".to_string()),
            line: Some(1),
        }],
        limitations: vec!["Only locally cached review history was searched.".to_string()],
    };

    let target = input.focus_targets[0].clone();
    let record = fallback_focus_record(&input, &target, None);

    assert_eq!(record.history_signals.len(), 1);
    assert!(record.history_signals[0]
        .detail
        .contains("Evidence: PR #12"));
    assert!(record.history_signals[0]
        .detail
        .contains("FINAL_HISTORY_MARKER"));
    assert!(record.history_signals[0]
        .detail
        .contains("without premature clipping"));
}

#[test]
fn repo_path_normalization_uses_git_style_separators() {
    assert_eq!(normalize_repo_path(r".\src\lib.rs"), "src/lib.rs");
}

#[test]
fn review_partner_prompt_and_focus_records_include_semantic_context() {
    let stack = stack();
    let semantic_review = semantic_review_for_stack(&stack);
    let checkout_root = std::env::temp_dir();
    let checkout_root = checkout_root.to_string_lossy();
    let input = build_review_partner_generation_input(
        &detail_with_deleted_symbol(),
        ReviewAiProvider::Codex,
        checkout_root.as_ref(),
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

    let bundle = bundle_for_input(&input);
    let prompt = build_review_partner_prompt(&bundle);
    assert!(prompt.contains("semanticEvidence"));
    assert!(prompt.contains("semantic-evidence.json"));
    assert!(prompt.contains("collected-context.json"));
    assert!(bundle
        .workspace_root
        .join("semantic-evidence.json")
        .is_file());
    assert!(bundle
        .workspace_root
        .join("collected-context.json")
        .is_file());

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
    let layer_brief = partner
        .layer("layer-1")
        .map(|layer| layer.brief.clone())
        .unwrap_or_default();
    assert!(!layer_brief.contains("Sem"));
    assert!(!layer_brief.contains("The useful meaning"));
    assert!(!layer_brief.contains("src/lib.rs"));
    assert!(layer_brief.contains("removed_helper"));

    let focus_bundle = write_review_partner_focus_context_bundle_at(
        &partner,
        &partner.focus_targets[0],
        "/tmp",
        &unique_test_directory("review-partner-focus-bundle-root"),
    )
    .expect("focus bundle");
    let focus_prompt = build_focus_record_prompt(&focus_bundle, &partner.focus_targets[0]);
    assert!(focus_prompt.contains("semanticEvidence"));
    assert!(focus_prompt.contains("understandingCheckpoints"));
    assert!(focus_prompt.contains("historySignals"));
    assert!(focus_prompt.contains("exact supplied target.key"));
}

#[test]
fn partner_request_key_includes_generator_and_context_versions() {
    let detail = detail_with_deleted_symbol();
    let key = build_review_partner_request_key(&detail, ReviewAiProvider::Codex);

    assert!(key.contains(ReviewAiProvider::Codex.slug()));
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
    assert!(focus_record["properties"]
        .get("understandingCheckpoints")
        .is_some());
    assert!(focus_record["properties"].get("assumptions").is_some());
    assert!(focus_record["properties"].get("historySignals").is_some());
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
        ReviewAiProvider::Codex,
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

    let target = focus_target_for_diff_focus(&document, "src/lib.rs", Some(1), Some("RIGHT"), None);

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
        understanding_checkpoints: Vec::new(),
        assumptions: Vec::new(),
        history_signals: Vec::new(),
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
            similar_code: Vec::new(),
            codebase_fit: Vec::new(),
            concerns: Vec::new(),
            limitations: Vec::new(),
        }],
        focus_records: Vec::new(),
    };

    let error = merge_review_partner(response, &input, None).expect_err("unknown layer rejected");
    assert!(error.contains("unknown layer id"));
}

#[test]
fn merge_rejects_truncated_prompt_response_without_focus_records() {
    let input = input(ReviewPartnerContextPack::empty());
    let response = ReviewPartnerResponse {
        stack_brief: "brief".to_string(),
        stack_concerns: Vec::new(),
        limitations: Vec::new(),
        warnings: vec![
            "The supplied prompt was truncated before any focusTargets were visible.".to_string(),
        ],
        layers: Vec::new(),
        focus_records: Vec::new(),
    };

    let error = merge_review_partner(response, &input, None)
        .expect_err("truncated prompt without focus records should not be cached as success");

    assert!(error.contains("omitted focus records"));
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
            understanding_checkpoints: vec![ReviewPartnerItemResponse {
                title: "Invariant".to_string(),
                detail: "Keep the usage contract stable.".to_string(),
                path: None,
                line: None,
            }],
            assumptions: Vec::new(),
            history_signals: Vec::new(),
            limitations: Vec::new(),
        }],
    };

    let partner =
        merge_review_partner(response, &input, Some("model".to_string())).expect("partner context");

    assert_eq!(partner.layers[0].layer_id, "layer-1");
    assert_eq!(partner.layers[0].brief, "partner brief");
    assert_eq!(partner.layers[0].changed_items.len(), MAX_SECTION_ITEMS);
    assert_eq!(partner.focus_records.len(), 1);
    assert_eq!(partner.focus_records[0].understanding_checkpoints.len(), 1);
    assert_eq!(partner.model.as_deref(), Some("model"));
}

#[test]
fn merge_accepts_layer_id_focus_record_key_alias() {
    let input = input(ReviewPartnerContextPack::empty());
    assert_eq!(input.focus_targets[0].key, "layer:layer-1");

    let response = ReviewPartnerResponse {
        stack_brief: "brief".to_string(),
        stack_concerns: Vec::new(),
        limitations: Vec::new(),
        warnings: Vec::new(),
        layers: Vec::new(),
        focus_records: vec![ReviewPartnerFocusRecordResponse {
            key: "layer-1".to_string(),
            title: "Layer focus".to_string(),
            subtitle: None,
            summary: Some("Generated layer-level behavior explanation.".to_string()),
            codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                follows: true,
                summary: "follows codebase style".to_string(),
                evidence: Vec::new(),
            }),
            sections: Vec::new(),
            understanding_checkpoints: vec![ReviewPartnerItemResponse {
                title: "Invariant".to_string(),
                detail: "Keep the generated focus record attached to the layer target.".to_string(),
                path: None,
                line: None,
            }],
            assumptions: Vec::new(),
            history_signals: Vec::new(),
            limitations: Vec::new(),
        }],
    };

    let partner = merge_review_partner(response, &input, None).expect("partner context");
    let record = &partner.focus_records[0];

    assert_eq!(record.key, "layer:layer-1");
    assert_eq!(record.title, "Layer focus");
    assert_eq!(
        record.summary,
        "Generated layer-level behavior explanation."
    );
    assert_eq!(record.understanding_checkpoints.len(), 1);
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
            codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                follows: true,
                summary: "follows codebase style".to_string(),
                evidence: Vec::new(),
            }),
            sections: Vec::new(),
            understanding_checkpoints: Vec::new(),
            assumptions: Vec::new(),
            history_signals: Vec::new(),
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
            codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                follows: true,
                summary: "follows codebase style".to_string(),
                evidence: Vec::new(),
            }),
            sections: Vec::new(),
            understanding_checkpoints: Vec::new(),
            assumptions: Vec::new(),
            history_signals: Vec::new(),
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
            codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                follows: true,
                summary: "follows codebase style".to_string(),
                evidence: Vec::new(),
            }),
            sections: Vec::new(),
            understanding_checkpoints: Vec::new(),
            assumptions: Vec::new(),
            history_signals: Vec::new(),
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
            codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                follows: true,
                summary: "follows codebase style".to_string(),
                evidence: Vec::new(),
            }),
            sections: Vec::new(),
            understanding_checkpoints: Vec::new(),
            assumptions: Vec::new(),
            history_signals: Vec::new(),
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
fn file_inventory_focus_summary_uses_behavior_fallback() {
    let mut input = input(ReviewPartnerContextPack {
        version: REVIEW_PARTNER_CONTEXT_VERSION.to_string(),
        layers: vec![ReviewPartnerCollectedLayer {
            layer_id: "layer-1".to_string(),
            semantic_layers: Vec::new(),
            semantic_focus: Vec::new(),
            changed_symbols: vec![
                ReviewPartnerCollectedSymbol {
                    symbol: "ApiClient".to_string(),
                    path: "backend/common/src/main/kotlin/fi/fintraffic/common/integration/ApiClient.kt"
                        .to_string(),
                    line: Some(12),
                    atom_ids: vec!["atom-1".to_string()],
                    search_strategy: "test".to_string(),
                    reference_count: 0,
                    references: Vec::new(),
                },
                ReviewPartnerCollectedSymbol {
                    symbol: "CachedJwt".to_string(),
                    path: "backend/common/src/main/kotlin/fi/fintraffic/common/integration/CachedJwt.kt"
                        .to_string(),
                    line: Some(24),
                    atom_ids: vec!["atom-1".to_string()],
                    search_strategy: "test".to_string(),
                    reference_count: 0,
                    references: Vec::new(),
                },
            ],
            removed_symbols: Vec::new(),
            similar_locations: Vec::new(),
            style_notes: Vec::new(),
            limitations: Vec::new(),
        }],
        warnings: Vec::new(),
    });
    input.stack.layers[0].title = "Webview JWT signing flow".to_string();
    input.stack.layers[0].summary =
        "This layer covers ApiClient, CachedJwt, plus 7 others. The useful meaning is what state changes."
            .to_string();
    let fallback =
        fallback_focus_summary_from_stack(&input.focus_targets[0], &input.stack, &input.context);
    assert!(
        fallback.contains("Webview JWT signing flow"),
        "fallback was: {fallback}"
    );
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
            summary: Some("This layer covers ApiClient, ApiClientTest, CachedJwt, plus 7 others in backend/common/src/main/kotlin/fi/fintraffic/common/integration/ApiClient.kt, backend/common/src/test/kotlin/fi/fintraffic/common/integration/ApiClientTest.kt. The useful meaning is the behavior these symbols now express: what state changes.".to_string()),
            codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                follows: true,
                summary: "follows codebase style".to_string(),
                evidence: Vec::new(),
            }),
            sections: Vec::new(),
            understanding_checkpoints: Vec::new(),
            assumptions: Vec::new(),
            history_signals: Vec::new(),
            limitations: Vec::new(),
        }],
    };

    let partner = merge_review_partner(response, &input, None).expect("partner context");
    let summary = &partner.focus_records[0].summary;

    assert!(
        summary.contains("Webview JWT signing flow"),
        "summary was: {summary}"
    );
    assert!(summary.contains("ApiClient"), "summary was: {summary}");
    assert!(summary.contains("CachedJwt"), "summary was: {summary}");
    assert!(
        !summary.contains("backend/common"),
        "summary was: {summary}"
    );
    assert!(
        !summary.contains("useful meaning"),
        "summary was: {summary}"
    );
    assert!(!summary.contains("plus 7"), "summary was: {summary}");
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
            understanding_checkpoints: Vec::new(),
            assumptions: Vec::new(),
            history_signals: Vec::new(),
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
            codebase_fit: Some(ReviewPartnerCodebaseFitResponse {
                follows: false,
                summary: "This uses a different row structure than nearby panels.".to_string(),
                evidence: vec![ReviewPartnerItemResponse {
                    title: "Existing panel row".to_string(),
                    detail: "Nearby rows use a compact icon and title before details.".to_string(),
                    path: Some("src/panel.rs".to_string()),
                    line: Some(12),
                }],
            }),
            sections: Vec::new(),
            understanding_checkpoints: Vec::new(),
            assumptions: Vec::new(),
            history_signals: Vec::new(),
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

fn prompt_for_input(input: &GenerateReviewPartnerInput) -> String {
    let bundle = bundle_for_input(input);
    build_review_partner_prompt(&bundle)
}

fn bundle_for_input(input: &GenerateReviewPartnerInput) -> ReviewPartnerContextBundle {
    write_review_partner_context_bundle_at(
        input,
        &unique_test_directory("review-partner-bundle-root"),
    )
    .expect("bundle")
}

fn input(context: ReviewPartnerContextPack) -> GenerateReviewPartnerInput {
    let stack = stack();
    let structural_evidence = StructuralEvidencePack::empty();
    let focus_targets = build_review_partner_focus_targets(&stack, &structural_evidence);

    GenerateReviewPartnerInput {
        provider: ReviewAiProvider::Codex,
        working_directory: "/tmp".to_string(),
        repository: "acme/widgets".to_string(),
        number: 42,
        code_version_key: "head-a".to_string(),
        title: "Improve widgets".to_string(),
        body: String::new(),
        url: "https://github.com/acme/widgets/pull/42".to_string(),
        base_ref_name: "main".to_string(),
        head_ref_name: "feature".to_string(),
        comments: Vec::new(),
        latest_reviews: Vec::new(),
        review_threads: Vec::new(),
        stack,
        structural_evidence,
        semantic_review: None,
        review_memory: ReviewMemoryPromptContext::default(),
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
        provider: ReviewAiProvider::Codex,
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
        used_checkout_context: true,
        checkout_command_count: 1,
        inspected_path_hints: vec!["src/lib.rs".to_string()],
        prompt_bytes: 0,
        stack,
        structural_evidence,
        semantic_review: None,
        review_memory: ReviewMemoryPromptContext::default(),
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
            operations: Vec::new(),
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
        commits: Vec::new(),
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
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}-{sequence}",
        std::process::id(),
        now_ms()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create temp dir");
    path
}
