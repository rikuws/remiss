use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::CacheStore;
use crate::diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine};
use crate::github::{
    PullRequestComment, PullRequestDataCompleteness, PullRequestDetail, PullRequestFile,
    PullRequestReview, PullRequestReviewComment, PullRequestReviewThread,
};
use crate::lsp::{LspServerCapabilities, LspServerStatus};
use crate::managed_lsp::ManagedServerKind;
use crate::onboarding::{StartupWizardOptions, WizardStepTarget};
use crate::review_session::ReviewCenterMode;
use crate::tutorial_pr::TUTORIAL_PR_KEY;

use super::{
    diff_anchor_for_line, first_review_comment_after_focus_index, review_comment_navigation_items,
    summary_key, AppState, DetailState, DiffScrollbarActivity, PullRequestSurface, ReviewModeFocus,
    SectionId, StructuralDiffWarmupState,
};

fn temp_cache_store(name: &str) -> CacheStore {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = PathBuf::from(format!(
        "/tmp/remiss-state-test-{name}-{suffix}/cache.sqlite"
    ));
    CacheStore::new(path).expect("cache")
}

#[test]
fn app_state_mute_unmute_persists_over_restart() {
    let cache = temp_cache_store("muted-state");
    let cache_path = cache.path().to_path_buf();
    let mut state = AppState::new(cache, StartupWizardOptions::force_welcome());

    state.mute_repository("owner/repo");

    let restarted_cache = CacheStore::new(cache_path.clone()).expect("reopen cache");
    let mut restarted = AppState::new(restarted_cache, StartupWizardOptions::force_welcome());
    assert!(restarted.muted_repos.contains("owner/repo"));

    restarted.unmute_repository("owner/repo");

    let final_cache = CacheStore::new(cache_path).expect("reopen cache");
    let final_state = AppState::new(final_cache, StartupWizardOptions::force_welcome());
    assert!(!final_state.muted_repos.contains("owner/repo"));
}

#[test]
fn diff_scrollbar_activity_hides_only_current_generation() {
    let activity = DiffScrollbarActivity::default();
    assert!(!activity.is_visible());

    let (first_generation, was_hidden) = activity.show();
    assert!(was_hidden);
    assert!(activity.is_visible());

    let (second_generation, was_hidden) = activity.show();
    assert!(!was_hidden);
    assert!(activity.is_visible());

    assert!(!activity.hide_if_current(first_generation));
    assert!(activity.is_visible());
    assert!(activity.hide_if_current(second_generation));
    assert!(!activity.is_visible());
}

#[test]
fn lsp_status_notice_tracks_symbol_loading_after_server_ready() {
    let mut detail_state = DetailState::default();
    detail_state.lsp_statuses.insert(
        "src/lib.rs".to_string(),
        LspServerStatus::ready(
            "rust".to_string(),
            "/tmp/bin/rust-analyzer".to_string(),
            LspServerCapabilities {
                hover_supported: true,
                ..Default::default()
            },
        ),
    );

    assert_eq!(detail_state.lsp_status_notice_for_path("src/lib.rs"), None);

    detail_state.begin_lsp_symbol_loading("src/lib.rs");
    detail_state.begin_lsp_symbol_loading("src/lib.rs");
    let notice = detail_state
        .lsp_status_notice_for_path("src/lib.rs")
        .expect("expected symbol loading notice");
    assert_eq!(notice.title, "Loading code intelligence");
    assert_eq!(
        notice.detail,
        "rust-analyzer is fetching hover details for lib.rs."
    );
    assert!(notice.busy);

    detail_state.finish_lsp_symbol_loading("src/lib.rs");
    assert_eq!(
        detail_state
            .lsp_status_notice_for_path("src/lib.rs")
            .expect("expected symbol loading notice")
            .title,
        "Loading code intelligence"
    );

    detail_state.finish_lsp_symbol_loading("src/lib.rs");
    assert_eq!(detail_state.lsp_status_notice_for_path("src/lib.rs"), None);
}

#[test]
fn lsp_status_notice_offers_managed_install_for_missing_server() {
    let mut detail_state = DetailState::default();
    detail_state.lsp_statuses.insert(
        "src/lib.rs".to_string(),
        LspServerStatus::missing_server("rust", "rust-analyzer"),
    );

    let notice = detail_state
        .lsp_status_notice_for_path("src/lib.rs")
        .expect("expected missing server notice");
    assert_eq!(notice.title, "Language server not installed");
    assert_eq!(notice.install_kind, Some(ManagedServerKind::RustAnalyzer));
    assert!(notice.detail.contains("managed rust-analyzer"));
    assert!(!notice.busy);
    assert_eq!(notice.dismissal_key, "managed-missing:RustAnalyzer");
}

#[test]
fn dismissed_lsp_status_notice_hides_same_managed_server() {
    let mut state = AppState::new(
        temp_cache_store("lsp-dismiss"),
        StartupWizardOptions::force_welcome(),
    );
    let detail_key = "owner/repo#1".to_string();
    let mut detail_state = DetailState::default();
    detail_state.lsp_statuses.insert(
        "src/lib.rs".to_string(),
        LspServerStatus::missing_server("rust", "rust-analyzer"),
    );
    detail_state.lsp_statuses.insert(
        "src/main.rs".to_string(),
        LspServerStatus::missing_server("rust", "rust-analyzer"),
    );
    state.active_pr_key = Some(detail_key.clone());
    state.detail_states.insert(detail_key, detail_state);

    let notice = state
        .active_lsp_status_notice_for_path("src/lib.rs")
        .expect("expected missing server notice");
    state.dismiss_lsp_status_notice_key(notice.dismissal_key);

    assert!(state
        .active_lsp_status_notice_for_path("src/lib.rs")
        .is_none());
    assert!(state
        .active_lsp_status_notice_for_path("src/main.rs")
        .is_none());
}

#[test]
fn diff_anchor_for_line_preserves_requested_side() {
    let parsed = ParsedDiffFile {
        path: "src/lib.rs".to_string(),
        previous_path: Some("src/lib.rs".to_string()),
        is_binary: false,
        hunks: vec![ParsedDiffHunk {
            header: "@@ -1,2 +1,2 @@".to_string(),
            lines: vec![
                ParsedDiffLine {
                    kind: DiffLineKind::Context,
                    prefix: " ".to_string(),
                    left_line_number: Some(1),
                    right_line_number: Some(1),
                    content: "fn main() {".to_string(),
                },
                ParsedDiffLine {
                    kind: DiffLineKind::Deletion,
                    prefix: "-".to_string(),
                    left_line_number: Some(2),
                    right_line_number: None,
                    content: "    old();".to_string(),
                },
                ParsedDiffLine {
                    kind: DiffLineKind::Addition,
                    prefix: "+".to_string(),
                    left_line_number: None,
                    right_line_number: Some(2),
                    content: "    new();".to_string(),
                },
            ],
        }],
    };

    let right =
        diff_anchor_for_line(&parsed, 2, Some("RIGHT")).expect("right line anchor should resolve");
    let left =
        diff_anchor_for_line(&parsed, 2, Some("LEFT")).expect("left line anchor should resolve");

    assert_eq!(right.side.as_deref(), Some("RIGHT"));
    assert_eq!(right.hunk_header.as_deref(), Some("@@ -1,2 +1,2 @@"));
    assert_eq!(left.side.as_deref(), Some("LEFT"));
}

#[test]
fn onboarding_tutorial_pr_is_injected_and_removed_without_workspace_queue_pollution() {
    let cache = temp_cache_store("tutorial-onboarding");
    let mut state = AppState::new(cache, StartupWizardOptions::force_welcome());
    state.active_section = SectionId::Settings;
    state.active_surface = PullRequestSurface::Overview;
    state.active_pr_key = None;

    state.set_onboarding_step(1);

    assert_eq!(state.active_pr_key.as_deref(), Some(TUTORIAL_PR_KEY));
    assert!(state
        .open_tabs
        .iter()
        .any(|tab| summary_key(tab) == TUTORIAL_PR_KEY));
    assert!(state.detail_states.contains_key(TUTORIAL_PR_KEY));
    assert!(state.workspace.is_none());

    state.set_onboarding_step(2);
    assert_eq!(
        state.active_onboarding_target(),
        Some(WizardStepTarget::LocalReview)
    );
    assert_eq!(state.active_section, SectionId::Overview);
    assert_eq!(state.active_pr_key, None);

    state.set_onboarding_step(1);
    assert_eq!(
        state
            .detail_states
            .get(TUTORIAL_PR_KEY)
            .map(|detail| detail.review_session.center_mode),
        Some(ReviewCenterMode::SemanticDiff)
    );

    state
        .review_ai_settings
        .settings
        .experimental_features_enabled = true;
    state.set_onboarding_step(2);
    assert_eq!(
        state
            .detail_states
            .get(TUTORIAL_PR_KEY)
            .map(|detail| detail.review_session.center_mode),
        Some(ReviewCenterMode::GuidedReview)
    );

    state.set_onboarding_step(3);
    assert_eq!(state.active_section, SectionId::Overview);
    assert_eq!(state.active_pr_key, None);
    assert!(state
        .open_tabs
        .iter()
        .any(|tab| summary_key(tab) == TUTORIAL_PR_KEY));

    state.complete_active_onboarding_wizard();

    assert!(!state
        .open_tabs
        .iter()
        .any(|tab| summary_key(tab) == TUTORIAL_PR_KEY));
    assert!(!state.detail_states.contains_key(TUTORIAL_PR_KEY));
    assert_eq!(state.active_section, SectionId::Settings);
    assert_eq!(state.active_pr_key, None);
}

#[test]
fn structural_diff_warmup_hides_complete_ready_status() {
    let state = StructuralDiffWarmupState {
        request_key: Some("pr:head".to_string()),
        total: 4,
        completed: 4,
        failed: 0,
        loading: false,
    };

    assert_eq!(state.status_text(), None);
}

#[test]
fn structural_diff_warmup_status_avoids_ready_copy() {
    let state = StructuralDiffWarmupState {
        request_key: Some("pr:head".to_string()),
        total: 4,
        completed: 2,
        failed: 1,
        loading: true,
    };

    let text = state.status_text().expect("loading status is visible");
    assert!(text.starts_with("Preparing structural diffs 3/4"));
    assert!(!text.contains("ready"));
}

#[test]
fn review_comment_navigation_follows_rendered_diff_order() {
    let detail = detail_with_threads(vec![
        review_thread("file", "src/a.rs", None, "RIGHT", "FILE", false),
        review_thread("inline", "src/a.rs", Some(3), "RIGHT", "LINE", false),
        review_thread("outdated", "src/a.rs", Some(1), "RIGHT", "LINE", true),
        review_thread("second-file", "src/b.rs", Some(5), "RIGHT", "LINE", false),
    ]);

    let items = review_comment_navigation_items(&detail, ReviewCenterMode::SemanticDiff);
    let ids = items
        .iter()
        .map(|item| detail.review_threads[item.thread_index].id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["file", "inline", "outdated", "second-file"]);
}

#[test]
fn next_review_comment_after_focus_uses_rendered_row_position() {
    let detail = detail_with_threads(vec![
        review_thread("file", "src/a.rs", None, "RIGHT", "FILE", false),
        review_thread("inline", "src/a.rs", Some(3), "RIGHT", "LINE", false),
        review_thread("outdated", "src/a.rs", Some(1), "RIGHT", "LINE", true),
        review_thread("second-file", "src/b.rs", Some(5), "RIGHT", "LINE", false),
    ]);
    let items = review_comment_navigation_items(&detail, ReviewCenterMode::StructuralDiff);
    let focus = ReviewModeFocus {
        mode: ReviewCenterMode::StructuralDiff,
        file_path: "src/a.rs".to_string(),
        line: Some(4),
        side: Some("RIGHT".to_string()),
        anchor: None,
    };

    let index = first_review_comment_after_focus_index(&detail, &items, &focus)
        .expect("outdated thread should be after the focused row");

    assert_eq!(
        detail.review_threads[items[index].thread_index].id,
        "outdated"
    );
    assert_eq!(items[index].location.mode, ReviewCenterMode::StructuralDiff);
}

fn detail_with_threads(review_threads: Vec<PullRequestReviewThread>) -> PullRequestDetail {
    PullRequestDetail {
        id: "pr".to_string(),
        repository: "org/repo".to_string(),
        number: 1,
        title: "Review comments".to_string(),
        body: String::new(),
        url: "https://example.com/pr".to_string(),
        author_login: "alice".to_string(),
        author_avatar_url: None,
        state: "OPEN".to_string(),
        is_draft: false,
        review_decision: None,
        base_ref_name: "main".to_string(),
        head_ref_name: "feature".to_string(),
        base_ref_oid: None,
        head_ref_oid: None,
        additions: 0,
        deletions: 0,
        changed_files: 2,
        comments_count: 0,
        commits_count: 1,
        commits: Vec::new(),
        created_at: "2026-05-13T00:00:00Z".to_string(),
        updated_at: "2026-05-13T00:00:00Z".to_string(),
        labels: Vec::new(),
        reviewers: Vec::new(),
        reviewer_avatar_urls: BTreeMap::new(),
        comments: Vec::<PullRequestComment>::new(),
        latest_reviews: Vec::<PullRequestReview>::new(),
        review_threads,
        viewer_pending_review: None,
        files: vec![file("src/a.rs"), file("src/b.rs")],
        raw_diff: String::new(),
        parsed_diff: vec![parsed_file("src/a.rs", 1, 5), parsed_file("src/b.rs", 1, 6)],
        data_completeness: PullRequestDataCompleteness::default(),
    }
}

fn file(path: &str) -> PullRequestFile {
    PullRequestFile {
        path: path.to_string(),
        additions: 0,
        deletions: 0,
        change_type: "MODIFIED".to_string(),
    }
}

fn parsed_file(path: &str, start: i64, end: i64) -> ParsedDiffFile {
    ParsedDiffFile {
        path: path.to_string(),
        previous_path: Some(path.to_string()),
        is_binary: false,
        hunks: vec![ParsedDiffHunk {
            header: format!("@@ -{start},{end} +{start},{end} @@"),
            lines: (start..=end)
                .map(|line| ParsedDiffLine {
                    kind: DiffLineKind::Context,
                    prefix: " ".to_string(),
                    left_line_number: Some(line),
                    right_line_number: Some(line),
                    content: format!("line {line}"),
                })
                .collect(),
        }],
    }
}

fn review_thread(
    id: &str,
    path: &str,
    line: Option<i64>,
    diff_side: &str,
    subject_type: &str,
    is_outdated: bool,
) -> PullRequestReviewThread {
    PullRequestReviewThread {
        id: id.to_string(),
        path: path.to_string(),
        line,
        original_line: line,
        start_line: None,
        original_start_line: None,
        diff_side: diff_side.to_string(),
        start_diff_side: None,
        is_collapsed: false,
        is_outdated,
        is_resolved: false,
        subject_type: subject_type.to_string(),
        resolved_by_login: None,
        viewer_can_reply: true,
        viewer_can_resolve: true,
        viewer_can_unresolve: true,
        comments: vec![review_comment(id, path, line)],
    }
}

fn review_comment(id: &str, path: &str, line: Option<i64>) -> PullRequestReviewComment {
    PullRequestReviewComment {
        id: format!("{id}-comment"),
        author_login: "bob".to_string(),
        author_avatar_url: None,
        body: "Please check this.".to_string(),
        path: path.to_string(),
        line,
        original_line: line,
        start_line: None,
        original_start_line: None,
        state: "SUBMITTED".to_string(),
        created_at: "2026-05-13T00:00:00Z".to_string(),
        updated_at: "2026-05-13T00:00:00Z".to_string(),
        published_at: Some("2026-05-13T00:00:00Z".to_string()),
        reply_to_id: None,
        viewer_can_update: false,
        viewer_can_delete: false,
        url: "https://example.com/comment".to_string(),
    }
}
