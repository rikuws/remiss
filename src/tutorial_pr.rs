use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::code_tour::{
    tour_code_version_key, CodeTourProvider, DiffAnchor, GeneratedCodeTour, TourSection,
    TourSectionCategory, TourSectionPriority, TourStep,
};
use crate::diff::parse_unified_diff;
use crate::github::{
    AuthState, PendingPullRequestReview, PullRequestDataCompleteness, PullRequestDetail,
    PullRequestDetailSnapshot, PullRequestFile, PullRequestReviewComment, PullRequestReviewThread,
    PullRequestSummary,
};
use crate::review_brief::{ReviewBrief, ReviewBriefConfidence};
use crate::review_guide::{GeneratedReviewGuide, ReviewGuideLayer, REVIEW_GUIDE_GENERATOR_VERSION};
use crate::review_session::{
    DiffLayout, ReviewCenterMode, ReviewGuideLens, ReviewLocation, ReviewSessionDocument,
    ReviewTaskRoute, ReviewWaymark,
};
use crate::stacks::model::{
    stack_now_ms, ChangeAtom, ChangeAtomSource, ChangeRole, Confidence, LayerMetrics,
    LayerReviewStatus, LineRange, ReviewStack, ReviewStackLayer, StackDiffMode,
    StackProviderMetadata, StackSource, StackWarning, VirtualLayerRef, STACK_GENERATOR_VERSION,
};
use crate::structural_evidence::{
    StructuralEvidencePack, StructuralEvidenceStatus, STRUCTURAL_EVIDENCE_VERSION,
};

pub const TUTORIAL_PR_KEY: &str = "tutorial:welcome-pr";
pub const TUTORIAL_REPOSITORY: &str = "remiss/tutorial";
pub const TUTORIAL_PR_NUMBER: i64 = 1;

const GENERATED_AT: i64 = 1_780_000_000_000;
const UPDATED_AT: &str = "2026-05-15T09:00:00Z";
const TUTORIAL_URL: &str = "remiss://tutorial/welcome-pr";

pub fn summary() -> PullRequestSummary {
    let detail = detail();
    PullRequestSummary {
        local_key: Some(TUTORIAL_PR_KEY.to_string()),
        repository: detail.repository.clone(),
        number: detail.number,
        title: detail.title.clone(),
        author_login: detail.author_login.clone(),
        author_avatar_url: None,
        is_draft: false,
        comments_count: detail.comments_count,
        additions: detail.additions,
        deletions: detail.deletions,
        changed_files: detail.changed_files,
        state: "TUTORIAL".to_string(),
        review_decision: None,
        updated_at: detail.updated_at.clone(),
        url: detail.url.clone(),
    }
}

pub fn snapshot() -> PullRequestDetailSnapshot {
    PullRequestDetailSnapshot {
        auth: AuthState {
            is_authenticated: true,
            active_login: Some("you".to_string()),
            active_hostname: Some("tutorial.local".to_string()),
            message: "Tutorial data is local-only.".to_string(),
        },
        loaded_from_cache: false,
        fetched_at_ms: Some(GENERATED_AT),
        detail: Some(detail()),
    }
}

pub fn detail() -> PullRequestDetail {
    let raw_diff = raw_diff().to_string();
    let parsed_diff = parse_unified_diff(&raw_diff);
    let files = vec![
        PullRequestFile {
            path: "src/review_toolbar.rs".to_string(),
            additions: 13,
            deletions: 3,
            change_type: "MODIFIED".to_string(),
        },
        PullRequestFile {
            path: "tests/review_toolbar_test.rs".to_string(),
            additions: 19,
            deletions: 0,
            change_type: "ADDED".to_string(),
        },
    ];

    PullRequestDetail {
        id: TUTORIAL_PR_KEY.to_string(),
        repository: TUTORIAL_REPOSITORY.to_string(),
        number: TUTORIAL_PR_NUMBER,
        title: "Tutorial: organize review feedback".to_string(),
        body: "A small synthetic change that demonstrates line comments, waypoints, submit review, and Guided Review layers.".to_string(),
        url: TUTORIAL_URL.to_string(),
        author_login: "remiss-guide".to_string(),
        author_avatar_url: None,
        state: "TUTORIAL".to_string(),
        is_draft: false,
        review_decision: None,
        base_ref_name: "main".to_string(),
        head_ref_name: "tutorial-feedback-flow".to_string(),
        base_ref_oid: Some("0000000000000000000000000000000000000000".to_string()),
        head_ref_oid: Some("1111111111111111111111111111111111111111".to_string()),
        additions: files.iter().map(|file| file.additions).sum(),
        deletions: files.iter().map(|file| file.deletions).sum(),
        changed_files: files.len() as i64,
        comments_count: 2,
        commits_count: 2,
        created_at: UPDATED_AT.to_string(),
        updated_at: UPDATED_AT.to_string(),
        labels: vec!["tutorial".to_string()],
        reviewers: vec!["you".to_string()],
        reviewer_avatar_urls: BTreeMap::new(),
        comments: Vec::new(),
        latest_reviews: Vec::new(),
        review_threads: review_threads(),
        viewer_pending_review: Some(pending_review()),
        files,
        raw_diff,
        parsed_diff,
        data_completeness: PullRequestDataCompleteness::default(),
    }
}

pub fn review_brief(detail: &PullRequestDetail) -> ReviewBrief {
    ReviewBrief {
        provider: CodeTourProvider::Codex,
        generated_at_ms: GENERATED_AT,
        code_version_key: tour_code_version_key(detail),
        confidence: ReviewBriefConfidence::High,
        brief_paragraph: "This tutorial PR adds a compact review feedback toolbar path and a regression test for preserving draft context.".to_string(),
        likely_intent: "Make review feedback easier to finish without losing line-level context.".to_string(),
        changed_summary: vec![
            "Adds waypoint counts and a finish-review action to the toolbar.".to_string(),
            "Covers the empty/pending feedback states in tests.".to_string(),
        ],
        risks_questions: vec![
            "Check that pending comments and waypoints stay visible while switching review modes."
                .to_string(),
        ],
        warnings: Vec::new(),
        related_file_paths: vec![
            "src/review_toolbar.rs".to_string(),
            "tests/review_toolbar_test.rs".to_string(),
        ],
        model: Some("tutorial-fixture".to_string()),
    }
}

pub fn generated_tour() -> GeneratedCodeTour {
    let first_anchor = DiffAnchor {
        file_path: "src/review_toolbar.rs".to_string(),
        hunk_header: Some("@@ -1,18 +1,28 @@".to_string()),
        line: Some(7),
        side: Some("RIGHT".to_string()),
        thread_id: None,
    };
    let second_anchor = DiffAnchor {
        file_path: "tests/review_toolbar_test.rs".to_string(),
        hunk_header: Some("@@ -0,0 +1,19 @@".to_string()),
        line: Some(9),
        side: Some("RIGHT".to_string()),
        thread_id: None,
    };

    GeneratedCodeTour {
        provider: CodeTourProvider::Codex,
        model: Some("tutorial-fixture".to_string()),
        generated_at: UPDATED_AT.to_string(),
        summary:
            "Follow the toolbar change first, then confirm the behavior with the focused test."
                .to_string(),
        review_focus: "Feedback organization and submit-readiness.".to_string(),
        open_questions: vec![
            "Should the finish action stay disabled when no review body or pending draft exists?"
                .to_string(),
        ],
        warnings: Vec::new(),
        sections: vec![TourSection {
            id: "feedback-flow".to_string(),
            title: "Feedback flow".to_string(),
            summary: "The toolbar now keeps waypoints and pending review state together."
                .to_string(),
            detail: "Review the UI state calculation before reading the test.".to_string(),
            badge: "Review".to_string(),
            category: TourSectionCategory::UiUx,
            priority: TourSectionPriority::High,
            step_ids: vec!["toolbar-state".to_string(), "toolbar-test".to_string()],
            review_points: vec![
                "Confirm the button state matches pending comments.".to_string(),
                "Check whether waypoint count changes should update eagerly.".to_string(),
            ],
            callsites: Vec::new(),
        }],
        steps: vec![
            TourStep {
                id: "toolbar-state".to_string(),
                kind: "diff".to_string(),
                title: "Toolbar state".to_string(),
                summary: "Adds waypoint count and finish-review enablement.".to_string(),
                detail: "Start here because this is the behavior users will touch directly."
                    .to_string(),
                file_path: Some(first_anchor.file_path.clone()),
                anchor: Some(first_anchor),
                additions: 13,
                deletions: 3,
                unresolved_thread_count: 1,
                snippet: Some("ReviewToolbarState { pending_comments, waypoints }".to_string()),
                badge: "UI".to_string(),
            },
            TourStep {
                id: "toolbar-test".to_string(),
                kind: "test".to_string(),
                title: "Regression test".to_string(),
                summary: "Covers pending drafts and waypoint display together.".to_string(),
                detail: "Use this test to confirm the UI state is intentional.".to_string(),
                file_path: Some(second_anchor.file_path.clone()),
                anchor: Some(second_anchor),
                additions: 19,
                deletions: 0,
                unresolved_thread_count: 0,
                snippet: Some("assert_eq!(state.finish_label, \"Submit review (1)\")".to_string()),
                badge: "Test".to_string(),
            },
        ],
    }
}

pub fn review_stack() -> ReviewStack {
    ReviewStack {
        id: "tutorial-guided-review".to_string(),
        repository: TUTORIAL_REPOSITORY.to_string(),
        selected_pr_number: TUTORIAL_PR_NUMBER,
        source: StackSource::VirtualAi,
        kind: crate::stacks::model::StackKind::Virtual,
        confidence: Confidence::High,
        trunk_branch: Some("main".to_string()),
        base_oid: Some("0000000000000000000000000000000000000000".to_string()),
        head_oid: Some("1111111111111111111111111111111111111111".to_string()),
        layers: vec![
            ReviewStackLayer {
                id: "tutorial-layer-feedback-ui".to_string(),
                index: 0,
                title: "Feedback toolbar state".to_string(),
                summary: "Review how pending comments and waypoints drive the toolbar.".to_string(),
                rationale:
                    "This is the user-facing behavior that makes the review pass feel organized."
                        .to_string(),
                pr: None,
                virtual_layer: Some(VirtualLayerRef {
                    source: StackSource::VirtualAi,
                    role: ChangeRole::Presentation,
                    source_label: "tutorial fixture".to_string(),
                }),
                base_oid: None,
                head_oid: None,
                atom_ids: vec!["tutorial-atom-toolbar".to_string()],
                depends_on_layer_ids: Vec::new(),
                metrics: LayerMetrics {
                    file_count: 1,
                    atom_count: 1,
                    additions: 13,
                    deletions: 3,
                    changed_lines: 16,
                    unresolved_thread_count: 1,
                    risk_score: 2,
                },
                status: LayerReviewStatus::NotReviewed,
                confidence: Confidence::High,
                warnings: Vec::new(),
            },
            ReviewStackLayer {
                id: "tutorial-layer-test-coverage".to_string(),
                index: 1,
                title: "Feedback regression coverage".to_string(),
                summary: "Check the focused test that locks the submit label and waypoint count."
                    .to_string(),
                rationale: "The test is the guardrail that future UI changes should keep passing."
                    .to_string(),
                pr: None,
                virtual_layer: Some(VirtualLayerRef {
                    source: StackSource::VirtualAi,
                    role: ChangeRole::Tests,
                    source_label: "tutorial fixture".to_string(),
                }),
                base_oid: None,
                head_oid: None,
                atom_ids: vec!["tutorial-atom-tests".to_string()],
                depends_on_layer_ids: vec!["tutorial-layer-feedback-ui".to_string()],
                metrics: LayerMetrics {
                    file_count: 1,
                    atom_count: 1,
                    additions: 19,
                    deletions: 0,
                    changed_lines: 19,
                    unresolved_thread_count: 0,
                    risk_score: 1,
                },
                status: LayerReviewStatus::NotReviewed,
                confidence: Confidence::High,
                warnings: Vec::new(),
            },
        ],
        atoms: vec![
            ChangeAtom {
                id: "tutorial-atom-toolbar".to_string(),
                source: ChangeAtomSource::Hunk { hunk_index: 0 },
                path: "src/review_toolbar.rs".to_string(),
                previous_path: None,
                role: ChangeRole::Presentation,
                semantic_kind: Some("ui-state".to_string()),
                symbol_name: Some("build_review_toolbar_state".to_string()),
                defined_symbols: vec!["ReviewToolbarState".to_string()],
                referenced_symbols: vec!["PendingReview".to_string()],
                old_range: Some(LineRange { start: 1, end: 18 }),
                new_range: Some(LineRange { start: 1, end: 28 }),
                hunk_headers: vec!["@@ -1,18 +1,28 @@".to_string()],
                hunk_indices: vec![0],
                additions: 13,
                deletions: 3,
                patch_hash: "tutorial-toolbar".to_string(),
                risk_score: 2,
                review_thread_ids: vec!["tutorial-thread-toolbar".to_string()],
                warnings: Vec::new(),
            },
            ChangeAtom {
                id: "tutorial-atom-tests".to_string(),
                source: ChangeAtomSource::File,
                path: "tests/review_toolbar_test.rs".to_string(),
                previous_path: None,
                role: ChangeRole::Tests,
                semantic_kind: Some("regression-test".to_string()),
                symbol_name: Some("toolbar_shows_pending_review_state".to_string()),
                defined_symbols: Vec::new(),
                referenced_symbols: vec!["build_review_toolbar_state".to_string()],
                old_range: None,
                new_range: Some(LineRange { start: 1, end: 19 }),
                hunk_headers: vec!["@@ -0,0 +1,19 @@".to_string()],
                hunk_indices: vec![0],
                additions: 19,
                deletions: 0,
                patch_hash: "tutorial-tests".to_string(),
                risk_score: 1,
                review_thread_ids: Vec::new(),
                warnings: Vec::new(),
            },
        ],
        warnings: Vec::<StackWarning>::new(),
        provider: Some(StackProviderMetadata {
            provider: "tutorial-fixture".to_string(),
            raw_payload: None,
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }
}

pub fn review_guide(detail: &PullRequestDetail, stack: ReviewStack) -> GeneratedReviewGuide {
    let structural_evidence = StructuralEvidencePack::empty();
    GeneratedReviewGuide {
        provider: CodeTourProvider::Codex,
        model: Some("tutorial-fixture".to_string()),
        generated_at_ms: GENERATED_AT,
        code_version_key: tour_code_version_key(detail),
        generator_version: REVIEW_GUIDE_GENERATOR_VERSION.to_string(),
        structural_evidence_version: STRUCTURAL_EVIDENCE_VERSION.to_string(),
        partner_overview: "Guided Review groups the tutorial into the feedback UI change and the test that protects it.".to_string(),
        review_strategy: "Read the toolbar state first, then use the test layer to confirm the expected finish-review behavior.".to_string(),
        open_questions: vec![
            "Should waypoints be shown even when there are no pending line comments?".to_string(),
        ],
        warnings: Vec::new(),
        structural_evidence,
        layers: vec![
            ReviewGuideLayer {
                layer_id: "tutorial-layer-feedback-ui".to_string(),
                title: "Feedback toolbar state".to_string(),
                what_changed: "The toolbar state now includes waypoint counts and a pending-review submit label.".to_string(),
                why_it_matters: "Reviewers can keep orientation while drafting feedback and finishing the pass.".to_string(),
                how_to_review: vec![
                    "Check the state inputs for pending comments and waypoints.".to_string(),
                    "Confirm the finish action remains visible without crowding the toolbar.".to_string(),
                ],
                bug_risks: vec![
                    "A stale pending count would encourage submitting incomplete feedback.".to_string(),
                ],
                evidence_notes: vec!["One unresolved tutorial comment is attached to this layer.".to_string()],
                follow_ups: Vec::new(),
                structural_evidence_status: StructuralEvidenceStatus::Unavailable,
            },
            ReviewGuideLayer {
                layer_id: "tutorial-layer-test-coverage".to_string(),
                title: "Feedback regression coverage".to_string(),
                what_changed: "A focused test covers the toolbar label and waypoint count.".to_string(),
                why_it_matters: "The feedback flow is easy to regress during review UI polish.".to_string(),
                how_to_review: vec![
                    "Confirm the test names the user-visible behavior.".to_string(),
                    "Check whether another test should cover the zero-pending state.".to_string(),
                ],
                bug_risks: Vec::new(),
                evidence_notes: Vec::new(),
                follow_ups: Vec::new(),
                structural_evidence_status: StructuralEvidenceStatus::Unavailable,
            },
        ],
        stack,
    }
}

pub fn review_session() -> ReviewSessionDocument {
    ReviewSessionDocument {
        selected_file_path: Some("src/review_toolbar.rs".to_string()),
        selected_diff_anchor: Some(DiffAnchor {
            file_path: "src/review_toolbar.rs".to_string(),
            hunk_header: Some("@@ -1,18 +1,28 @@".to_string()),
            line: Some(7),
            side: Some("RIGHT".to_string()),
            thread_id: None,
        }),
        center_mode: ReviewCenterMode::SemanticDiff,
        code_lens_mode: ReviewCenterMode::SemanticDiff,
        normal_diff_layout: DiffLayout::Unified,
        structural_diff_layout: DiffLayout::SideBySide,
        guided_review_lens: ReviewGuideLens::Diff,
        guided_review_panel_width: crate::review_session::GUIDED_REVIEW_PANEL_DEFAULT_WIDTH,
        wrap_diff_lines: false,
        show_file_tree: true,
        source_target: None,
        waymarks: vec![ReviewWaymark {
            id: "tutorial-waymark-toolbar".to_string(),
            name: "Toolbar submit state".to_string(),
            location: ReviewLocation::from_diff(
                "src/review_toolbar.rs",
                Some(DiffAnchor {
                    file_path: "src/review_toolbar.rs".to_string(),
                    hunk_header: Some("@@ -1,18 +1,28 @@".to_string()),
                    line: Some(7),
                    side: Some("RIGHT".to_string()),
                    thread_id: None,
                }),
            ),
            created_at_ms: GENERATED_AT,
        }],
        task_route: Some(ReviewTaskRoute {
            id: "tutorial-feedback-route".to_string(),
            title: "Feedback flow".to_string(),
            summary: "Review the toolbar state, then verify the regression test.".to_string(),
            stops: vec![
                ReviewLocation::from_ai_tour(
                    "src/review_toolbar.rs",
                    Some(DiffAnchor {
                        file_path: "src/review_toolbar.rs".to_string(),
                        hunk_header: Some("@@ -1,18 +1,28 @@".to_string()),
                        line: Some(7),
                        side: Some("RIGHT".to_string()),
                        thread_id: None,
                    }),
                ),
                ReviewLocation::from_ai_tour(
                    "tests/review_toolbar_test.rs",
                    Some(DiffAnchor {
                        file_path: "tests/review_toolbar_test.rs".to_string(),
                        hunk_header: Some("@@ -0,0 +1,19 @@".to_string()),
                        line: Some(9),
                        side: Some("RIGHT".to_string()),
                        thread_id: None,
                    }),
                ),
            ],
        }),
        route: Vec::new(),
        history_back: Vec::new(),
        history_forward: Vec::new(),
        last_read: None,
        collapsed_sections: Vec::new(),
        collapsed_file_paths: Vec::new(),
        reviewed_file_paths: Vec::new(),
        stack_rail_expanded: true,
        selected_stack_layer_id: Some("tutorial-layer-feedback-ui".to_string()),
        stack_diff_mode: StackDiffMode::CurrentLayerOnly,
        reviewed_stack_layer_ids: Vec::new(),
        reviewed_stack_atom_ids: Vec::new(),
    }
}

pub fn request_key(detail: &PullRequestDetail) -> String {
    format!(
        "tutorial:{}:{}:{}",
        detail.repository,
        detail.number,
        detail.head_ref_oid.as_deref().unwrap_or("head")
    )
}

pub fn tour_states() -> HashMap<CodeTourProvider, crate::state::CodeTourState> {
    let mut states = HashMap::new();
    states.insert(
        CodeTourProvider::Codex,
        crate::state::CodeTourState {
            request_key: Some(TUTORIAL_PR_KEY.to_string()),
            document: Some(generated_tour()),
            loading: false,
            generating: false,
            progress_summary: None,
            progress_detail: None,
            progress_log: Vec::new(),
            progress_log_file_path: None,
            error: None,
            message: Some("Tutorial Guided Review data loaded locally.".to_string()),
            success: true,
        },
    );
    states
}

pub fn raw_diff() -> &'static str {
    r#"diff --git a/src/review_toolbar.rs b/src/review_toolbar.rs
index 0b11111..0b22222 100644
--- a/src/review_toolbar.rs
+++ b/src/review_toolbar.rs
@@ -1,18 +1,28 @@
 pub struct ReviewToolbarState {
     pub pending_comments: usize,
+    pub waypoints: usize,
     pub is_submitting: bool,
 }
 
-pub fn build_review_toolbar_state(pending_comments: usize, is_submitting: bool) -> ReviewToolbarState {
+pub fn build_review_toolbar_state(
+    pending_comments: usize,
+    waypoints: usize,
+    is_submitting: bool,
+) -> ReviewToolbarState {
     ReviewToolbarState {
         pending_comments,
+        waypoints,
         is_submitting,
     }
 }
 
 pub fn finish_label(state: &ReviewToolbarState) -> String {
     if state.pending_comments > 0 {
         format!("Submit review ({})", state.pending_comments)
     } else {
-        "Submit review".to_string()
+        "Finish review".to_string()
     }
 }
+
+pub fn waypoint_label(state: &ReviewToolbarState) -> Option<String> {
+    (state.waypoints > 0).then(|| format!("{} waypoints", state.waypoints))
+}
diff --git a/tests/review_toolbar_test.rs b/tests/review_toolbar_test.rs
new file mode 100644
index 0000000..0b33333
--- /dev/null
+++ b/tests/review_toolbar_test.rs
@@ -0,0 +1,19 @@
+use remiss::review_toolbar::{build_review_toolbar_state, finish_label, waypoint_label};
+
+#[test]
+fn toolbar_shows_pending_review_state() {
+    let state = build_review_toolbar_state(1, 2, false);
+
+    assert_eq!(finish_label(&state), "Submit review (1)");
+    assert_eq!(waypoint_label(&state).as_deref(), Some("2 waypoints"));
+}
+
+#[test]
+fn toolbar_uses_finish_copy_without_pending_comments() {
+    let state = build_review_toolbar_state(0, 0, false);
+
+    assert_eq!(finish_label(&state), "Finish review");
+    assert_eq!(waypoint_label(&state), None);
+}
"#
}

fn review_threads() -> Vec<PullRequestReviewThread> {
    vec![PullRequestReviewThread {
        id: "tutorial-thread-toolbar".to_string(),
        path: "src/review_toolbar.rs".to_string(),
        line: Some(7),
        original_line: Some(4),
        start_line: None,
        original_start_line: None,
        diff_side: "RIGHT".to_string(),
        start_diff_side: None,
        is_collapsed: false,
        is_outdated: false,
        is_resolved: false,
        subject_type: "LINE".to_string(),
        resolved_by_login: None,
        viewer_can_reply: true,
        viewer_can_resolve: true,
        viewer_can_unresolve: true,
        comments: vec![PullRequestReviewComment {
            id: "tutorial-comment-toolbar".to_string(),
            author_login: "senior-reviewer".to_string(),
            author_avatar_url: None,
            body:
                "Good place to check whether waypoints and pending comments can drift out of sync."
                    .to_string(),
            path: "src/review_toolbar.rs".to_string(),
            line: Some(7),
            original_line: Some(4),
            start_line: None,
            original_start_line: None,
            state: "SUBMITTED".to_string(),
            created_at: UPDATED_AT.to_string(),
            updated_at: UPDATED_AT.to_string(),
            published_at: Some(UPDATED_AT.to_string()),
            reply_to_id: None,
            viewer_can_update: false,
            viewer_can_delete: false,
            url: TUTORIAL_URL.to_string(),
        }],
    }]
}

fn pending_review() -> PendingPullRequestReview {
    PendingPullRequestReview {
        id: "tutorial-pending-review".to_string(),
        author_login: "you".to_string(),
        author_avatar_url: None,
        body: String::new(),
        comments: vec![PullRequestReviewComment {
            id: "tutorial-pending-comment".to_string(),
            author_login: "you".to_string(),
            author_avatar_url: None,
            body: "Draft note: confirm the empty state still says Finish review.".to_string(),
            path: "tests/review_toolbar_test.rs".to_string(),
            line: Some(14),
            original_line: None,
            start_line: None,
            original_start_line: None,
            state: "PENDING".to_string(),
            created_at: UPDATED_AT.to_string(),
            updated_at: UPDATED_AT.to_string(),
            published_at: None,
            reply_to_id: None,
            viewer_can_update: true,
            viewer_can_delete: true,
            url: TUTORIAL_URL.to_string(),
        }],
    }
}

pub fn apply_fixture_to_detail_state(detail_state: &mut crate::state::DetailState) {
    let snapshot = snapshot();
    let detail = snapshot
        .detail
        .as_ref()
        .expect("tutorial snapshot includes detail")
        .clone();
    let stack = review_stack();
    let guide = review_guide(&detail, stack.clone());
    let request_key = request_key(&detail);

    detail_state.snapshot = Some(snapshot);
    detail_state.loading = false;
    detail_state.syncing = false;
    detail_state.error = None;
    detail_state.review_intelligence_request_key = Some(request_key.clone());
    detail_state.review_intelligence_loading = false;
    detail_state.review_brief_state = crate::state::ReviewBriefState {
        request_key: Some(request_key.clone()),
        document: Some(review_brief(&detail)),
        loading: false,
        generating: false,
        progress_text: None,
        error: None,
        message: Some("Tutorial briefing loaded locally.".to_string()),
        success: true,
    };
    detail_state.review_guide_state = crate::state::ReviewGuideState {
        request_key: Some(request_key.clone()),
        document: Some(Arc::new(guide)),
        loading: false,
        generating: false,
        progress_text: None,
        error: None,
        message: Some("Tutorial Guided Review loaded locally.".to_string()),
        success: true,
    };
    detail_state.ai_stack_state = crate::state::AiStackState {
        request_key: Some(request_key),
        stack: Some(Arc::new(stack)),
        loading: false,
        generating: false,
        error: None,
        message: Some("Tutorial review stack loaded locally.".to_string()),
        success: true,
    };
    detail_state.tour_states = tour_states();
    detail_state.review_session =
        crate::review_session::ReviewSessionState::from_document(review_session());
    detail_state.stack_open_pull_requests = Some(Vec::new());
    detail_state.stack_open_pull_requests_loading = false;
    detail_state.stack_open_pull_requests_error = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tutorial_pr_fixture_has_core_review_data() {
        let snapshot = snapshot();
        let detail = snapshot.detail.expect("detail");

        assert_eq!(detail.id, TUTORIAL_PR_KEY);
        assert_eq!(detail.files.len(), 2);
        assert_eq!(detail.parsed_diff.len(), 2);
        assert!(!detail.review_threads.is_empty());
        assert!(detail.viewer_pending_review.is_some());
        assert_eq!(review_stack().layers.len(), 2);
        assert_eq!(review_session().center_mode, ReviewCenterMode::SemanticDiff);
    }

    #[test]
    fn tutorial_summary_uses_local_key() {
        let summary = summary();

        assert_eq!(summary.local_key.as_deref(), Some(TUTORIAL_PR_KEY));
        assert_eq!(summary.state, "TUTORIAL");
    }
}
