use super::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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
fn repo_path_normalization_uses_git_style_separators() {
    assert_eq!(normalize_repo_path(r".\src\lib.rs"), "src/lib.rs");
}

#[test]
fn semantic_layer_context_falls_back_to_path_and_hunk_overlap() {
    let stack = stack();
    let mut summary = summarize_semantic_review(&semantic_review_for_stack(&stack));
    for layer in &mut summary.layers {
        layer.atom_ids.clear();
        layer.file_paths = vec![r"src\lib.rs".to_string()];
        layer.hunk_indices = vec![0];
    }

    let semantic_layers = collect_layer_context(
        &detail_with_deleted_symbol(),
        &stack.layers[0],
        &[&stack.atoms[0]],
        Path::new("/tmp"),
        Some(&summary),
        None,
        &mut Vec::new(),
    )
    .semantic_layers;

    assert!(!semantic_layers.is_empty());
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

    let error = merge_review_partner(response, &input, None).expect_err("unknown layer rejected");
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

    let partner =
        merge_review_partner(response, &input, Some("model".to_string())).expect("partner context");

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
                    detail: "Nearby rows use a compact icon and title before details.".to_string(),
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
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), now_ms()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create temp dir");
    path
}
