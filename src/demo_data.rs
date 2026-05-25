use std::collections::BTreeMap;

use crate::{
    diff::parse_unified_diff,
    github::{
        ActionResult, AuthState, PendingPullRequestReview, PullRequestComment, PullRequestCommit,
        PullRequestDataCompleteness, PullRequestDetail, PullRequestDetailSnapshot, PullRequestFile,
        PullRequestQueue, PullRequestReview, PullRequestReviewComment, PullRequestReviewThread,
        PullRequestSummary, RepositoryFileContent, Viewer, WorkspaceSnapshot,
        REPOSITORY_FILE_SOURCE_GITHUB,
    },
    stacks::model::StackPullRequestRef,
    triage::{PullRequestTriageSignal, PullRequestTriageSignalKind},
};

pub const DEMO_MODE_ENV: &str = "REMISS_DEMO_MODE";

const DEMO_FETCHED_AT_MS: i64 = 1_780_000_000_000;
const DEMO_VIEWER_LOGIN: &str = "demo-reviewer";
const DEMO_HOSTNAME: &str = "demo.local";

#[derive(Clone)]
struct DemoPullRequest {
    summary: PullRequestSummary,
    detail: PullRequestDetail,
}

#[derive(Clone, Copy)]
struct DemoFile {
    path: &'static str,
    additions: i64,
    deletions: i64,
    change_type: &'static str,
}

pub fn demo_mode_enabled() -> bool {
    demo_mode_enabled_from_vars(|name| std::env::var(name).ok())
}

fn demo_mode_enabled_from_vars<F>(mut var: F) -> bool
where
    F: FnMut(&str) -> Option<String>,
{
    var(DEMO_MODE_ENV)
        .map(|value| env_truthy(&value))
        .unwrap_or(false)
        || crate::screenshot_mode::screenshot_mode_enabled_from_vars(var)
}

pub fn auth_state() -> AuthState {
    AuthState {
        is_authenticated: true,
        active_login: Some(DEMO_VIEWER_LOGIN.to_string()),
        active_hostname: Some(DEMO_HOSTNAME.to_string()),
        message: format!("{DEMO_MODE_ENV}=1 is serving local demo data."),
    }
}

pub fn workspace_snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        auth: auth_state(),
        loaded_from_cache: false,
        fetched_at_ms: Some(DEMO_FETCHED_AT_MS),
        viewer: Some(Viewer {
            login: DEMO_VIEWER_LOGIN.to_string(),
            name: Some("Demo Reviewer".to_string()),
        }),
        queues: demo_queues(),
    }
}

pub fn pull_request_detail_snapshots() -> Vec<PullRequestDetailSnapshot> {
    demo_pull_requests()
        .into_iter()
        .map(|pull| PullRequestDetailSnapshot {
            auth: auth_state(),
            loaded_from_cache: false,
            fetched_at_ms: Some(DEMO_FETCHED_AT_MS),
            detail: Some(pull.detail),
        })
        .collect()
}

pub fn pull_request_summary(repository: &str, number: i64) -> Option<PullRequestSummary> {
    demo_pull_requests()
        .into_iter()
        .find(|pull| pull.summary.repository == repository && pull.summary.number == number)
        .map(|pull| pull.summary)
}

pub fn pull_request_detail_snapshot(
    repository: &str,
    number: i64,
) -> Option<PullRequestDetailSnapshot> {
    demo_pull_requests()
        .into_iter()
        .find(|pull| pull.detail.repository == repository && pull.detail.number == number)
        .map(|pull| PullRequestDetailSnapshot {
            auth: auth_state(),
            loaded_from_cache: false,
            fetched_at_ms: Some(DEMO_FETCHED_AT_MS),
            detail: Some(pull.detail),
        })
}

pub fn pull_request_file_content(
    repository: &str,
    reference: &str,
    path: &str,
) -> Option<RepositoryFileContent> {
    let content = source_content(repository, path)?;
    let size_bytes = content.len();
    Some(RepositoryFileContent {
        repository: repository.to_string(),
        reference: reference.to_string(),
        path: path.to_string(),
        content: Some(content),
        is_binary: false,
        size_bytes,
        source: REPOSITORY_FILE_SOURCE_GITHUB.to_string(),
    })
}

pub fn open_pull_request_stack_refs(repository: &str) -> Vec<StackPullRequestRef> {
    demo_pull_requests()
        .into_iter()
        .filter(|pull| pull.summary.repository == repository)
        .map(|pull| StackPullRequestRef {
            repository: pull.summary.repository,
            number: pull.summary.number,
            title: pull.summary.title,
            url: pull.summary.url,
            base_ref_name: pull.detail.base_ref_name,
            head_ref_name: pull.detail.head_ref_name,
            base_ref_oid: pull.detail.base_ref_oid,
            head_ref_oid: pull.detail.head_ref_oid,
            review_decision: pull.summary.review_decision,
            state: pull.summary.state,
            is_draft: pull.summary.is_draft,
        })
        .collect()
}

pub fn action_result(action: &str) -> ActionResult {
    ActionResult {
        success: true,
        message: format!("Demo mode: {action}. No GitHub data was changed."),
    }
}

fn env_truthy(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    !normalized.is_empty() && !matches!(normalized.as_str(), "0" | "false" | "no" | "off")
}

fn demo_queues() -> Vec<PullRequestQueue> {
    let pulls = demo_pull_requests();
    vec![
        queue(
            "reviewRequested",
            "Review requested",
            summaries_where(&pulls, |pull| {
                pull.summary.review_decision.as_deref() == Some("REVIEW_REQUIRED")
                    && !pull.summary.is_draft
            }),
        ),
        queue(
            "assigned",
            "Assigned",
            summaries_where(&pulls, |pull| {
                pull.detail
                    .reviewers
                    .iter()
                    .any(|reviewer| reviewer == DEMO_VIEWER_LOGIN)
                    || pull
                        .detail
                        .labels
                        .iter()
                        .any(|label| label == "assigned-demo")
            }),
        ),
        queue(
            "authored",
            "Authored",
            summaries_where(&pulls, |pull| {
                pull.summary.author_login == DEMO_VIEWER_LOGIN
                    || pull
                        .detail
                        .labels
                        .iter()
                        .any(|label| label == "authored-demo")
            }),
        ),
        queue(
            "mentioned",
            "Mentioned",
            summaries_where(&pulls, |pull| {
                pull.detail
                    .labels
                    .iter()
                    .any(|label| label == "mentioned-demo")
                    || pull
                        .detail
                        .comments
                        .iter()
                        .any(|comment| comment.body.contains(DEMO_VIEWER_LOGIN))
            }),
        ),
        queue(
            "involved",
            "Involved",
            pulls
                .iter()
                .take(24)
                .map(|pull| pull.summary.clone())
                .collect(),
        ),
    ]
}

fn queue(id: &str, label: &str, items: Vec<PullRequestSummary>) -> PullRequestQueue {
    PullRequestQueue {
        id: id.to_string(),
        label: label.to_string(),
        total_count: items.len() as i64,
        items,
        is_complete: true,
        truncated_reason: None,
    }
}

fn summaries_where<F>(pulls: &[DemoPullRequest], mut predicate: F) -> Vec<PullRequestSummary>
where
    F: FnMut(&DemoPullRequest) -> bool,
{
    pulls
        .iter()
        .filter(|pull| predicate(pull))
        .map(|pull| pull.summary.clone())
        .collect()
}

fn demo_pull_requests() -> Vec<DemoPullRequest> {
    let mut pulls = vec![
        filter_presets_pr(),
        sidebar_polish_pr(),
        local_changes_pr(),
        docs_pr(),
    ];
    pulls.extend(extended_demo_prs());
    pulls.sort_by(|left, right| right.summary.updated_at.cmp(&left.summary.updated_at));
    pulls
}

fn filter_presets_pr() -> DemoPullRequest {
    build_demo_pr(
        "remiss/remiss",
        42,
        "Add saved review queue filter presets",
        "Adds reusable queue filters, trust signals, and the modal used to save custom presets.",
        "maya",
        "COLLABORATOR",
        false,
        Some("REVIEW_REQUIRED"),
        "2026-05-23T18:15:00Z",
        "2026-05-23T09:30:00Z",
        "main",
        "maya/filter-presets",
        "0000000000000000000000000000000000000042",
        "1111111111111111111111111111111111110042",
        &["review-ui", "filters", "needs-polish"],
        &["demo-reviewer", "riku"],
        demo_files(&[
            DemoFile {
                path: "src/review_filters.rs",
                additions: 82,
                deletions: 18,
                change_type: "MODIFIED",
            },
            DemoFile {
                path: "src/views/sections/pull_request_filters.rs",
                additions: 146,
                deletions: 0,
                change_type: "ADDED",
            },
            DemoFile {
                path: "src/triage.rs",
                additions: 64,
                deletions: 4,
                change_type: "MODIFIED",
            },
        ]),
        FILTER_PRESETS_DIFF.to_string(),
        vec![issue_comment(
            "demo-issue-filter-1",
            "riku",
            "The preset flow is easier to scan now. I left one thread on reset semantics.",
            "2026-05-23T16:20:00Z",
            "remiss/remiss",
            42,
        )],
        vec![review(
            "demo-review-filter-1",
            "riku",
            "COMMENTED",
            "Looks good overall. Please verify the custom preset survives a restart.",
            "2026-05-23T17:02:00Z",
        )],
        vec![review_thread(
            "demo-thread-filter-reset",
            "src/review_filters.rs",
            29,
            "When the active preset is deleted, the UI should fall back to All instead of leaving a stale hidden filter.",
            "riku",
            false,
            "2026-05-23T17:12:00Z",
        )],
        Some(pending_review(vec![review_comment(
            "demo-pending-filter-name",
            DEMO_VIEWER_LOGIN,
            "Draft: the dialog should probably trim duplicate whitespace before saving.",
            "src/views/sections/pull_request_filters.rs",
            48,
            None,
            "PENDING",
            "2026-05-23T18:01:00Z",
        )])),
        vec![
            signal(PullRequestTriageSignalKind::Trusted, "trusted", Some("collaborator")),
            signal(PullRequestTriageSignalKind::Vouched, "vouched", Some("VOUCHED.td")),
        ],
    )
}

fn sidebar_polish_pr() -> DemoPullRequest {
    build_demo_pr(
        "remiss/gpui-lab",
        77,
        "Polish collapsed sidebar navigation",
        "Tightens icon-only sidebar sizing and keeps route transitions stable while the workspace list changes.",
        "ash",
        "FIRST_TIME_CONTRIBUTOR",
        false,
        Some("REVIEW_REQUIRED"),
        "2026-05-22T14:40:00Z",
        "2026-05-21T19:10:00Z",
        "main",
        "ash/sidebar-motion",
        "0000000000000000000000000000000000000077",
        "1111111111111111111111111111111111110077",
        &["ui", "motion"],
        &["demo-reviewer"],
        demo_files(&[
            DemoFile {
                path: "src/views/root.rs",
                additions: 38,
                deletions: 16,
                change_type: "MODIFIED",
            },
            DemoFile {
                path: "src/views/motion.rs",
                additions: 27,
                deletions: 5,
                change_type: "MODIFIED",
            },
        ]),
        SIDEBAR_POLISH_DIFF.to_string(),
        vec![issue_comment(
            "demo-issue-sidebar-1",
            "lin",
            "I can reproduce the previous jitter at 320px, this patch seems to remove it.",
            "2026-05-22T13:00:00Z",
            "remiss/gpui-lab",
            77,
        )],
        vec![review(
            "demo-review-sidebar-1",
            "lin",
            "CHANGES_REQUESTED",
            "Please check the tooltip hit target in icon-only mode.",
            "2026-05-22T14:12:00Z",
        )],
        vec![
            review_thread(
                "demo-thread-sidebar-hit-target",
                "src/views/root.rs",
                118,
                "This keeps layout stable, but the hit target should remain at least 28px.",
                "lin",
                false,
                "2026-05-22T14:20:00Z",
            ),
            review_thread(
                "demo-thread-sidebar-motion",
                "src/views/motion.rs",
                18,
                "The easing constant reads better here than inline in the view.",
                "riku",
                true,
                "2026-05-22T13:52:00Z",
            ),
        ],
        None,
        vec![signal(
            PullRequestTriageSignalKind::FirstTimeContributor,
            "first-time contributor",
            None,
        )],
    )
}

fn local_changes_pr() -> DemoPullRequest {
    build_demo_pr(
        "remiss/local-changes",
        108,
        "Route local agent requests through review sessions",
        "Persists local agent requests alongside review-session state and keeps them distinct from GitHub review comments.",
        "jon",
        "CONTRIBUTOR",
        false,
        None,
        "2026-05-19T11:22:00Z",
        "2026-05-18T10:00:00Z",
        "main",
        "jon/local-agent-requests",
        "0000000000000000000000000000000000000108",
        "1111111111111111111111111111111111110108",
        &["local-changes", "agent-handoff"],
        &["demo-reviewer", "riku"],
        demo_files(&[
            DemoFile {
                path: "src/local_review.rs",
                additions: 420,
                deletions: 44,
                change_type: "MODIFIED",
            },
            DemoFile {
                path: "src/review_session.rs",
                additions: 318,
                deletions: 25,
                change_type: "MODIFIED",
            },
            DemoFile {
                path: "src/views/diff_view/review_comments.rs",
                additions: 286,
                deletions: 31,
                change_type: "MODIFIED",
            },
        ]),
        LOCAL_CHANGES_DIFF.to_string(),
        vec![
            issue_comment(
                "demo-issue-local-1",
                "riku",
                "This is the right product split. I want one more pass on refresh behavior.",
                "2026-05-19T08:30:00Z",
                "remiss/local-changes",
                108,
            ),
            issue_comment(
                "demo-issue-local-2",
                "maya",
                "The session key now survives closing and reopening the local review tab.",
                "2026-05-19T10:44:00Z",
                "remiss/local-changes",
                108,
            ),
        ],
        vec![review(
            "demo-review-local-1",
            "riku",
            "COMMENTED",
            "The flow is promising; the remaining risk is request/comment mixing.",
            "2026-05-19T10:55:00Z",
        )],
        vec![review_thread(
            "demo-thread-local-request-key",
            "src/review_session.rs",
            77,
            "Make sure this key never collides with a GitHub PR session key.",
            "riku",
            false,
            "2026-05-19T11:05:00Z",
        )],
        None,
        vec![signal(
            PullRequestTriageSignalKind::PriorContributor,
            "prior contributor",
            None,
        )],
    )
}

fn docs_pr() -> DemoPullRequest {
    build_demo_pr(
        "remiss/docs",
        12,
        "Refresh contributor workflow docs",
        "Documents the local development loop and adds the demo-mode launch command.",
        DEMO_VIEWER_LOGIN,
        "OWNER",
        true,
        None,
        "2026-05-24T08:45:00Z",
        "2026-05-24T08:00:00Z",
        "main",
        "demo/docs-demo-mode",
        "0000000000000000000000000000000000000012",
        "1111111111111111111111111111111111110012",
        &["docs", "draft"],
        &[],
        demo_files(&[
            DemoFile {
                path: "README.md",
                additions: 14,
                deletions: 2,
                change_type: "MODIFIED",
            },
            DemoFile {
                path: "docs/release-checklist.md",
                additions: 22,
                deletions: 0,
                change_type: "ADDED",
            },
        ]),
        DOCS_DIFF.to_string(),
        vec![issue_comment(
            "demo-issue-docs-1",
            "riku",
            "The command reads clearly now. I left one note on where release-only checks belong.",
            "2026-05-24T08:36:00Z",
            "remiss/docs",
            12,
        )],
        vec![review(
            "demo-review-docs-1",
            "riku",
            "COMMENTED",
            "Good demo-mode docs. Please keep the release checklist separate from everyday setup.",
            "2026-05-24T08:40:00Z",
        )],
        vec![review_thread(
            "demo-thread-docs-release-checklist",
            "docs/release-checklist.md",
            4,
            "This belongs in release checks, not the regular local development path.",
            "riku",
            false,
            "2026-05-24T08:41:00Z",
        )],
        None,
        vec![signal(
            PullRequestTriageSignalKind::Trusted,
            "trusted",
            Some("owner"),
        )],
    )
}

#[derive(Clone, Copy)]
struct ExtendedDemoScenario {
    repository: &'static str,
    number: i64,
    title: &'static str,
    body: &'static str,
    author_login: &'static str,
    author_association: &'static str,
    is_draft: bool,
    review_decision: Option<&'static str>,
    updated_at: &'static str,
    created_at: &'static str,
    head_ref_name: &'static str,
    labels: &'static [&'static str],
    reviewers: &'static [&'static str],
    paths: &'static [&'static str],
    additions: i64,
    deletions: i64,
    latest_review_state: Option<&'static str>,
    triage_kind: PullRequestTriageSignalKind,
    triage_label: &'static str,
    triage_detail: Option<&'static str>,
    issue_comment: Option<&'static str>,
    thread_comment: Option<&'static str>,
    pending_comment: Option<&'static str>,
    resolved_thread: bool,
}

fn extended_demo_prs() -> Vec<DemoPullRequest> {
    EXTENDED_DEMO_SCENARIOS
        .iter()
        .map(build_extended_demo_pr)
        .collect()
}

fn build_extended_demo_pr(scenario: &ExtendedDemoScenario) -> DemoPullRequest {
    let files = generated_files(scenario);
    let raw_diff = generated_diff(&files, scenario);
    let base_oid = format!("{:040x}", scenario.number.max(0));
    let head_oid = format!("{:040x}", scenario.number.max(0) + 10_000);
    let comments = scenario
        .issue_comment
        .map(|body| {
            issue_comment(
                &format!("demo-issue-{}-{}", scenario.repository, scenario.number),
                "demo-teammate",
                body,
                scenario.updated_at,
                scenario.repository,
                scenario.number,
            )
        })
        .into_iter()
        .collect();
    let latest_reviews = scenario
        .latest_review_state
        .map(|state| {
            review(
                &format!("demo-review-{}-{}", scenario.repository, scenario.number),
                "demo-reviewer",
                state,
                review_body_for_state(state),
                scenario.updated_at,
            )
        })
        .into_iter()
        .collect();
    let review_threads = scenario
        .thread_comment
        .map(|body| {
            review_thread(
                &format!("demo-thread-{}-{}", scenario.repository, scenario.number),
                &files
                    .first()
                    .map(|file| file.path.clone())
                    .unwrap_or_else(|| "src/lib.rs".to_string()),
                8,
                body,
                generated_review_thread_author(scenario),
                scenario.resolved_thread,
                scenario.updated_at,
            )
        })
        .into_iter()
        .collect();
    let viewer_pending_review = scenario.pending_comment.map(|body| {
        pending_review(vec![review_comment(
            &format!("demo-pending-{}-{}", scenario.repository, scenario.number),
            DEMO_VIEWER_LOGIN,
            body,
            &files
                .last()
                .map(|file| file.path.clone())
                .unwrap_or_else(|| "src/lib.rs".to_string()),
            11,
            None,
            "PENDING",
            scenario.updated_at,
        )])
    });

    build_demo_pr(
        scenario.repository,
        scenario.number,
        scenario.title,
        scenario.body,
        scenario.author_login,
        scenario.author_association,
        scenario.is_draft,
        scenario.review_decision,
        scenario.updated_at,
        scenario.created_at,
        "main",
        scenario.head_ref_name,
        &base_oid,
        &head_oid,
        scenario.labels,
        scenario.reviewers,
        files,
        raw_diff,
        comments,
        latest_reviews,
        review_threads,
        viewer_pending_review,
        vec![signal(
            scenario.triage_kind,
            scenario.triage_label,
            scenario.triage_detail,
        )],
    )
}

fn review_body_for_state(state: &str) -> &'static str {
    match state {
        "APPROVED" => "Approved in the demo fixture.",
        "CHANGES_REQUESTED" => "Demo fixture has a requested-change review.",
        _ => "Commented in the demo fixture.",
    }
}

fn generated_review_thread_author(scenario: &ExtendedDemoScenario) -> &'static str {
    if scenario.author_login == DEMO_VIEWER_LOGIN {
        "riku"
    } else {
        "demo-teammate"
    }
}

fn generated_files(scenario: &ExtendedDemoScenario) -> Vec<PullRequestFile> {
    let file_count = scenario.paths.len().max(1) as i64;
    scenario
        .paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let weight = index as i64 + 1;
            PullRequestFile {
                path: (*path).to_string(),
                additions: (scenario.additions / file_count).max(1) + weight,
                deletions: (scenario.deletions / file_count).max(0) + (weight % 3),
                change_type: generated_change_type(path).to_string(),
            }
        })
        .collect()
}

fn generated_change_type(path: &str) -> &'static str {
    if path.contains("deleted") || path.contains("removed") {
        "DELETED"
    } else if path.contains("new_")
        || path.contains("checklist")
        || path.contains("troubleshooting")
    {
        "ADDED"
    } else {
        "MODIFIED"
    }
}

fn generated_diff(files: &[PullRequestFile], scenario: &ExtendedDemoScenario) -> String {
    let mut diff = String::new();
    let slug = scenario
        .head_ref_name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();

    for (index, file) in files.iter().enumerate() {
        let index_hex = format!("{:06x}", scenario.number * 97 + index as i64);
        let path = &file.path;
        match file.change_type.as_str() {
            "ADDED" => {
                diff.push_str(&format!(
                    "diff --git a/{path} b/{path}\nnew file mode 100644\nindex 0000000..{index_hex}\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,12 @@\n+pub fn demo_{slug}_{index}() {{\n+    let review_surface = \"{}\";\n+    let queue = \"{}\";\n+    assert!(!review_surface.is_empty());\n+    assert!(!queue.is_empty());\n+}}\n",
                    scenario.repository,
                    scenario.title.replace('"', "'")
                ));
            }
            "DELETED" => {
                diff.push_str(&format!(
                    "diff --git a/{path} b/{path}\ndeleted file mode 100644\nindex {index_hex}..0000000\n--- a/{path}\n+++ /dev/null\n@@ -1,7 +0,0 @@\n-pub fn removed_demo_{slug}_{index}() {{\n-    let old_path = \"{path}\";\n-    assert!(!old_path.is_empty());\n-}}\n"
                ));
            }
            _ => {
                diff.push_str(&format!(
                    "diff --git a/{path} b/{path}\nindex {index_hex}..{index_hex}1 100644\n--- a/{path}\n+++ b/{path}\n@@ -1,8 +1,13 @@\n pub fn demo_{slug}_{index}() {{\n-    let status = \"before\";\n+    let status = \"after\";\n+    let repository = \"{}\";\n+    let title = \"{}\";\n     assert!(!status.is_empty());\n+    assert!(!repository.is_empty());\n+    assert!(!title.is_empty());\n }}\n",
                    scenario.repository,
                    scenario.title.replace('"', "'")
                ));
            }
        }
    }

    diff
}

static EXTENDED_DEMO_SCENARIOS: &[ExtendedDemoScenario] = &[
    ExtendedDemoScenario {
        repository: "remiss/review-core",
        number: 201,
        title: "Cache review route waypoints per PR",
        body: "Persists route waypoints so the reviewer can close a PR and resume the same reading path.",
        author_login: "nora",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-24T07:55:00Z",
        created_at: "2026-05-23T18:20:00Z",
        head_ref_name: "nora/route-waypoint-cache",
        labels: &["review-core", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN, "riku"],
        paths: &[
            "src/review_routes.rs",
            "src/review_session.rs",
            "tests/review_routes_test.rs",
        ],
        additions: 188,
        deletions: 37,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Vouched,
        triage_label: "vouched",
        triage_detail: Some("VOUCHED.td"),
        issue_comment: Some("demo-reviewer this should make resume behavior easy to inspect."),
        thread_comment: Some("The route cache needs to invalidate when the head oid changes."),
        pending_comment: Some("Draft: add a restart check for the selected route stop."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/review-core",
        number: 202,
        title: "Collapse duplicate review threads in combined diff",
        body: "Normalizes thread anchors so Copilot-style duplicate comments only render once.",
        author_login: "eli",
        author_association: "CONTRIBUTOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-24T07:10:00Z",
        created_at: "2026-05-23T15:00:00Z",
        head_ref_name: "eli/dedupe-review-threads",
        labels: &["review-comments", "mentioned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &[
            "src/review_anchors.rs",
            "src/views/diff_view/review_comments.rs",
            "src/diff.rs",
        ],
        additions: 214,
        deletions: 51,
        latest_review_state: Some("CHANGES_REQUESTED"),
        triage_kind: PullRequestTriageSignalKind::PriorContributor,
        triage_label: "prior contributor",
        triage_detail: None,
        issue_comment: Some("The old duplicate rendering was easiest to see in combined diff."),
        thread_comment: Some("Please keep the canonical anchor stable for pending comments too."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/review-core",
        number: 203,
        title: "Keep Review Partner fallback visibly labeled",
        body: "Makes fallback summaries explicit so a model failure cannot masquerade as generated context.",
        author_login: "sam",
        author_association: "NONE",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-23T23:40:00Z",
        created_at: "2026-05-23T11:00:00Z",
        head_ref_name: "sam/visible-review-partner-fallback",
        labels: &["review-ai", "fallback", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/review_partner.rs", "src/views/diff_view/guided_review.rs"],
        additions: 132,
        deletions: 28,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::TrustUnknown,
        triage_label: "trust unknown",
        triage_detail: None,
        issue_comment: Some("This is useful demo data for fallback and warning states."),
        thread_comment: Some("Fallback copy should be visible in both the panel and rail."),
        pending_comment: Some("Draft: verify cached fallback docs are not reused as success."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/workspace",
        number: 88,
        title: "Make workspace sync notification-free in demo mode",
        body: "Avoids macOS notifications while the app is serving deterministic local data.",
        author_login: DEMO_VIEWER_LOGIN,
        author_association: "OWNER",
        is_draft: false,
        review_decision: None,
        updated_at: "2026-05-23T22:05:00Z",
        created_at: "2026-05-23T20:00:00Z",
        head_ref_name: "demo/notification-free-sync",
        labels: &["workspace", "authored-demo"],
        reviewers: &["riku"],
        paths: &["src/notifications.rs", "src/views/workspace_sync.rs"],
        additions: 54,
        deletions: 9,
        latest_review_state: Some("APPROVED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("owner"),
        issue_comment: None,
        thread_comment: Some("Good place to verify demo mode never posts system notifications."),
        pending_comment: None,
        resolved_thread: true,
    },
    ExtendedDemoScenario {
        repository: "remiss/workspace",
        number: 91,
        title: "Defer shader board warmup until route settles",
        body: "Keeps the review board from doing expensive shader work before the first queue is visible.",
        author_login: "min",
        author_association: "FIRST_TIME_CONTRIBUTOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-23T20:30:00Z",
        created_at: "2026-05-22T17:25:00Z",
        head_ref_name: "min/lazy-board-shaders",
        labels: &["performance", "ui", "mentioned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/views/root.rs", "src/shader_surface.rs"],
        additions: 92,
        deletions: 20,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::FirstTimeContributor,
        triage_label: "first-time contributor",
        triage_detail: None,
        issue_comment: Some("demo-reviewer the first-paint behavior is the thing to inspect here."),
        thread_comment: Some("Request animation only after the static placeholder has painted."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/lsp",
        number: 64,
        title: "Lazy-load LSP references from hover disclosure",
        body: "Moves expensive references behind an explicit disclosure in the hover popup.",
        author_login: "lin",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-23T19:15:00Z",
        created_at: "2026-05-22T13:00:00Z",
        head_ref_name: "lin/lazy-lsp-references",
        labels: &["lsp", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/code_display.rs", "src/lsp.rs", "src/source_browser.rs"],
        additions: 174,
        deletions: 46,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("collaborator"),
        issue_comment: Some("The hover should feel instant until the references disclosure opens."),
        thread_comment: Some("Make sure loading state is scoped to the exact symbol and file."),
        pending_comment: Some("Draft: test the no-LSP fallback path too."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/lsp",
        number: 65,
        title: "Handle rust-analyzer shutdown in process groups",
        body: "Terminates managed language-server process groups when review tabs close.",
        author_login: "ava",
        author_association: "CONTRIBUTOR",
        is_draft: false,
        review_decision: Some("APPROVED"),
        updated_at: "2026-05-23T17:48:00Z",
        created_at: "2026-05-21T09:40:00Z",
        head_ref_name: "ava/lsp-process-groups",
        labels: &["lsp", "processes"],
        reviewers: &["riku"],
        paths: &["src/lsp.rs", "src/process_group.rs"],
        additions: 126,
        deletions: 33,
        latest_review_state: Some("APPROVED"),
        triage_kind: PullRequestTriageSignalKind::PriorContributor,
        triage_label: "prior contributor",
        triage_detail: None,
        issue_comment: None,
        thread_comment: Some("This resolved thread demonstrates the reopen action in demo mode."),
        pending_comment: None,
        resolved_thread: true,
    },
    ExtendedDemoScenario {
        repository: "remiss/design",
        number: 33,
        title: "Unify dark sidebar and diff surfaces",
        body: "Moves shared chrome toward the darker diff-view palette.",
        author_login: "riku",
        author_association: "OWNER",
        is_draft: false,
        review_decision: None,
        updated_at: "2026-05-23T16:35:00Z",
        created_at: "2026-05-22T18:20:00Z",
        head_ref_name: "riku/unified-dark-chrome",
        labels: &["design", "authored-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/theme.rs", "src/views/root.rs", "src/views/sections.rs"],
        additions: 88,
        deletions: 74,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("owner"),
        issue_comment: Some("The collapsed sidebar seam is intentionally represented here."),
        thread_comment: Some("Verify the icon rail and workspace header share the same canvas tone."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/design",
        number: 34,
        title: "Resize filter dialog at compact widths",
        body: "Keeps filter preset controls usable in narrow windows.",
        author_login: "sol",
        author_association: "NONE",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-22T22:50:00Z",
        created_at: "2026-05-22T08:00:00Z",
        head_ref_name: "sol/filter-dialog-compact",
        labels: &["design", "filters", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &[
            "src/views/sections/pull_request_filters.rs",
            "src/selectable_text.rs",
        ],
        additions: 76,
        deletions: 12,
        latest_review_state: Some("CHANGES_REQUESTED"),
        triage_kind: PullRequestTriageSignalKind::NoTrustList,
        triage_label: "no trust list",
        triage_detail: None,
        issue_comment: Some("Small-width screenshots should make this easy to verify."),
        thread_comment: Some("The filter name input should keep native text-edit shortcuts."),
        pending_comment: Some("Draft: check 320px width before merging."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/local-changes",
        number: 111,
        title: "Persist local agent request filters",
        body: "Adds saved filters for local agent requests without mixing them with GitHub comments.",
        author_login: "jon",
        author_association: "CONTRIBUTOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-22T18:45:00Z",
        created_at: "2026-05-21T14:00:00Z",
        head_ref_name: "jon/local-agent-request-filters",
        labels: &["local-changes", "filters", "mentioned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &[
            "src/agent_requests.rs",
            "src/review_session.rs",
            "src/views/diff_view/review_comments.rs",
        ],
        additions: 260,
        deletions: 41,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::PriorContributor,
        triage_label: "prior contributor",
        triage_detail: None,
        issue_comment: Some("demo-reviewer this should not create GitHub-side pending review state."),
        thread_comment: Some("Keep local agent request state under the review-session document."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/local-changes",
        number: 112,
        title: "Export agent session bundle after save",
        body: "Writes the active local agent session bundle after local request edits.",
        author_login: DEMO_VIEWER_LOGIN,
        author_association: "OWNER",
        is_draft: true,
        review_decision: None,
        updated_at: "2026-05-22T15:12:00Z",
        created_at: "2026-05-22T11:30:00Z",
        head_ref_name: "demo/export-agent-session",
        labels: &["local-changes", "authored-demo"],
        reviewers: &["riku"],
        paths: &["src/local_review.rs", "src/review_partner/bundle.rs"],
        additions: 118,
        deletions: 8,
        latest_review_state: None,
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("owner"),
        issue_comment: Some("The exported bundle opens cleanly after a restart."),
        thread_comment: Some("Keep this path deterministic so support bundles can be compared between runs."),
        pending_comment: Some("Draft: bundle path should be deterministic for test snapshots."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/release",
        number: 18,
        title: "Update Sparkle signing checklist",
        body: "Refreshes release checklist steps for signing and notarization.",
        author_login: "riku",
        author_association: "OWNER",
        is_draft: false,
        review_decision: None,
        updated_at: "2026-05-21T21:30:00Z",
        created_at: "2026-05-21T17:00:00Z",
        head_ref_name: "riku/sparkle-checklist",
        labels: &["release", "docs", "authored-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &[
            "docs/release-checklist.md",
            "scripts/package-app.sh",
            "scripts/notarize-app.sh",
        ],
        additions: 66,
        deletions: 14,
        latest_review_state: Some("APPROVED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("owner"),
        issue_comment: Some("This card gives the authored queue a release-shaped PR."),
        thread_comment: Some("The checklist should keep Apple-account-only steps clearly separated."),
        pending_comment: None,
        resolved_thread: true,
    },
    ExtendedDemoScenario {
        repository: "remiss/release",
        number: 19,
        title: "Harden updater error messaging",
        body: "Improves Settings update errors when the appcast cannot be read.",
        author_login: "maya",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-21T19:18:00Z",
        created_at: "2026-05-21T12:20:00Z",
        head_ref_name: "maya/updater-errors",
        labels: &["release", "settings", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/platform_macos.rs", "src/views/settings.rs"],
        additions: 84,
        deletions: 17,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("collaborator"),
        issue_comment: Some("The stale-network state is the one to inspect."),
        thread_comment: Some("Avoid implying an update exists when only the appcast failed."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/platform",
        number: 54,
        title: "Add file URL deep-link recovery",
        body: "Accepts file URL launch requests and maps them back into review context when possible.",
        author_login: "ash",
        author_association: "FIRST_TIME_CONTRIBUTOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-21T16:55:00Z",
        created_at: "2026-05-20T09:20:00Z",
        head_ref_name: "ash/file-url-recovery",
        labels: &["platform", "deep-link", "mentioned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/deep_link.rs", "src/platform_macos.rs"],
        additions: 102,
        deletions: 29,
        latest_review_state: Some("CHANGES_REQUESTED"),
        triage_kind: PullRequestTriageSignalKind::FirstTimeContributor,
        triage_label: "first-time contributor",
        triage_detail: None,
        issue_comment: Some("demo-reviewer this exercises first-time contributor and changes-requested badges."),
        thread_comment: Some("Reject unsupported schemes before parsing file paths."),
        pending_comment: Some("Draft: add a Windows-path rejection fixture."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/platform",
        number: 55,
        title: "Debounce duplicate macOS open-url events",
        body: "Suppresses duplicate URL events delivered through multiple macOS app launch paths.",
        author_login: "lin",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("APPROVED"),
        updated_at: "2026-05-21T14:22:00Z",
        created_at: "2026-05-20T18:20:00Z",
        head_ref_name: "lin/debounce-open-url",
        labels: &["platform", "deep-link"],
        reviewers: &["riku"],
        paths: &["src/deep_link.rs", "src/main.rs"],
        additions: 68,
        deletions: 11,
        latest_review_state: Some("APPROVED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("collaborator"),
        issue_comment: None,
        thread_comment: Some("Resolved demo thread for approved PR styling."),
        pending_comment: None,
        resolved_thread: true,
    },
    ExtendedDemoScenario {
        repository: "remiss/review-ai",
        number: 144,
        title: "Split Review Brief retry prompt",
        body: "Adds a stricter retry prompt for overlong review brief responses.",
        author_login: "nora",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-20T22:18:00Z",
        created_at: "2026-05-20T14:40:00Z",
        head_ref_name: "nora/review-brief-retry-prompt",
        labels: &["review-ai", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/review_brief.rs", "src/agents/prompt.rs"],
        additions: 156,
        deletions: 44,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Vouched,
        triage_label: "vouched",
        triage_detail: Some("VOUCHED.td"),
        issue_comment: Some("This is a compact Review Brief regression case."),
        thread_comment: Some("Retry prompt should restate field limits without changing schema."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/review-ai",
        number: 145,
        title: "Cache stack title polish separately",
        body: "Keeps core stack generation usable when optional title polish fails.",
        author_login: "eli",
        author_association: "CONTRIBUTOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-20T18:02:00Z",
        created_at: "2026-05-19T21:40:00Z",
        head_ref_name: "eli/stack-title-polish-cache",
        labels: &["review-ai", "large", "mentioned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &[
            "src/stacks/title_polish.rs",
            "src/review_partner.rs",
            "src/stacks/cache.rs",
        ],
        additions: 884,
        deletions: 188,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::PriorContributor,
        triage_label: "prior contributor",
        triage_detail: None,
        issue_comment: Some("Large PR fixture for filter and Review Partner stress states."),
        thread_comment: Some("Title polish must stay best-effort and separately cached."),
        pending_comment: Some("Draft: add cache-version note before merging."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/review-ai",
        number: 146,
        title: "Move generated context bundles outside checkout",
        body: "Writes Review Partner context artifacts outside the inspected checkout tree.",
        author_login: "sam",
        author_association: "NONE",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-20T15:34:00Z",
        created_at: "2026-05-19T11:45:00Z",
        head_ref_name: "sam/context-bundle-outside-checkout",
        labels: &["review-ai", "bundle", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/review_partner/bundle.rs", "src/review_context.rs"],
        additions: 144,
        deletions: 21,
        latest_review_state: Some("CHANGES_REQUESTED"),
        triage_kind: PullRequestTriageSignalKind::TrustUnknown,
        triage_label: "trust unknown",
        triage_detail: None,
        issue_comment: Some("This should be visible as an unknown-trust review request."),
        thread_comment: Some("Bundle writes should never modify the checkout under review."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/filters",
        number: 23,
        title: "Add denounced contributor filter chip",
        body: "Adds a high-friction triage filter for explicitly denounced contributors.",
        author_login: "badactor",
        author_association: "NONE",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-20T12:08:00Z",
        created_at: "2026-05-20T06:30:00Z",
        head_ref_name: "badactor/denounced-filter-chip",
        labels: &["filters", "triage"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/review_filters.rs", "src/triage.rs"],
        additions: 72,
        deletions: 10,
        latest_review_state: Some("CHANGES_REQUESTED"),
        triage_kind: PullRequestTriageSignalKind::Denounced,
        triage_label: "denounced",
        triage_detail: Some("Demo trust list entry"),
        issue_comment: Some("Denounced demo row for queue filtering and badge color checks."),
        thread_comment: Some("This should be easy to isolate with the denounced filter."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/filters",
        number: 24,
        title: "Save compound preset groups",
        body: "Lets users stack saved presets with ad hoc queue filters.",
        author_login: "maya",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-20T09:14:00Z",
        created_at: "2026-05-19T16:15:00Z",
        head_ref_name: "maya/compound-presets",
        labels: &["filters", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &[
            "src/review_filters.rs",
            "src/views/sections/pull_request_filters.rs",
        ],
        additions: 110,
        deletions: 16,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("collaborator"),
        issue_comment: Some("Compound filters need realistic overlap in demo queues."),
        thread_comment: Some("Toggling All should still clear every compound facet."),
        pending_comment: Some("Draft: test deleting an active compound custom preset."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/docs",
        number: 13,
        title: "Document keyboard review navigation",
        body: "Explains review-mode movement, file chooser, and next-comment navigation.",
        author_login: DEMO_VIEWER_LOGIN,
        author_association: "OWNER",
        is_draft: false,
        review_decision: None,
        updated_at: "2026-05-19T22:48:00Z",
        created_at: "2026-05-19T20:00:00Z",
        head_ref_name: "demo/keyboard-navigation-docs",
        labels: &["docs", "authored-demo"],
        reviewers: &["riku"],
        paths: &["README.md", "docs/review-navigation.md"],
        additions: 36,
        deletions: 4,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("owner"),
        issue_comment: None,
        thread_comment: Some("Keep shortcut docs out of visible onboarding copy."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/docs",
        number: 14,
        title: "Add troubleshooting page for LSP setup",
        body: "Documents common missing-server and repair states.",
        author_login: "quinn",
        author_association: "NONE",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-19T17:25:00Z",
        created_at: "2026-05-19T12:10:00Z",
        head_ref_name: "quinn/lsp-troubleshooting-docs",
        labels: &["docs", "lsp", "mentioned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["docs/troubleshooting-lsp.md", "README.md"],
        additions: 58,
        deletions: 5,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::NoTrustList,
        triage_label: "no trust list",
        triage_detail: None,
        issue_comment: Some("demo-reviewer good docs row for NoTrustList styling."),
        thread_comment: Some("Avoid promising automatic repair for externally installed servers."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/perf",
        number: 71,
        title: "Virtualize combined diff hydration queue",
        body: "Batches combined diff file loading so very large PRs stay responsive.",
        author_login: "rui",
        author_association: "CONTRIBUTOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-18T23:40:00Z",
        created_at: "2026-05-18T11:00:00Z",
        head_ref_name: "rui/combined-diff-hydration",
        labels: &["performance", "large", "assigned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &[
            "src/views/diff_view/combined_diff.rs",
            "src/views/diff_view/file_content.rs",
            "src/state.rs",
        ],
        additions: 940,
        deletions: 210,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::PriorContributor,
        triage_label: "prior contributor",
        triage_detail: None,
        issue_comment: Some("Large performance PR for scrolling and hydration demos."),
        thread_comment: Some("Stale hydration results should not apply after switching PRs."),
        pending_comment: Some("Draft: measure first visible file load before approving."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/perf",
        number: 72,
        title: "Skip semantic warmup for binary-only PRs",
        body: "Avoids semantic-review work when all changed files are binary or generated assets.",
        author_login: "tin",
        author_association: "NONE",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-18T18:16:00Z",
        created_at: "2026-05-18T08:45:00Z",
        head_ref_name: "tin/binary-only-warmup-skip",
        labels: &["performance", "semantic-review"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/semantic_review.rs", "assets/brand/remiss-app-icon.png"],
        additions: 44,
        deletions: 6,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::NoTrustList,
        triage_label: "no trust list",
        triage_detail: None,
        issue_comment: Some("This row stands in for binary-heavy PR behavior."),
        thread_comment: Some("The skip should be based on file metadata, not path suffix alone."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/security",
        number: 8,
        title: "Avoid logging prompt bodies in diagnostics",
        body: "Adds redaction around AI diagnostics and keeps prompt bodies out of exported logs.",
        author_login: "riku",
        author_association: "OWNER",
        is_draft: false,
        review_decision: None,
        updated_at: "2026-05-18T14:36:00Z",
        created_at: "2026-05-18T09:15:00Z",
        head_ref_name: "riku/redact-prompt-diagnostics",
        labels: &["security", "diagnostics", "authored-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/sentry_diagnostics.rs", "src/diagnostic_logs.rs"],
        additions: 92,
        deletions: 19,
        latest_review_state: Some("APPROVED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("owner"),
        issue_comment: Some("Security-shaped authored PR for demo queue density."),
        thread_comment: Some("Do not include model request payloads in exported archives."),
        pending_comment: None,
        resolved_thread: true,
    },
    ExtendedDemoScenario {
        repository: "remiss/security",
        number: 9,
        title: "Redact repository paths in exported bundles",
        body: "Normalizes local absolute paths before diagnostic bundle export.",
        author_login: "zev",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("REVIEW_REQUIRED"),
        updated_at: "2026-05-18T11:02:00Z",
        created_at: "2026-05-17T15:50:00Z",
        head_ref_name: "zev/redact-bundle-paths",
        labels: &["security", "diagnostics", "mentioned-demo"],
        reviewers: &[DEMO_VIEWER_LOGIN],
        paths: &["src/diagnostic_logs.rs", "src/review_partner/bundle.rs"],
        additions: 74,
        deletions: 12,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("collaborator"),
        issue_comment: Some("demo-reviewer path redaction needs a quick manual scan."),
        thread_comment: Some("Keep relative repository paths useful while hiding local roots."),
        pending_comment: None,
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/onboarding",
        number: 5,
        title: "Keep welcome tutorial out of workspace queues",
        body: "Ensures the synthetic tutorial PR never pollutes real workspace queues.",
        author_login: DEMO_VIEWER_LOGIN,
        author_association: "OWNER",
        is_draft: false,
        review_decision: None,
        updated_at: "2026-05-17T20:44:00Z",
        created_at: "2026-05-17T14:00:00Z",
        head_ref_name: "demo/tutorial-queue-isolation",
        labels: &["onboarding", "authored-demo"],
        reviewers: &["riku"],
        paths: &["src/onboarding.rs", "src/tutorial_pr.rs", "src/state.rs"],
        additions: 104,
        deletions: 22,
        latest_review_state: Some("COMMENTED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("owner"),
        issue_comment: None,
        thread_comment: Some("The tutorial detail state should be injected only while the wizard is active."),
        pending_comment: Some("Draft: run with demo mode and force wizard together."),
        resolved_thread: false,
    },
    ExtendedDemoScenario {
        repository: "remiss/onboarding",
        number: 6,
        title: "Show local review step only when available",
        body: "Hides unavailable onboarding targets without skipping forced tutorial flows.",
        author_login: "lin",
        author_association: "COLLABORATOR",
        is_draft: false,
        review_decision: Some("APPROVED"),
        updated_at: "2026-05-17T18:12:00Z",
        created_at: "2026-05-17T10:00:00Z",
        head_ref_name: "lin/onboarding-target-availability",
        labels: &["onboarding"],
        reviewers: &["riku"],
        paths: &["src/onboarding.rs", "src/views/welcome_wizard.rs"],
        additions: 62,
        deletions: 18,
        latest_review_state: Some("APPROVED"),
        triage_kind: PullRequestTriageSignalKind::Trusted,
        triage_label: "trusted",
        triage_detail: Some("collaborator"),
        issue_comment: None,
        thread_comment: Some("Approved demo row with onboarding code context."),
        pending_comment: None,
        resolved_thread: true,
    },
];

fn build_demo_pr(
    repository: &str,
    number: i64,
    title: &str,
    body: &str,
    author_login: &str,
    author_association: &str,
    is_draft: bool,
    review_decision: Option<&str>,
    updated_at: &str,
    created_at: &str,
    base_ref_name: &str,
    head_ref_name: &str,
    base_ref_oid: &str,
    head_ref_oid: &str,
    labels: &[&str],
    reviewers: &[&str],
    files: Vec<PullRequestFile>,
    raw_diff: String,
    comments: Vec<PullRequestComment>,
    latest_reviews: Vec<PullRequestReview>,
    review_threads: Vec<PullRequestReviewThread>,
    viewer_pending_review: Option<PendingPullRequestReview>,
    triage_signals: Vec<PullRequestTriageSignal>,
) -> DemoPullRequest {
    let commits = vec![
        commit(
            repository,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "Shape demo fixture",
        ),
        commit(
            repository,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "Wire UI behavior",
        ),
    ];
    let comments_count = comments.len() as i64
        + review_threads
            .iter()
            .map(|thread| thread.comments.len() as i64)
            .sum::<i64>()
        + viewer_pending_review
            .as_ref()
            .map(|review| review.comments.len() as i64)
            .unwrap_or(0);
    let additions = files.iter().map(|file| file.additions).sum();
    let deletions = files.iter().map(|file| file.deletions).sum();
    let url = format!("https://github.com/{repository}/pull/{number}");
    let parsed_diff = parse_unified_diff(&raw_diff);
    let detail = PullRequestDetail {
        id: format!("DEMO_{repository}_{number}"),
        repository: repository.to_string(),
        number,
        title: title.to_string(),
        body: body.to_string(),
        url: url.clone(),
        author_login: author_login.to_string(),
        author_avatar_url: None,
        state: "OPEN".to_string(),
        is_draft,
        review_decision: review_decision.map(str::to_string),
        base_ref_name: base_ref_name.to_string(),
        head_ref_name: head_ref_name.to_string(),
        base_ref_oid: Some(base_ref_oid.to_string()),
        head_ref_oid: Some(head_ref_oid.to_string()),
        additions,
        deletions,
        changed_files: files.len() as i64,
        comments_count,
        commits_count: commits.len() as i64,
        created_at: created_at.to_string(),
        updated_at: updated_at.to_string(),
        labels: labels.iter().map(|label| label.to_string()).collect(),
        reviewers: reviewers
            .iter()
            .map(|reviewer| reviewer.to_string())
            .collect(),
        reviewer_avatar_urls: BTreeMap::new(),
        comments,
        commits,
        latest_reviews,
        review_threads,
        viewer_pending_review,
        files,
        raw_diff,
        parsed_diff,
        data_completeness: PullRequestDataCompleteness::default(),
    };
    let summary = PullRequestSummary {
        local_key: None,
        repository: repository.to_string(),
        number,
        title: title.to_string(),
        author_login: author_login.to_string(),
        author_avatar_url: None,
        is_draft,
        comments_count,
        additions,
        deletions,
        changed_files: detail.changed_files,
        state: "OPEN".to_string(),
        author_association: author_association.to_string(),
        review_decision: review_decision.map(str::to_string),
        updated_at: updated_at.to_string(),
        url,
        repository_default_branch: Some(base_ref_name.to_string()),
        triage_signals,
    };

    DemoPullRequest { summary, detail }
}

fn demo_files(files: &[DemoFile]) -> Vec<PullRequestFile> {
    files
        .iter()
        .map(|file| PullRequestFile {
            path: file.path.to_string(),
            additions: file.additions,
            deletions: file.deletions,
            change_type: file.change_type.to_string(),
        })
        .collect()
}

fn signal(
    kind: PullRequestTriageSignalKind,
    label: &str,
    detail: Option<&str>,
) -> PullRequestTriageSignal {
    PullRequestTriageSignal {
        kind,
        label: label.to_string(),
        detail: detail.map(str::to_string),
    }
}

fn issue_comment(
    id: &str,
    author_login: &str,
    body: &str,
    updated_at: &str,
    repository: &str,
    number: i64,
) -> PullRequestComment {
    PullRequestComment {
        id: id.to_string(),
        author_login: author_login.to_string(),
        author_avatar_url: None,
        body: body.to_string(),
        created_at: updated_at.to_string(),
        updated_at: updated_at.to_string(),
        url: format!("https://github.com/{repository}/pull/{number}#issuecomment-{id}"),
    }
}

fn review(
    id: &str,
    author_login: &str,
    state: &str,
    body: &str,
    submitted_at: &str,
) -> PullRequestReview {
    PullRequestReview {
        id: Some(id.to_string()),
        author_login: author_login.to_string(),
        author_avatar_url: None,
        state: state.to_string(),
        body: body.to_string(),
        submitted_at: Some(submitted_at.to_string()),
    }
}

fn review_thread(
    id: &str,
    path: &str,
    line: i64,
    body: &str,
    author_login: &str,
    resolved: bool,
    updated_at: &str,
) -> PullRequestReviewThread {
    PullRequestReviewThread {
        id: id.to_string(),
        path: path.to_string(),
        line: Some(line),
        original_line: None,
        start_line: None,
        original_start_line: None,
        diff_side: "RIGHT".to_string(),
        start_diff_side: None,
        is_collapsed: false,
        is_outdated: false,
        is_resolved: resolved,
        subject_type: "LINE".to_string(),
        resolved_by_login: resolved.then(|| DEMO_VIEWER_LOGIN.to_string()),
        viewer_can_reply: true,
        viewer_can_resolve: !resolved,
        viewer_can_unresolve: resolved,
        comments: vec![review_comment(
            &format!("{id}-comment"),
            author_login,
            body,
            path,
            line,
            None,
            "SUBMITTED",
            updated_at,
        )],
    }
}

fn pending_review(comments: Vec<PullRequestReviewComment>) -> PendingPullRequestReview {
    PendingPullRequestReview {
        id: "demo-pending-review".to_string(),
        author_login: DEMO_VIEWER_LOGIN.to_string(),
        author_avatar_url: None,
        body: String::new(),
        comments,
    }
}

fn review_comment(
    id: &str,
    author_login: &str,
    body: &str,
    path: &str,
    line: i64,
    reply_to_id: Option<&str>,
    state: &str,
    updated_at: &str,
) -> PullRequestReviewComment {
    PullRequestReviewComment {
        id: id.to_string(),
        author_login: author_login.to_string(),
        author_avatar_url: None,
        body: body.to_string(),
        path: path.to_string(),
        line: Some(line),
        original_line: None,
        start_line: None,
        original_start_line: None,
        state: state.to_string(),
        created_at: updated_at.to_string(),
        updated_at: updated_at.to_string(),
        published_at: (state != "PENDING").then(|| updated_at.to_string()),
        reply_to_id: reply_to_id.map(str::to_string),
        viewer_can_update: author_login == DEMO_VIEWER_LOGIN,
        viewer_can_delete: author_login == DEMO_VIEWER_LOGIN,
        url: format!("https://github.com/remiss/demo/pull/1#discussion-{id}"),
    }
}

fn commit(repository: &str, oid: &str, headline: &str) -> PullRequestCommit {
    PullRequestCommit {
        id: format!("DEMO_COMMIT_{oid}"),
        oid: oid.to_string(),
        abbreviated_oid: oid.chars().take(7).collect(),
        message_headline: headline.to_string(),
        committed_date: "2026-05-23T10:00:00Z".to_string(),
        author_name: Some("Demo Author".to_string()),
        author_login: Some("demo-author".to_string()),
        author_avatar_url: None,
        url: format!("https://github.com/{repository}/commit/{oid}"),
    }
}

fn source_content(repository: &str, path: &str) -> Option<String> {
    match (repository, path) {
        ("remiss/remiss", "src/review_filters.rs") => Some(FILTERS_SOURCE.to_string()),
        ("remiss/remiss", "src/views/sections/pull_request_filters.rs") => {
            Some(FILTER_PANEL_SOURCE.to_string())
        }
        ("remiss/remiss", "src/triage.rs") => Some(TRIAGE_SOURCE.to_string()),
        ("remiss/gpui-lab", "src/views/root.rs") => Some(ROOT_SOURCE.to_string()),
        ("remiss/gpui-lab", "src/views/motion.rs") => Some(MOTION_SOURCE.to_string()),
        ("remiss/local-changes", "src/local_review.rs") => Some(LOCAL_REVIEW_SOURCE.to_string()),
        ("remiss/local-changes", "src/review_session.rs") => {
            Some(REVIEW_SESSION_SOURCE.to_string())
        }
        ("remiss/local-changes", "src/views/diff_view/review_comments.rs") => {
            Some(REVIEW_COMMENTS_SOURCE.to_string())
        }
        ("remiss/docs", "README.md") => Some(README_SOURCE.to_string()),
        ("remiss/docs", "docs/release-checklist.md") => Some(RELEASE_CHECKLIST_SOURCE.to_string()),
        _ => generated_source_content(repository, path),
    }
}

fn generated_source_content(repository: &str, path: &str) -> Option<String> {
    let has_demo_file = demo_pull_requests().iter().any(|pull| {
        pull.detail.repository == repository
            && pull.detail.files.iter().any(|file| file.path == path)
    });
    if !has_demo_file {
        return None;
    }

    if path.ends_with(".md") {
        return Some(format!(
            "# {}\n\nThis demo document belongs to `{repository}` and exists so review surfaces have realistic file content.\n",
            path.rsplit('/').next().unwrap_or(path)
        ));
    }
    if path.ends_with(".sh") {
        return Some(format!(
            "#!/usr/bin/env bash\nset -euo pipefail\n\necho \"demo script for {repository}:{path}\"\n"
        ));
    }
    if path.ends_with(".png") || path.ends_with(".icns") {
        return Some(format!(
            "Demo text stand-in for binary asset {repository}:{path}.\n"
        ));
    }

    let symbol = path
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    Some(format!(
        "pub fn demo_source_{symbol}() -> &'static str {{\n    \"{repository}:{path}\"\n}}\n\npub fn demo_review_note() -> &'static str {{\n    \"This generated file gives demo mode enough source content for file loading, search, and review navigation.\"\n}}\n"
    ))
}

const FILTER_PRESETS_DIFF: &str = r#"diff --git a/src/review_filters.rs b/src/review_filters.rs
index 42a0000..42b0000 100644
--- a/src/review_filters.rs
+++ b/src/review_filters.rs
@@ -1,12 +1,18 @@
 pub struct PullRequestFilterSettings {
     pub active_preset_id: Option<String>,
     pub include_muted: bool,
+    pub custom_presets: Vec<PullRequestFilterPreset>,
 }
 
 impl PullRequestFilterSettings {
+    pub fn save_current_as_preset(&mut self, label: &str) -> Result<String, String> {
+        let id = format!("custom:{}", label.trim().to_ascii_lowercase().replace(' ', "-"));
+        self.custom_presets.push(PullRequestFilterPreset { id: id.clone(), label: label.trim().to_string() });
+        Ok(id)
+    }
 }
diff --git a/src/views/sections/pull_request_filters.rs b/src/views/sections/pull_request_filters.rs
new file mode 100644
index 0000000..42c0000
--- /dev/null
+++ b/src/views/sections/pull_request_filters.rs
@@ -0,0 +1,16 @@
+pub fn render_pull_request_filter_dialog() {
+    let title = "Save filter";
+    let helper = "Custom filters stay local to this machine.";
+    render_modal(title, helper);
+}
+
+fn render_modal(title: &str, helper: &str) {
+    println!("{title}: {helper}");
+}
diff --git a/src/triage.rs b/src/triage.rs
index 42d0000..42e0000 100644
--- a/src/triage.rs
+++ b/src/triage.rs
@@ -1,7 +1,11 @@
 pub enum PullRequestTriageSignalKind {
     Trusted,
+    Vouched,
     FirstTimeContributor,
 }
+
+pub fn badge_label(kind: PullRequestTriageSignalKind) -> &'static str {
+    match kind { PullRequestTriageSignalKind::Trusted => "trusted", PullRequestTriageSignalKind::Vouched => "vouched", PullRequestTriageSignalKind::FirstTimeContributor => "first-time contributor" }
+}
"#;

const SIDEBAR_POLISH_DIFF: &str = r#"diff --git a/src/views/root.rs b/src/views/root.rs
index 77a0000..77b0000 100644
--- a/src/views/root.rs
+++ b/src/views/root.rs
@@ -110,10 +110,16 @@ fn render_sidebar_item(label: &str, icon: Icon, collapsed: bool) {
     let label_visible = !collapsed;
     button()
-        .width(if collapsed { 24 } else { 168 })
+        .width(if collapsed { 32 } else { 168 })
+        .min_width(if collapsed { 32 } else { 168 })
         .icon(icon)
         .when(label_visible, |button| button.label(label));
 }
diff --git a/src/views/motion.rs b/src/views/motion.rs
index 77c0000..77d0000 100644
--- a/src/views/motion.rs
+++ b/src/views/motion.rs
@@ -1,6 +1,12 @@
 pub fn route_progress(raw: f32) -> f32 {
-    raw.clamp(0.0, 1.0)
+    ease_in_out(raw.clamp(0.0, 1.0))
 }
+
+fn ease_in_out(t: f32) -> f32 {
+    t * t * (3.0 - 2.0 * t)
+}
"#;

const LOCAL_CHANGES_DIFF: &str = r#"diff --git a/src/local_review.rs b/src/local_review.rs
index 108a000..108b000 100644
--- a/src/local_review.rs
+++ b/src/local_review.rs
@@ -40,6 +40,12 @@ pub struct LocalReviewInspection {
     pub repository: String,
     pub head_oid: String,
+    pub agent_session_key: String,
 }
+
+pub fn local_agent_session_key(repository: &str, head_oid: &str) -> String {
+    format!("local-agent:{repository}:{head_oid}")
+}
diff --git a/src/review_session.rs b/src/review_session.rs
index 108c000..108d000 100644
--- a/src/review_session.rs
+++ b/src/review_session.rs
@@ -80,6 +80,8 @@ pub struct ReviewSessionDocument {
     pub route: Vec<ReviewLocation>,
+    pub agent_requests: Vec<AgentRequest>,
 }
+
+pub struct AgentRequest { pub id: String, pub body: String }
diff --git a/src/views/diff_view/review_comments.rs b/src/views/diff_view/review_comments.rs
index 108e000..108f000 100644
--- a/src/views/diff_view/review_comments.rs
+++ b/src/views/diff_view/review_comments.rs
@@ -22,6 +22,10 @@ pub fn submit_inline_comment(target: ReviewLineActionTarget) {
     submit_github_comment(target);
 }
+
+pub fn submit_local_agent_request(target: ReviewLineActionTarget, body: String) {
+    save_agent_request(target, body);
+}
"#;

const DOCS_DIFF: &str = r#"diff --git a/README.md b/README.md
index 12a0000..12b0000 100644
--- a/README.md
+++ b/README.md
@@ -72,6 +72,10 @@ cargo test --all-features
 cargo run
 ```
+
+To run with local fixture data:
+
+```sh
+REMISS_DEMO_MODE=1 cargo run
+```
diff --git a/docs/release-checklist.md b/docs/release-checklist.md
new file mode 100644
index 0000000..12c0000
--- /dev/null
+++ b/docs/release-checklist.md
@@ -0,0 +1,7 @@
+# Release checklist
+
+- Run the Rust validation gates.
+- Launch demo mode and inspect Overview, Review, and Local Changes.
+- Confirm update metadata after publishing.
"#;

const FILTERS_SOURCE: &str = r#"pub struct PullRequestFilterPreset {
    pub id: String,
    pub label: String,
}

pub struct PullRequestFilterSettings {
    pub active_preset_id: Option<String>,
    pub include_muted: bool,
    pub custom_presets: Vec<PullRequestFilterPreset>,
}

impl PullRequestFilterSettings {
    pub fn save_current_as_preset(&mut self, label: &str) -> Result<String, String> {
        let label = label.trim();
        if label.is_empty() {
            return Err("Give the filter a name before saving.".to_string());
        }
        let id = format!("custom:{}", label.to_ascii_lowercase().replace(' ', "-"));
        self.custom_presets.push(PullRequestFilterPreset { id: id.clone(), label: label.to_string() });
        Ok(id)
    }
}
"#;

const FILTER_PANEL_SOURCE: &str = r#"pub fn render_pull_request_filter_dialog() {
    let title = "Save filter";
    let helper = "Custom filters stay local to this machine.";
    render_modal(title, helper);
}

fn render_modal(title: &str, helper: &str) {
    println!("{title}: {helper}");
}
"#;

const TRIAGE_SOURCE: &str = r#"pub enum PullRequestTriageSignalKind {
    Trusted,
    Vouched,
    FirstTimeContributor,
}

pub fn badge_label(kind: PullRequestTriageSignalKind) -> &'static str {
    match kind {
        PullRequestTriageSignalKind::Trusted => "trusted",
        PullRequestTriageSignalKind::Vouched => "vouched",
        PullRequestTriageSignalKind::FirstTimeContributor => "first-time contributor",
    }
}
"#;

const ROOT_SOURCE: &str = r#"pub fn render_sidebar_item(label: &str, icon: Icon, collapsed: bool) {
    let label_visible = !collapsed;
    button()
        .width(if collapsed { 32 } else { 168 })
        .min_width(if collapsed { 32 } else { 168 })
        .icon(icon)
        .when(label_visible, |button| button.label(label));
}
"#;

const MOTION_SOURCE: &str = r#"pub fn route_progress(raw: f32) -> f32 {
    ease_in_out(raw.clamp(0.0, 1.0))
}

fn ease_in_out(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
"#;

const LOCAL_REVIEW_SOURCE: &str = r#"pub struct LocalReviewInspection {
    pub repository: String,
    pub head_oid: String,
    pub agent_session_key: String,
}

pub fn local_agent_session_key(repository: &str, head_oid: &str) -> String {
    format!("local-agent:{repository}:{head_oid}")
}
"#;

const REVIEW_SESSION_SOURCE: &str = r#"pub struct ReviewSessionDocument {
    pub route: Vec<ReviewLocation>,
    pub agent_requests: Vec<AgentRequest>,
}

pub struct AgentRequest {
    pub id: String,
    pub body: String,
}
"#;

const REVIEW_COMMENTS_SOURCE: &str = r#"pub fn submit_inline_comment(target: ReviewLineActionTarget) {
    submit_github_comment(target);
}

pub fn submit_local_agent_request(target: ReviewLineActionTarget, body: String) {
    save_agent_request(target, body);
}
"#;

const README_SOURCE: &str = r#"## Development

For working on the repo locally:

```sh
cargo fmt --check
./scripts/check-line-budget.sh
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

To run with local fixture data:

```sh
REMISS_DEMO_MODE=1 cargo run
```
"#;

const RELEASE_CHECKLIST_SOURCE: &str = r#"# Release checklist

- Run the Rust validation gates.
- Launch demo mode and inspect Overview, Review, and Local Changes.
- Confirm update metadata after publishing.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_workspace_contains_populated_review_queues() {
        let workspace = workspace_snapshot();
        let unique_pull_requests = workspace
            .queues
            .iter()
            .flat_map(|queue| {
                queue
                    .items
                    .iter()
                    .map(|summary| format!("{}#{}", summary.repository, summary.number))
            })
            .collect::<std::collections::BTreeSet<_>>();

        assert!(workspace.auth.is_authenticated);
        assert_eq!(
            workspace
                .viewer
                .as_ref()
                .map(|viewer| viewer.login.as_str()),
            Some(DEMO_VIEWER_LOGIN)
        );
        assert_eq!(workspace.queues.len(), 5);
        assert!(unique_pull_requests.len() >= 24);
        assert!(workspace.queues.iter().all(|queue| queue.items.len() >= 6));
        assert!(workspace
            .queues
            .iter()
            .find(|queue| queue.id == "reviewRequested")
            .map(|queue| queue.items.len() >= 16)
            .unwrap_or(false));
        assert!(workspace
            .queues
            .iter()
            .flat_map(|queue| queue.items.iter())
            .any(|summary| summary
                .triage_signals
                .iter()
                .any(|signal| signal.kind == PullRequestTriageSignalKind::FirstTimeContributor)));
        assert!(workspace
            .queues
            .iter()
            .flat_map(|queue| queue.items.iter())
            .any(|summary| summary
                .triage_signals
                .iter()
                .any(|signal| signal.kind == PullRequestTriageSignalKind::Denounced)));
    }

    #[test]
    fn demo_detail_and_source_content_are_available_for_queue_items() {
        let detail = pull_request_detail_snapshot("remiss/remiss", 42)
            .and_then(|snapshot| snapshot.detail)
            .expect("demo detail");

        assert_eq!(detail.files.len(), 3);
        assert!(!detail.review_threads.is_empty());
        assert!(detail.viewer_pending_review.is_some());

        let document = pull_request_file_content(
            "remiss/remiss",
            detail.head_ref_oid.as_deref().unwrap_or("HEAD"),
            "src/review_filters.rs",
        )
        .expect("demo source");
        assert!(document
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("save_current_as_preset"));

        let generated_detail = pull_request_detail_snapshot("remiss/perf", 71)
            .and_then(|snapshot| snapshot.detail)
            .expect("generated demo detail");
        assert!(generated_detail.additions + generated_detail.deletions >= 1_000);
        assert!(!generated_detail.review_threads.is_empty());
        let generated_document = pull_request_file_content(
            "remiss/perf",
            generated_detail.head_ref_oid.as_deref().unwrap_or("HEAD"),
            "src/views/diff_view/combined_diff.rs",
        )
        .expect("generated demo source");
        assert!(generated_document
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("demo_source_src_views_diff_view_combined_diff_rs"));
    }

    #[test]
    fn demo_detail_snapshots_seed_overview_comment_buckets() {
        let details = pull_request_detail_snapshots()
            .into_iter()
            .filter_map(|snapshot| snapshot.detail)
            .collect::<Vec<_>>();

        let has_foreign_comment = |detail: &PullRequestDetail| {
            detail
                .comments
                .iter()
                .any(|comment| comment.author_login != DEMO_VIEWER_LOGIN)
                || detail
                    .review_threads
                    .iter()
                    .flat_map(|thread| &thread.comments)
                    .any(|comment| comment.author_login != DEMO_VIEWER_LOGIN)
        };
        let authored_with_foreign_comments = details
            .iter()
            .filter(|detail| detail.author_login == DEMO_VIEWER_LOGIN)
            .filter(|detail| has_foreign_comment(detail))
            .count();
        let other_with_foreign_comments = details
            .iter()
            .filter(|detail| detail.author_login != DEMO_VIEWER_LOGIN)
            .filter(|detail| has_foreign_comment(detail))
            .count();

        assert!(details.len() >= 30);
        assert!(authored_with_foreign_comments >= 5);
        assert!(other_with_foreign_comments >= 20);
    }

    #[test]
    fn demo_env_truthiness_accepts_common_run_values() {
        assert!(env_truthy("1"));
        assert!(env_truthy("true"));
        assert!(env_truthy("yes"));
        assert!(!env_truthy("0"));
        assert!(!env_truthy("false"));
        assert!(!env_truthy("off"));
        assert!(!env_truthy(" "));
    }

    #[test]
    fn screenshot_mode_implies_demo_mode() {
        let value = |name: &str| {
            (name == crate::screenshot_mode::SCREENSHOT_MODE_ENV).then(|| "1".to_string())
        };

        assert!(demo_mode_enabled_from_vars(value));
    }
}
