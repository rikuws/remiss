use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    cache::CacheStore,
    github::{
        self, PullRequestDetail, PullRequestReviewComment, PullRequestReviewThread,
        PullRequestSummary, WorkspaceSnapshot,
    },
    platform_macos,
    state::pr_key,
};

const NOTIFICATION_STATE_CACHE_KEY: &str = "notification-state-v1";

#[derive(Debug, Clone)]
pub struct WorkspaceSyncOutcome {
    pub workspace: WorkspaceSnapshot,
    pub notifications: Vec<SystemNotification>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemNotification {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedNotificationState {
    review_requested_pr_keys: Vec<String>,
    thread_last_comment_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationInput {
    review_requested_prs: Vec<TrackedPullRequest>,
    tracked_threads: Vec<TrackedReviewThread>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedPullRequest {
    pr_key: String,
    repository: String,
    number: i64,
    title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedReviewThread {
    id: String,
    pull_request: TrackedPullRequest,
    owner_login: String,
    comments: Vec<TrackedComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedComment {
    id: String,
    author_login: String,
    body: String,
}

struct NotificationEvaluation {
    state: PersistedNotificationState,
    notifications: Vec<SystemNotification>,
}

pub fn sync_workspace_with_notifications(
    cache: &CacheStore,
) -> Result<WorkspaceSyncOutcome, String> {
    let workspace = github::sync_workspace_snapshot(cache)?;
    let previous = cache
        .get::<PersistedNotificationState>(NOTIFICATION_STATE_CACHE_KEY)?
        .map(|document| document.value);
    let input = build_notification_input(cache, &workspace);
    let evaluation = evaluate_notifications(&input, previous.as_ref());
    cache.put(
        NOTIFICATION_STATE_CACHE_KEY,
        &evaluation.state,
        notification_timestamp_ms(),
    )?;

    Ok(WorkspaceSyncOutcome {
        workspace,
        notifications: evaluation.notifications,
    })
}

pub fn deliver_system_notifications(notifications: &[SystemNotification]) {
    for notification in notifications {
        if let Err(error) =
            platform_macos::deliver_system_notification(&notification.title, &notification.body)
        {
            eprintln!(
                "Failed to deliver system notification '{}': {error}",
                notification.title
            );
        }
    }
}

fn build_notification_input(
    cache: &CacheStore,
    workspace: &WorkspaceSnapshot,
) -> NotificationInput {
    let review_requested_prs = review_requested_pull_requests(workspace);
    let viewer_login = workspace
        .viewer
        .as_ref()
        .map(|viewer| viewer.login.as_str())
        .or(workspace.auth.active_login.as_deref())
        .unwrap_or_default()
        .to_string();

    let tracked_threads = review_requested_prs
        .iter()
        .flat_map(|pull_request| {
            match github::sync_pull_request_detail(
                cache,
                &pull_request.repository,
                pull_request.number,
            ) {
                Ok(snapshot) => snapshot
                    .detail
                    .as_ref()
                    .map(|detail| extract_tracked_threads(detail, pull_request, &viewer_login))
                    .unwrap_or_default(),
                Err(error) => {
                    eprintln!(
                        "Failed to load review threads for {}#{} notifications: {error}",
                        pull_request.repository, pull_request.number
                    );
                    Vec::new()
                }
            }
        })
        .collect();

    NotificationInput {
        review_requested_prs,
        tracked_threads,
    }
}

fn review_requested_pull_requests(workspace: &WorkspaceSnapshot) -> Vec<TrackedPullRequest> {
    workspace
        .queues
        .iter()
        .find(|queue| queue.id == "reviewRequested")
        .map(|queue| queue.items.iter().map(tracked_pull_request).collect())
        .unwrap_or_default()
}

fn tracked_pull_request(summary: &PullRequestSummary) -> TrackedPullRequest {
    TrackedPullRequest {
        pr_key: pr_key(&summary.repository, summary.number),
        repository: summary.repository.clone(),
        number: summary.number,
        title: summary.title.clone(),
    }
}

fn extract_tracked_threads(
    detail: &PullRequestDetail,
    pull_request: &TrackedPullRequest,
    viewer_login: &str,
) -> Vec<TrackedReviewThread> {
    detail
        .review_threads
        .iter()
        .filter_map(|thread| tracked_review_thread(thread, pull_request, viewer_login))
        .collect()
}

fn tracked_review_thread(
    thread: &PullRequestReviewThread,
    pull_request: &TrackedPullRequest,
    viewer_login: &str,
) -> Option<TrackedReviewThread> {
    let owner_login = thread.comments.first()?.author_login.clone();
    if owner_login != viewer_login {
        return None;
    }

    Some(TrackedReviewThread {
        id: thread.id.clone(),
        pull_request: pull_request.clone(),
        owner_login,
        comments: thread.comments.iter().map(tracked_comment).collect(),
    })
}

fn tracked_comment(comment: &PullRequestReviewComment) -> TrackedComment {
    TrackedComment {
        id: comment.id.clone(),
        author_login: comment.author_login.clone(),
        body: comment.body.clone(),
    }
}

fn evaluate_notifications(
    input: &NotificationInput,
    previous: Option<&PersistedNotificationState>,
) -> NotificationEvaluation {
    let next_state = PersistedNotificationState {
        review_requested_pr_keys: input
            .review_requested_prs
            .iter()
            .map(|pull_request| pull_request.pr_key.clone())
            .collect(),
        thread_last_comment_ids: input
            .tracked_threads
            .iter()
            .filter_map(|thread| Some((thread.id.clone(), thread.comments.last()?.id.clone())))
            .collect(),
    };

    let Some(previous) = previous else {
        return NotificationEvaluation {
            state: next_state,
            notifications: Vec::new(),
        };
    };

    let previous_review_requests = previous
        .review_requested_pr_keys
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut notifications = Vec::new();

    for pull_request in &input.review_requested_prs {
        if previous_review_requests.contains(&pull_request.pr_key) {
            continue;
        }

        notifications.push(SystemNotification {
            title: format!(
                "Review requested · {}#{}",
                pull_request.repository, pull_request.number
            ),
            body: summarize_text(&pull_request.title, 160),
        });
    }

    for thread in &input.tracked_threads {
        let Some(previous_comment_id) = previous.thread_last_comment_ids.get(&thread.id) else {
            continue;
        };
        let Some(previous_index) = thread
            .comments
            .iter()
            .position(|comment| comment.id == *previous_comment_id)
        else {
            continue;
        };

        for comment in thread.comments.iter().skip(previous_index + 1) {
            if comment.author_login == thread.owner_login {
                continue;
            }

            notifications.push(SystemNotification {
                title: format!(
                    "New comment on your review · {}#{}",
                    thread.pull_request.repository, thread.pull_request.number
                ),
                body: format!(
                    "{}: {}",
                    comment.author_login,
                    summarize_text(&comment.body, 160)
                ),
            });
        }
    }

    NotificationEvaluation {
        state: next_state,
        notifications,
    }
}

fn summarize_text(value: &str, max_len: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "No content".to_string();
    }

    if normalized.chars().count() <= max_len {
        return normalized;
    }

    normalized
        .chars()
        .take(max_len.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn notification_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_request(key: &str, repository: &str, number: i64, title: &str) -> TrackedPullRequest {
        TrackedPullRequest {
            pr_key: key.to_string(),
            repository: repository.to_string(),
            number,
            title: title.to_string(),
        }
    }

    fn comment(id: &str, author_login: &str, body: &str) -> TrackedComment {
        TrackedComment {
            id: id.to_string(),
            author_login: author_login.to_string(),
            body: body.to_string(),
        }
    }

    fn thread(
        id: &str,
        pull_request: &TrackedPullRequest,
        owner_login: &str,
        comments: Vec<TrackedComment>,
    ) -> TrackedReviewThread {
        TrackedReviewThread {
            id: id.to_string(),
            pull_request: pull_request.clone(),
            owner_login: owner_login.to_string(),
            comments,
        }
    }

    #[test]
    fn first_sync_primes_state_without_notifications() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![comment("c1", "me", "Please rename this")],
            )],
        };

        let evaluation = evaluate_notifications(&input, None);

        assert!(evaluation.notifications.is_empty());
        assert_eq!(
            evaluation.state.review_requested_pr_keys,
            vec!["org/repo#42".to_string()]
        );
        assert_eq!(
            evaluation.state.thread_last_comment_ids.get("thread-1"),
            Some(&"c1".to_string())
        );
    }

    #[test]
    fn notifies_when_pull_request_enters_review_requested_queue() {
        let input = NotificationInput {
            review_requested_prs: vec![pull_request(
                "org/repo#42",
                "org/repo",
                42,
                "Improve review UX",
            )],
            tracked_threads: Vec::new(),
        };
        let previous = PersistedNotificationState::default();

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert_eq!(evaluation.notifications.len(), 1);
        assert_eq!(
            evaluation.notifications[0].title,
            "Review requested · org/repo#42"
        );
    }

    #[test]
    fn notifies_for_new_foreign_comment_after_watermark() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![
                    comment("c1", "me", "Please rename this"),
                    comment("c2", "alice", "Done"),
                ],
            )],
        };
        let previous = PersistedNotificationState {
            review_requested_pr_keys: vec!["org/repo#42".to_string()],
            thread_last_comment_ids: BTreeMap::from([("thread-1".to_string(), "c1".to_string())]),
        };

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert_eq!(evaluation.notifications.len(), 1);
        assert_eq!(
            evaluation.notifications[0].title,
            "New comment on your review · org/repo#42"
        );
        assert_eq!(evaluation.notifications[0].body, "alice: Done");
    }

    #[test]
    fn ignores_new_comments_authored_by_viewer() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![
                    comment("c1", "me", "Please rename this"),
                    comment("c2", "me", "Following up"),
                ],
            )],
        };
        let previous = PersistedNotificationState {
            review_requested_pr_keys: vec!["org/repo#42".to_string()],
            thread_last_comment_ids: BTreeMap::from([("thread-1".to_string(), "c1".to_string())]),
        };

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert!(evaluation.notifications.is_empty());
    }

    #[test]
    fn does_not_notify_when_thread_is_seen_for_the_first_time() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![
                    comment("c1", "me", "Please rename this"),
                    comment("c2", "alice", "Done"),
                ],
            )],
        };
        let previous = PersistedNotificationState {
            review_requested_pr_keys: vec!["org/repo#42".to_string()],
            thread_last_comment_ids: BTreeMap::new(),
        };

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert!(evaluation.notifications.is_empty());
    }
}
