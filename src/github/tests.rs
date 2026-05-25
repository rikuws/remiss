use super::*;
use serde_json::json;

#[test]
fn maps_pull_request_summary_for_fast_url_open() {
    let node = json!({
        "number": 42,
        "title": "Improve PR loading",
        "url": "https://github.com/acme/repo/pull/42",
        "state": "OPEN",
        "isDraft": false,
        "updatedAt": "2026-05-22T08:00:00Z",
        "additions": 120,
        "deletions": 34,
        "changedFiles": 9,
        "authorAssociation": "COLLABORATOR",
        "reviewDecision": "REVIEW_REQUIRED",
        "author": { "login": "rikuws", "avatarUrl": "https://example.com/avatar.png" },
        "comments": { "totalCount": 6 },
        "repository": { "nameWithOwner": "acme/repo", "defaultBranchRef": { "name": "main" } }
    });

    let summary = map_pull_request_summary(&node).expect("summary");

    assert_eq!(summary.repository, "acme/repo");
    assert_eq!(summary.number, 42);
    assert_eq!(summary.title, "Improve PR loading");
    assert_eq!(summary.author_login, "rikuws");
    assert_eq!(summary.comments_count, 6);
    assert_eq!(summary.additions, 120);
    assert_eq!(summary.deletions, 34);
    assert_eq!(summary.changed_files, 9);
    assert_eq!(summary.author_association, "COLLABORATOR");
    assert_eq!(summary.review_decision.as_deref(), Some("REVIEW_REQUIRED"));
    assert_eq!(summary.repository_default_branch.as_deref(), Some("main"));
}

#[test]
fn maps_pull_request_commit_with_linked_user_author() {
    let node = json!({
        "id": "PRC_1",
        "commit": {
            "id": "C_kwDOcommit",
            "oid": "5f34fac6c995fdf1197dc62bfe2cd0f88360baca",
            "abbreviatedOid": "5f34fac",
            "messageHeadline": "Split local Git actions out of root view",
            "committedDate": "2026-05-18T06:28:00Z",
            "url": "https://github.com/acme/repo/commit/5f34fac",
            "author": {
                "name": "Riku Wikman",
                "avatarUrl": "https://example.com/git-avatar.png",
                "user": {
                    "login": "rikuws",
                    "avatarUrl": "https://example.com/user-avatar.png"
                }
            }
        }
    });

    let commit = map_pull_request_commit(&node).expect("commit");

    assert_eq!(commit.id, "C_kwDOcommit");
    assert_eq!(commit.oid, "5f34fac6c995fdf1197dc62bfe2cd0f88360baca");
    assert_eq!(commit.abbreviated_oid, "5f34fac");
    assert_eq!(
        commit.message_headline,
        "Split local Git actions out of root view"
    );
    assert_eq!(commit.committed_date, "2026-05-18T06:28:00Z");
    assert_eq!(commit.author_name.as_deref(), Some("Riku Wikman"));
    assert_eq!(commit.author_login.as_deref(), Some("rikuws"));
    assert_eq!(
        commit.author_avatar_url.as_deref(),
        Some("https://example.com/user-avatar.png")
    );
    assert_eq!(commit.url, "https://github.com/acme/repo/commit/5f34fac");
}

#[test]
fn maps_pull_request_commit_author_without_linked_user() {
    let node = json!({
        "id": "PRC_2",
        "commit": {
            "id": "C_kwDOcommit2",
            "oid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abbreviatedOid": "aaaaaaa",
            "messageHeadline": "Initial implementation",
            "committedDate": "2026-05-18T05:00:00Z",
            "url": "https://github.com/acme/repo/commit/aaaaaaa",
            "author": {
                "name": "Local Author",
                "avatarUrl": "https://example.com/git-avatar.png",
                "user": null
            }
        }
    });

    let commit = map_pull_request_commit(&node).expect("commit");

    assert_eq!(commit.author_name.as_deref(), Some("Local Author"));
    assert_eq!(commit.author_login, None);
    assert_eq!(
        commit.author_avatar_url.as_deref(),
        Some("https://example.com/git-avatar.png")
    );
}

#[test]
fn normalizes_submitted_reviews_to_latest_by_author() {
    let latest = latest_reviews_by_author(vec![
        pull_request_review("alice", "CHANGES_REQUESTED", Some("2026-04-14T10:00:00Z")),
        pull_request_review("bob", "COMMENTED", Some("2026-04-14T12:00:00Z")),
        pull_request_review("alice", "APPROVED", Some("2026-04-14T11:00:00Z")),
        pull_request_review("carol", "DISMISSED", Some("2026-04-14T09:00:00Z")),
    ]);

    let by_author = latest
        .iter()
        .map(|review| (review.author_login.as_str(), review.state.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(by_author.get("alice").copied(), Some("APPROVED"));
    assert_eq!(by_author.get("bob").copied(), Some("COMMENTED"));
    assert_eq!(by_author.get("carol").copied(), Some("DISMISSED"));
}

#[test]
fn maps_viewer_pending_review_comments_and_permissions() {
    let node = json!({
        "id": "PRR_pending",
        "body": "",
        "author": { "login": "riku", "avatarUrl": "https://example.com/a.png" },
        "comments": {
            "nodes": [{
                "id": "PRRC_draft",
                "body": "Please use a guard here.",
                "path": "src/lib.rs",
                "line": 42,
                "originalLine": 42,
                "startLine": 40,
                "originalStartLine": 40,
                "state": "PENDING",
                "createdAt": "2026-05-13T10:00:00Z",
                "updatedAt": "2026-05-13T10:01:00Z",
                "publishedAt": null,
                "url": "https://github.com/acme/repo/pull/1#discussion_r1",
                "viewerCanUpdate": true,
                "viewerCanDelete": true,
                "replyTo": null,
                "author": { "login": "riku", "avatarUrl": null }
            }]
        }
    });

    let review = map_pending_pull_request_review(&node).expect("pending review");
    assert_eq!(review.id, "PRR_pending");
    assert_eq!(review.author_login, "riku");
    assert_eq!(review.comments.len(), 1);

    let comment = &review.comments[0];
    assert_eq!(comment.state, "PENDING");
    assert_eq!(comment.path, "src/lib.rs");
    assert_eq!(comment.line, Some(42));
    assert_eq!(comment.start_line, Some(40));
    assert!(comment.viewer_can_update);
    assert!(comment.viewer_can_delete);

    let mut threads = Vec::new();
    append_missing_pending_review_threads(&mut threads, &review);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].comments[0].id, "PRRC_draft");
    assert_eq!(threads[0].line, Some(42));
    assert_eq!(threads[0].start_line, Some(40));
}

#[test]
fn maps_review_thread_comments_once_by_id() {
    let comment = json!({
        "id": "PRRC_duplicate",
        "body": "Please handle this edge case.",
        "path": "src/lib.rs",
        "line": 42,
        "originalLine": 42,
        "startLine": null,
        "originalStartLine": null,
        "state": "PUBLISHED",
        "createdAt": "2026-05-13T10:00:00Z",
        "updatedAt": "2026-05-13T10:01:00Z",
        "publishedAt": "2026-05-13T10:02:00Z",
        "url": "https://github.com/acme/repo/pull/1#discussion_r1",
        "viewerCanUpdate": false,
        "viewerCanDelete": false,
        "replyTo": null,
        "author": { "login": "reviewer", "avatarUrl": null }
    });
    let node = json!({
        "id": "PRRT_thread",
        "path": "src/lib.rs",
        "line": 42,
        "originalLine": 42,
        "startLine": null,
        "originalStartLine": null,
        "diffSide": "RIGHT",
        "startDiffSide": null,
        "isCollapsed": false,
        "isOutdated": false,
        "isResolved": false,
        "subjectType": "LINE",
        "resolvedBy": null,
        "viewerCanReply": true,
        "viewerCanResolve": true,
        "viewerCanUnresolve": false,
        "comments": {
            "nodes": [comment.clone(), comment]
        }
    });

    let thread = map_review_thread(&node).expect("review thread");

    assert_eq!(thread.comments.len(), 1);
    assert_eq!(thread.comments[0].id, "PRRC_duplicate");
}

#[test]
fn reads_review_thread_reply_comment_payload() {
    let response = json!({
        "data": {
            "addPullRequestReviewThreadReply": {
                "comment": { "id": "PRRC_reply" }
            }
        }
    });

    assert_eq!(
        review_thread_reply_comment_id(&response),
        Some("PRRC_reply")
    );
}

#[test]
fn ignores_legacy_thread_reply_payload_shape() {
    let response = json!({
        "data": {
            "addPullRequestReviewThreadReply": {
                "thread": { "id": "PRRT_thread" }
            }
        }
    });

    assert_eq!(review_thread_reply_comment_id(&response), None);
}

#[test]
fn classifies_oversized_diff_fetch_errors_as_non_retryable() {
    assert!(is_non_retryable_diff_unavailable_error(
        "Failed to fetch diff for acme/repo#7: GraphQL: This diff is too large to display."
    ));
    assert!(is_non_retryable_diff_unavailable_error(
        "Failed to fetch diff for acme/repo#7: pull request contains too many files"
    ));
    assert!(!is_non_retryable_diff_unavailable_error(
        "Failed to fetch diff for acme/repo#7: HTTP 502: gateway timeout"
    ));
}

fn pull_request_review(
    author_login: &str,
    state: &str,
    submitted_at: Option<&str>,
) -> PullRequestReview {
    PullRequestReview {
        id: None,
        author_login: author_login.to_string(),
        author_avatar_url: None,
        state: state.to_string(),
        body: String::new(),
        submitted_at: submitted_at.map(str::to_string),
    }
}
