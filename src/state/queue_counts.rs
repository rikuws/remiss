use std::collections::HashSet;

use crate::github::{IssueQueue, PullRequestQueue};

use super::{issue_key, summary_key};

pub(super) fn unique_pull_request_queue_count(queues: &[PullRequestQueue]) -> i64 {
    let mut keys = HashSet::new();
    let mut max_reported_count = 0;
    let mut all_queues_complete = true;

    for queue in queues {
        let reported_count = queue.total_count.max(0);
        max_reported_count = max_reported_count.max(reported_count);
        if !queue.is_complete || queue.items.len() as i64 != reported_count {
            all_queues_complete = false;
        }
        keys.extend(queue.items.iter().map(summary_key));
    }

    let unique_count = keys.len() as i64;
    if all_queues_complete {
        unique_count
    } else {
        unique_count.max(max_reported_count)
    }
}

pub(super) fn unique_issue_queue_count(queues: &[IssueQueue]) -> i64 {
    let mut keys = HashSet::new();
    let mut max_reported_count = 0;
    let mut all_queues_complete = true;

    for queue in queues {
        let reported_count = queue.total_count.max(0);
        max_reported_count = max_reported_count.max(reported_count);
        if !queue.is_complete || queue.items.len() as i64 != reported_count {
            all_queues_complete = false;
        }
        for issue in &queue.items {
            keys.insert(issue_key(&issue.repository, issue.number));
        }
    }

    let unique_count = keys.len() as i64;
    if all_queues_complete {
        unique_count
    } else {
        unique_count.max(max_reported_count)
    }
}
