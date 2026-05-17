use super::*;

pub fn trigger_submit_inline_comment(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let Some((detail_id, pending_review_id, repository, number, target, body, loading)) = ({
        let app_state = state.read(cx);
        app_state.active_detail().and_then(|detail| {
            if crate::local_review::is_local_review_detail(detail) {
                return None;
            }
            app_state.active_review_line_action.clone().map(|target| {
                (
                    detail.id.clone(),
                    detail
                        .viewer_pending_review
                        .as_ref()
                        .map(|review| review.id.clone()),
                    detail.repository.clone(),
                    detail.number,
                    target,
                    app_state.inline_comment_draft.clone(),
                    app_state.inline_comment_loading,
                )
            })
        })
    }) else {
        return;
    };

    if loading {
        return;
    }

    if body.trim().is_empty() {
        state.update(cx, |state, cx| {
            state.inline_comment_error =
                Some("Enter a line comment before submitting it.".to_string());
            cx.notify();
        });
        return;
    }

    let Some(line) = target.anchor.line else {
        return;
    };
    let Some(side) = target.anchor.side.clone() else {
        return;
    };

    state.update(cx, |state, cx| {
        state.inline_comment_loading = true;
        state.inline_comment_error = None;
        cx.notify();
    });

    let model = state.clone();
    let target_for_refresh = target.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let submit_result = cx
                .background_executor()
                .spawn(async move {
                    github::add_pending_pull_request_review_thread(
                        &detail_id,
                        pending_review_id.as_deref(),
                        &target.anchor.file_path,
                        &body,
                        Some(line),
                        Some(side.as_str()),
                        target.start_line,
                        target.start_side.as_deref(),
                    )
                })
                .await;

            let (success, message) = match submit_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            if !success {
                model
                    .update(cx, |state, cx| {
                        state.inline_comment_loading = false;
                        state.inline_comment_error = Some(message);
                        cx.notify();
                    })
                    .ok();
                return;
            }

            let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let repository_for_sync = repository.clone();

            let sync_result = cx
                .background_executor()
                .spawn(async move {
                    notifications::sync_pull_request_detail_with_read_state(
                        &cache,
                        &repository_for_sync,
                        number,
                    )
                })
                .await;

            model
                .update(cx, |state, cx| {
                    state.inline_comment_loading = false;
                    state.inline_comment_draft.clear();
                    state.inline_comment_error = None;

                    if state
                        .active_review_line_action
                        .as_ref()
                        .map(|active| active.stable_key() == target_for_refresh.stable_key())
                        .unwrap_or(false)
                    {
                        state.active_review_line_action = None;
                        state.active_review_line_action_position = None;
                        state.review_line_action_mode = ReviewLineActionMode::Menu;
                    }
                    cx.notify();
                })
                .ok();

            apply_detail_sync_result(&model, &repository, number, sync_result, cx).await;
        })
        .detach();
}

pub fn trigger_submit_review_from_review_mode(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).active_is_local_review() {
        return;
    }

    let Some((
        pull_request_id,
        pending_review_id,
        repository,
        number,
        action,
        body,
        loading,
        has_pending,
        own_pr,
    )) = ({
        let app_state = state.read(cx);
        app_state.active_detail().map(|detail| {
            let viewer = app_state.viewer_login().map(str::to_string);
            (
                detail.id.clone(),
                detail
                    .viewer_pending_review
                    .as_ref()
                    .map(|review| review.id.clone()),
                detail.repository.clone(),
                detail.number,
                app_state.review_action,
                app_state.review_body.clone(),
                app_state.review_loading,
                pending_review_comment_count(detail) > 0,
                viewer
                    .as_deref()
                    .map(|login| login == detail.author_login)
                    .unwrap_or(false),
            )
        })
    })
    else {
        return;
    };

    if loading {
        return;
    }

    if own_pr && action != ReviewAction::Comment {
        state.update(cx, |state, cx| {
            state.review_message =
                Some("You cannot approve or request changes on your own pull request.".to_string());
            state.review_success = false;
            cx.notify();
        });
        return;
    }

    if !has_pending && action == ReviewAction::Comment && body.trim().is_empty() {
        state.update(cx, |state, cx| {
            state.review_message =
                Some("Enter a review note before submitting a comment.".to_string());
            state.review_success = false;
            cx.notify();
        });
        return;
    }

    state.update(cx, |state, cx| {
        state.review_loading = true;
        state.review_message = None;
        state.review_success = false;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let body_for_submit = body.clone();
            let submit_result = cx
                .background_executor()
                .spawn(async move {
                    github::submit_graphql_pull_request_review(
                        &pull_request_id,
                        pending_review_id.as_deref(),
                        action,
                        &body_for_submit,
                    )
                })
                .await;

            let (success, message) = match submit_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            model
                .update(cx, |state, cx| {
                    state.review_loading = false;
                    state.review_message = Some(message.clone());
                    state.review_success = success;
                    if success {
                        state.review_body.clear();
                        state.review_editor_active = false;
                        state.review_editor_preview = false;
                        state.review_finish_modal_open = false;
                        let detail_key = pr_key(&repository, number);
                        state.detail_states.entry(detail_key).or_default().syncing = true;
                    }
                    cx.notify();
                })
                .ok();

            if success {
                let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
                if let Some(cache) = cache {
                    let repo_for_sync = repository.clone();
                    let sync_result = cx
                        .background_executor()
                        .spawn(async move {
                            notifications::sync_pull_request_detail_with_read_state(
                                &cache,
                                &repo_for_sync,
                                number,
                            )
                        })
                        .await;
                    apply_detail_sync_result(&model, &repository, number, sync_result, cx).await;
                }
            }
        })
        .detach();
}

fn trigger_update_pending_comment(
    state: &Entity<AppState>,
    comment_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((repository, number, body)) = ({
        let app_state = state.read(cx);
        app_state.active_detail().map(|detail| {
            (
                detail.repository.clone(),
                detail.number,
                app_state.inline_comment_draft.clone(),
            )
        })
    }) else {
        return;
    };

    if body.trim().is_empty() {
        state.update(cx, |state, cx| {
            state.review_thread_action_error = Some("Comment body cannot be empty.".to_string());
            cx.notify();
        });
        return;
    }

    state.update(cx, |state, cx| {
        state.review_comment_action_loading_id = Some(comment_id.clone());
        state.review_thread_action_error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let update_result = cx
                .background_executor()
                .spawn(
                    async move { github::update_pull_request_review_comment(&comment_id, &body) },
                )
                .await;
            let (success, message) = match update_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            model
                .update(cx, |state, cx| {
                    state.review_comment_action_loading_id = None;
                    if success {
                        state.editing_review_comment_id = None;
                        state.inline_comment_draft.clear();
                        state.inline_comment_preview = false;
                        state
                            .detail_states
                            .entry(pr_key(&repository, number))
                            .or_default()
                            .syncing = true;
                    } else {
                        state.review_thread_action_error = Some(message);
                    }
                    cx.notify();
                })
                .ok();

            if success {
                let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
                if let Some(cache) = cache {
                    let repo_for_sync = repository.clone();
                    let sync_result = cx
                        .background_executor()
                        .spawn(async move {
                            notifications::sync_pull_request_detail_with_read_state(
                                &cache,
                                &repo_for_sync,
                                number,
                            )
                        })
                        .await;
                    apply_detail_sync_result(&model, &repository, number, sync_result, cx).await;
                }
            }
        })
        .detach();
}

fn trigger_delete_pending_comment(
    state: &Entity<AppState>,
    comment_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((repository, number)) = state
        .read(cx)
        .active_detail()
        .map(|detail| (detail.repository.clone(), detail.number))
    else {
        return;
    };

    state.update(cx, |state, cx| {
        state.review_comment_action_loading_id = Some(comment_id.clone());
        state.review_thread_action_error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let delete_result = cx
                .background_executor()
                .spawn(async move { github::delete_pull_request_review_comment(&comment_id) })
                .await;
            let (success, message) = match delete_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            model
                .update(cx, |state, cx| {
                    state.review_comment_action_loading_id = None;
                    if success {
                        state.editing_review_comment_id = None;
                        state.inline_comment_draft.clear();
                        state.inline_comment_preview = false;
                        state
                            .detail_states
                            .entry(pr_key(&repository, number))
                            .or_default()
                            .syncing = true;
                    } else {
                        state.review_thread_action_error = Some(message);
                    }
                    cx.notify();
                })
                .ok();

            if success {
                let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
                if let Some(cache) = cache {
                    let repo_for_sync = repository.clone();
                    let sync_result = cx
                        .background_executor()
                        .spawn(async move {
                            notifications::sync_pull_request_detail_with_read_state(
                                &cache,
                                &repo_for_sync,
                                number,
                            )
                        })
                        .await;
                    apply_detail_sync_result(&model, &repository, number, sync_result, cx).await;
                }
            }
        })
        .detach();
}

fn trigger_submit_thread_reply(
    state: &Entity<AppState>,
    thread_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((repository, number, body)) = ({
        let app_state = state.read(cx);
        app_state.active_detail().map(|detail| {
            (
                detail.repository.clone(),
                detail.number,
                app_state.inline_comment_draft.clone(),
            )
        })
    }) else {
        return;
    };

    if body.trim().is_empty() {
        state.update(cx, |state, cx| {
            state.review_thread_action_error = Some("Reply body cannot be empty.".to_string());
            cx.notify();
        });
        return;
    }

    state.update(cx, |state, cx| {
        state.review_thread_action_loading_id = Some(thread_id.clone());
        state.review_thread_action_error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let reply_result = cx
                .background_executor()
                .spawn(async move { github::reply_to_review_thread(&thread_id, &body) })
                .await;
            let (success, message) = match reply_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            model
                .update(cx, |state, cx| {
                    state.review_thread_action_loading_id = None;
                    if success {
                        state.active_review_thread_reply_id = None;
                        state.inline_comment_draft.clear();
                        state.inline_comment_preview = false;
                        state
                            .detail_states
                            .entry(pr_key(&repository, number))
                            .or_default()
                            .syncing = true;
                    } else {
                        state.review_thread_action_error = Some(message);
                    }
                    cx.notify();
                })
                .ok();

            if success {
                let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
                if let Some(cache) = cache {
                    let repo_for_sync = repository.clone();
                    let sync_result = cx
                        .background_executor()
                        .spawn(async move {
                            notifications::sync_pull_request_detail_with_read_state(
                                &cache,
                                &repo_for_sync,
                                number,
                            )
                        })
                        .await;
                    apply_detail_sync_result(&model, &repository, number, sync_result, cx).await;
                }
            }
        })
        .detach();
}

fn trigger_set_review_thread_resolution(
    state: &Entity<AppState>,
    thread_id: String,
    resolved: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((repository, number)) = state
        .read(cx)
        .active_detail()
        .map(|detail| (detail.repository.clone(), detail.number))
    else {
        return;
    };

    state.update(cx, |state, cx| {
        state.review_thread_action_loading_id = Some(thread_id.clone());
        state.review_thread_action_error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let resolution_result = cx
                .background_executor()
                .spawn(async move { github::set_review_thread_resolution(&thread_id, resolved) })
                .await;
            let (success, message) = match resolution_result {
                Ok(result) => (result.success, result.message),
                Err(error) => (false, error),
            };

            model
                .update(cx, |state, cx| {
                    state.review_thread_action_loading_id = None;
                    if success {
                        state
                            .detail_states
                            .entry(pr_key(&repository, number))
                            .or_default()
                            .syncing = true;
                    } else {
                        state.review_thread_action_error = Some(message);
                    }
                    cx.notify();
                })
                .ok();

            if success {
                let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
                if let Some(cache) = cache {
                    let repo_for_sync = repository.clone();
                    let sync_result = cx
                        .background_executor()
                        .spawn(async move {
                            notifications::sync_pull_request_detail_with_read_state(
                                &cache,
                                &repo_for_sync,
                                number,
                            )
                        })
                        .await;
                    apply_detail_sync_result(&model, &repository, number, sync_result, cx).await;
                }
            }
        })
        .detach();
}

async fn apply_detail_sync_result(
    model: &Entity<AppState>,
    repository: &str,
    number: i64,
    sync_result: Result<(github::PullRequestDetailSnapshot, BTreeSet<String>), String>,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            let detail_key = pr_key(repository, number);
            let mut updated_detail = None;
            let mut unread_ids_update = None;
            {
                let detail_state = state.detail_states.entry(detail_key).or_default();
                detail_state.syncing = false;
                match sync_result {
                    Ok((snapshot, unread_ids)) => {
                        updated_detail = snapshot.detail.clone();
                        detail_state.snapshot = Some(snapshot);
                        detail_state.error = None;
                        unread_ids_update = Some(unread_ids);
                    }
                    Err(error) => {
                        detail_state.error = Some(error);
                    }
                }
            }
            if let Some(unread_ids) = unread_ids_update {
                state.unread_review_comment_ids = unread_ids;
            }
            if let Some(detail) = updated_detail.as_ref() {
                update_open_tab_summary_from_detail(state, detail);
            }
            cx.notify();
        })
        .ok();
}

fn update_open_tab_summary_from_detail(state: &mut AppState, detail: &PullRequestDetail) {
    let detail_key = pr_key(&detail.repository, detail.number);
    let Some(tab) = state
        .open_tabs
        .iter_mut()
        .find(|tab| pr_key(&tab.repository, tab.number) == detail_key)
    else {
        return;
    };

    tab.title = detail.title.clone();
    tab.author_login = detail.author_login.clone();
    tab.author_avatar_url = detail.author_avatar_url.clone();
    tab.is_draft = detail.is_draft;
    tab.comments_count = detail.comments_count;
    tab.additions = detail.additions;
    tab.deletions = detail.deletions;
    tab.changed_files = detail.changed_files;
    tab.state = detail.state.clone();
    tab.review_decision = detail.review_decision.clone();
    tab.updated_at = detail.updated_at.clone();
    tab.url = detail.url.clone();
}

pub(super) fn pending_review_comment_count(detail: &PullRequestDetail) -> usize {
    let pending_review_count = detail
        .viewer_pending_review
        .as_ref()
        .map(|review| review.comments.len())
        .unwrap_or(0);
    let pending_thread_count = detail
        .review_threads
        .iter()
        .flat_map(|thread| thread.comments.iter())
        .filter(|comment| comment.state == "PENDING")
        .count();

    pending_review_count.max(pending_thread_count)
}

fn pending_review_comments(detail: &PullRequestDetail) -> Vec<&PullRequestReviewComment> {
    let mut seen = BTreeSet::new();
    let mut comments = Vec::new();

    if let Some(review) = detail.viewer_pending_review.as_ref() {
        for comment in &review.comments {
            if seen.insert(comment.id.clone()) {
                comments.push(comment);
            }
        }
    }

    for comment in detail
        .review_threads
        .iter()
        .flat_map(|thread| thread.comments.iter())
        .filter(|comment| comment.state == "PENDING")
    {
        if seen.insert(comment.id.clone()) {
            comments.push(comment);
        }
    }

    comments
}

fn pending_comment_location(comment: &PullRequestReviewComment) -> String {
    let line = comment
        .start_line
        .zip(comment.line)
        .filter(|(start, end)| start != end)
        .map(|(start, end)| format!("{start}-{end}"))
        .or_else(|| comment.line.map(|line| line.to_string()))
        .unwrap_or_else(|| "file".to_string());
    format!("{}:{line}", comment.path)
}

pub(super) fn open_review_line_action(
    state: &Entity<AppState>,
    target: ReviewLineActionTarget,
    position: Point<Pixels>,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        state.active_surface = PullRequestSurface::Files;
        state.navigate_to_review_location(target.review_location(), true);
        state.active_review_line_action = Some(target);
        state.active_review_line_action_position = Some(position);
        state.review_line_action_mode = ReviewLineActionMode::Comment;
        state.active_review_line_drag_origin = None;
        state.active_review_line_drag_current = None;
        state.inline_comment_draft.clear();
        state.inline_comment_preview = false;
        state.inline_comment_error = None;
        state.waypoint_spotlight_open = false;
        state.persist_active_review_session();
        cx.notify();
    });
}

pub(super) fn review_line_action_target_with_range(
    state: &Entity<AppState>,
    mut target: ReviewLineActionTarget,
    range_requested: bool,
    cx: &App,
) -> ReviewLineActionTarget {
    if !range_requested {
        return target;
    }

    let Some(current_anchor) = state.read(cx).selected_diff_anchor.clone() else {
        return target;
    };
    if current_anchor.file_path != target.anchor.file_path
        || current_anchor.side != target.anchor.side
        || current_anchor.line == target.anchor.line
    {
        return target;
    }

    let Some(current_line) = current_anchor.line else {
        return target;
    };
    let Some(target_line) = target.anchor.line else {
        return target;
    };

    let start = current_line.min(target_line);
    let end = current_line.max(target_line);
    target.start_line = Some(start);
    target.start_side = target.anchor.side.clone();
    target.anchor.line = Some(end);
    target.label = format!("{}:{start}-{end}", target.anchor.file_path);
    target
}

fn review_line_action_target_from_drag_range(
    origin: &ReviewLineActionTarget,
    mut target: ReviewLineActionTarget,
) -> ReviewLineActionTarget {
    if origin.anchor.file_path != target.anchor.file_path
        || origin.anchor.side != target.anchor.side
        || origin.anchor.line == target.anchor.line
    {
        return target;
    }

    let Some(origin_line) = origin.anchor.line else {
        return target;
    };
    let Some(target_line) = target.anchor.line else {
        return target;
    };

    let start = origin_line.min(target_line);
    let end = origin_line.max(target_line);
    target.start_line = Some(start);
    target.start_side = target.anchor.side.clone();
    target.anchor.line = Some(end);
    target.label = format!("{}:{start}-{end}", target.anchor.file_path);
    target
}

pub(super) fn begin_review_line_drag(
    state: &Entity<AppState>,
    target: ReviewLineActionTarget,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        state.active_review_line_drag_origin = Some(target.clone());
        state.active_review_line_drag_current = Some(target.clone());
        state.active_review_line_action = None;
        state.active_review_line_action_position = None;
        state.review_line_action_mode = ReviewLineActionMode::Menu;
        state.inline_comment_error = None;
        state.navigate_to_review_location(target.review_location(), true);
        cx.notify();
    });
}

pub(super) fn update_review_line_drag(
    state: &Entity<AppState>,
    target: ReviewLineActionTarget,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        if state.active_review_line_drag_origin.is_none() {
            return;
        }
        state.active_review_line_drag_current = Some(target.clone());
        state.navigate_to_review_location(target.review_location(), false);
        cx.notify();
    });
}

pub(super) fn finish_review_line_drag(
    state: &Entity<AppState>,
    target: ReviewLineActionTarget,
    position: Point<Pixels>,
    cx: &mut App,
) {
    let Some(origin) = state.read(cx).active_review_line_drag_origin.clone() else {
        return;
    };
    let target = review_line_action_target_from_drag_range(&origin, target);

    state.update(cx, |state, cx| {
        state.active_review_line_drag_origin = None;
        state.active_review_line_drag_current = None;
        cx.notify();
    });

    open_review_line_action(state, target, position, cx);
}

fn line_target_in_review_range(
    target: &ReviewLineActionTarget,
    range_end: &ReviewLineActionTarget,
) -> bool {
    let Some(start_line) = range_end.start_line else {
        return false;
    };
    if target.anchor.file_path != range_end.anchor.file_path
        || target.anchor.side != range_end.anchor.side
    {
        return false;
    }

    let Some(target_line) = target.anchor.line else {
        return false;
    };
    let Some(end_line) = range_end.anchor.line else {
        return false;
    };

    let start = start_line.min(end_line);
    let end = start_line.max(end_line);
    (start..=end).contains(&target_line)
}

fn line_target_in_active_drag_range(
    target: &ReviewLineActionTarget,
    origin: Option<&ReviewLineActionTarget>,
    current: Option<&ReviewLineActionTarget>,
) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    let Some(current) = current else {
        return false;
    };
    if target.anchor.file_path != origin.anchor.file_path
        || target.anchor.file_path != current.anchor.file_path
        || target.anchor.side != origin.anchor.side
        || target.anchor.side != current.anchor.side
    {
        return false;
    }

    let Some(target_line) = target.anchor.line else {
        return false;
    };
    let Some(origin_line) = origin.anchor.line else {
        return false;
    };
    let Some(current_line) = current.anchor.line else {
        return false;
    };

    let start = origin_line.min(current_line);
    let end = origin_line.max(current_line);
    (start..=end).contains(&target_line)
}

pub(super) fn build_review_line_action_target(
    file_path: &str,
    hunk_header: Option<&str>,
    line: &ParsedDiffLine,
) -> Option<ReviewLineActionTarget> {
    let side = if matches!(line.kind, DiffLineKind::Deletion) {
        Some("LEFT")
    } else if matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Context) {
        Some("RIGHT")
    } else {
        None
    }?;

    let line_number = match side {
        "LEFT" => line.left_line_number,
        _ => line.right_line_number,
    }?;
    let display_line = usize::try_from(line_number).ok().filter(|line| *line > 0)?;

    Some(ReviewLineActionTarget {
        anchor: DiffAnchor {
            file_path: file_path.to_string(),
            hunk_header: hunk_header.map(str::to_string),
            line: Some(line_number),
            side: Some(side.to_string()),
            thread_id: None,
        },
        start_line: None,
        start_side: None,
        label: format!("{file_path}:{display_line}"),
    })
}

pub(super) fn render_reviewable_diff_line(
    state: &Entity<AppState>,
    gutter_layout: DiffGutterLayout,
    file_path: &str,
    hunk_header: Option<&str>,
    line: &ParsedDiffLine,
    syntax_spans: Option<&[SyntaxSpan]>,
    emphasis_ranges: Option<&[DiffInlineRange]>,
    selected_anchor: Option<&DiffAnchor>,
    lsp_context: Option<&DiffLineLspContext>,
    temp_source_target: Option<TempSourceTarget>,
    cx: &App,
) -> impl IntoElement {
    let line_action_target = build_review_line_action_target(file_path, hunk_header, line);
    let (active_line_action, drag_origin, drag_current, waypoint, wrap_diff_lines) = {
        let app_state = state.read(cx);
        let active_line_action = app_state.active_review_line_action.clone();
        let drag_origin = app_state.active_review_line_drag_origin.clone();
        let drag_current = app_state.active_review_line_drag_current.clone();
        let waypoint = line_action_target
            .as_ref()
            .and_then(|target| {
                app_state
                    .active_review_session()
                    .and_then(|session| session.waymark_for_location(&target.review_location()))
            })
            .cloned();
        let wrap_diff_lines = app_state
            .active_review_session()
            .map(|session| session.wrap_diff_lines)
            .unwrap_or(false);
        (
            active_line_action,
            drag_origin,
            drag_current,
            waypoint,
            wrap_diff_lines,
        )
    };

    let popup_open = line_action_target
        .as_ref()
        .zip(active_line_action.as_ref())
        .map(|(line_target, active_target)| line_target.stable_key() == active_target.stable_key())
        .unwrap_or(false);
    let has_waypoint = !popup_open && waypoint.is_some();
    let range_selected = line_action_target
        .as_ref()
        .map(|target| {
            active_line_action
                .as_ref()
                .map(|active| line_target_in_review_range(target, active))
                .unwrap_or(false)
                || line_target_in_active_drag_range(
                    target,
                    drag_origin.as_ref(),
                    drag_current.as_ref(),
                )
        })
        .unwrap_or(false);

    render_diff_line(
        gutter_layout,
        file_path,
        line,
        syntax_spans,
        emphasis_ranges,
        selected_anchor,
        lsp_context,
        line_action_target.map(|target| (state.clone(), target)),
        temp_source_target.map(|target| (state.clone(), target)),
        has_waypoint,
        popup_open,
        range_selected,
        wrap_diff_lines,
    )
}

pub(super) fn render_diff_waypoint_icon() -> impl IntoElement {
    div()
        .relative()
        .w(px(12.0))
        .h(px(12.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(transparent())
        .bg(waypoint_icon_bg())
        .child(
            div()
                .absolute()
                .left(px(3.0))
                .top(px(3.0))
                .w(px(4.0))
                .h(px(4.0))
                .rounded(px(999.0))
                .bg(waypoint_icon_core()),
        )
}

pub(super) fn render_diff_open_source_icon() -> impl IntoElement {
    div()
        .w(px(12.0))
        .h(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .font_family("lucide")
        .text_size(px(12.0))
        .line_height(px(12.0))
        .child(LucideIcon::ExternalLink.unicode().to_string())
}

pub(super) fn render_waypoint_pill(label: &str, active: bool) -> impl IntoElement {
    div()
        .max_w(px(280.0))
        .flex_shrink_0()
        .px(px(9.0))
        .py(px(4.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(transparent())
        .bg(if active {
            waypoint_active_bg()
        } else {
            waypoint_bg()
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(999.0))
                        .bg(waypoint_icon_core()),
                )
                .child(
                    div()
                        .max_w(px(220.0))
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(waypoint_fg())
                        .child(label.to_string()),
                ),
        )
}

pub(super) fn render_review_line_action_overlay(
    state: &Entity<AppState>,
    target: &ReviewLineActionTarget,
    position: Point<Pixels>,
    mode: ReviewLineActionMode,
    cx: &App,
) -> impl IntoElement {
    let has_waypoint = state
        .read(cx)
        .active_review_session()
        .and_then(|session| session.waymark_for_location(&target.review_location()))
        .is_some();

    anchored()
        .position(position)
        .anchor(Corner::TopLeft)
        .offset(point(px(12.0), px(10.0)))
        .snap_to_window_with_margin(px(12.0))
        .child(render_review_line_action_popup(
            state,
            Some(target),
            mode,
            has_waypoint,
            cx,
        ))
}

pub(super) fn render_finish_review_modal(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    cx: &App,
) -> impl IntoElement {
    let app_state = state.read(cx);
    let pending_comments = pending_review_comments(detail);
    let pending_count = pending_comments.len();
    let action = app_state.review_action;
    let body_empty = app_state.review_body.trim().is_empty();
    let loading = app_state.review_loading;
    let preview = app_state.review_editor_preview;
    let message = app_state.review_message.clone();
    let success = app_state.review_success;
    let own_pr = app_state
        .viewer_login()
        .map(|login| login == detail.author_login)
        .unwrap_or(false);
    let invalid_empty_comment = pending_count == 0 && action == ReviewAction::Comment && body_empty;
    let own_pr_blocked = own_pr && action != ReviewAction::Comment;
    let submit_disabled = loading || invalid_empty_comment || own_pr_blocked;
    let close_state = state.clone();
    let submit_state = state.clone();

    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(72.0))
        .pb(px(32.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    close_review_finish_modal(&close_state, cx);
                }),
        )
        .child(
            div()
                .relative()
                .w(px(860.0))
                .max_w(px(1040.0))
                .max_h(px(640.0))
                .rounded(radius_lg())
                .border_1()
                .border_color(transparent())
                .bg(bg_overlay())
                .shadow(dialog_shadow())
                .occlude()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(22.0))
                        .py(px(16.0))
                        .border_b(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Finish your review"),
                        )
                        .child(
                            div()
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded(radius_sm())
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(hover_bg()))
                                .on_mouse_down(MouseButton::Left, {
                                    let state = state.clone();
                                    move |_, _, cx| {
                                        cx.stop_propagation();
                                        close_review_finish_modal(&state, cx);
                                    }
                                })
                                .child(lucide_icon(LucideIcon::X, 17.0, fg_muted())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .id("finish-review-scroll")
                        .gap(px(14.0))
                        .p(px(22.0))
                        .overflow_y_scroll()
                        .child(render_markdown_editor(
                            state,
                            AppTextFieldKind::ReviewBody,
                            "finish-review-body",
                            "Leave a comment",
                            preview,
                            170.0,
                            cx,
                        ))
                        .child(render_pending_review_summary(pending_comments))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(render_review_action_choice(
                                    state,
                                    ReviewAction::Comment,
                                    action,
                                    false,
                                    "Comment",
                                    "Submit general feedback without explicit approval.",
                                ))
                                .child(render_review_action_choice(
                                    state,
                                    ReviewAction::Approve,
                                    action,
                                    own_pr,
                                    "Approve",
                                    if own_pr {
                                        "You cannot approve your own pull request."
                                    } else {
                                        "Submit feedback and approve merging these changes."
                                    },
                                ))
                                .child(render_review_action_choice(
                                    state,
                                    ReviewAction::RequestChanges,
                                    action,
                                    own_pr,
                                    "Request changes",
                                    if own_pr {
                                        "You cannot request changes on your own pull request."
                                    } else {
                                        "Submit feedback suggesting changes."
                                    },
                                )),
                        )
                        .when_some(message, |el, message: String| {
                            el.child(if success {
                                success_text(&message).into_any_element()
                            } else {
                                error_text(&message).into_any_element()
                            })
                        }),
                )
                .child(
                    div()
                        .px(px(22.0))
                        .py(px(16.0))
                        .border_t(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_size(px(12.0)).text_color(fg_subtle()).child(
                            if pending_count > 0 {
                                "Pending drafts will be published with this review.".to_string()
                            } else {
                                "Approval can be submitted without a body.".to_string()
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(ghost_button("Cancel", {
                                    let state = state.clone();
                                    move |_, _, cx| {
                                        close_review_finish_modal(&state, cx);
                                    }
                                }))
                                .child(review_submit_button(
                                    if loading {
                                        "Submitting..."
                                    } else {
                                        "Submit review"
                                    },
                                    submit_disabled,
                                    move |_, window, cx| {
                                        trigger_submit_review_from_review_mode(
                                            &submit_state,
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                        ),
                ),
        )
}

fn render_pending_review_summary(comments: Vec<&PullRequestReviewComment>) -> impl IntoElement {
    let count = comments.len();

    div()
        .rounded(radius())
        .border_1()
        .border_color(transparent())
        .bg(bg_surface())
        .overflow_hidden()
        .child(
            div()
                .px(px(12.0))
                .py(px(9.0))
                .border_b(px(1.0))
                .border_color(border_muted())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Pending drafts"),
                )
                .child(badge(&count.to_string())),
        )
        .child(if comments.is_empty() {
            div()
                .px(px(12.0))
                .py(px(10.0))
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child("No pending line comments.")
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(comments.into_iter().take(5).map(|comment| {
                    div()
                        .px(px(12.0))
                        .py(px(9.0))
                        .border_b(px(1.0))
                        .border_color(border_muted())
                        .flex()
                        .items_start()
                        .gap(px(10.0))
                        .child(lucide_icon(LucideIcon::MessageSquare, 14.0, fg_muted()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_muted())
                                        .child(pending_comment_location(comment)),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .line_height(px(22.0))
                                        .text_color(fg_default())
                                        .whitespace_normal()
                                        .child(
                                            comment.body.lines().next().unwrap_or("").to_string(),
                                        ),
                                ),
                        )
                }))
                .into_any_element()
        })
}

fn render_review_action_choice(
    state: &Entity<AppState>,
    action: ReviewAction,
    selected: ReviewAction,
    disabled: bool,
    label: &'static str,
    description: &'static str,
) -> impl IntoElement {
    let active = action == selected;
    let state_for_click = state.clone();

    div()
        .flex()
        .items_start()
        .gap(px(10.0))
        .p(px(10.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(if active { bg_selected() } else { bg_surface() })
        .opacity(if disabled { 0.48 } else { 1.0 })
        .when(!disabled, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    state_for_click.update(cx, |state, cx| {
                        state.review_action = action;
                        state.review_message = None;
                        state.review_success = false;
                        cx.notify();
                    });
                })
        })
        .child(
            div()
                .w(px(16.0))
                .h(px(16.0))
                .mt(px(1.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(if active { accent() } else { border_default() })
                .flex()
                .items_center()
                .justify_center()
                .child(if active {
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(999.0))
                        .bg(accent())
                        .into_any_element()
                } else {
                    div().into_any_element()
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(17.0))
                        .text_color(fg_muted())
                        .child(description),
                ),
        )
}

fn review_submit_button(
    label: &'static str,
    disabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .bg(if disabled {
            control_button_bg()
        } else {
            primary_action_bg()
        })
        .text_color(if disabled {
            fg_subtle()
        } else {
            fg_on_primary_action()
        })
        .text_size(px(13.0))
        .font_weight(FontWeight::SEMIBOLD)
        .opacity(if disabled { 0.72 } else { 1.0 })
        .when(!disabled, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(primary_action_hover()))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(label)
}

fn render_review_line_action_popup(
    state: &Entity<AppState>,
    target: Option<&ReviewLineActionTarget>,
    mode: ReviewLineActionMode,
    has_waypoint: bool,
    cx: &App,
) -> impl IntoElement {
    let app_state = state.read(cx);
    let inline_comment_loading = app_state.inline_comment_loading;
    let inline_comment_error = app_state.inline_comment_error.clone();
    let inline_comment_preview = app_state.inline_comment_preview;
    let popup_key = target
        .map(|target| target.stable_key())
        .unwrap_or_else(|| "line-action-popup".to_string());
    let popup_animation_key = popup_key.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(33).wrapping_add(byte as usize)
    });

    div()
        .min_w(px(420.0))
        .max_w(px(560.0))
        .rounded(radius())
        .border_1()
        .border_color(transparent())
        .bg(bg_overlay())
        // Prevent diff rows behind the popup from receiving mouse interactions.
        .occlude()
        .shadow(popover_shadow())
        .on_any_mouse_down(|_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .border_b(px(1.0))
                .border_color(border_muted())
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_family(mono_font_family())
                        .text_color(waypoint_fg())
                        .child(
                            target
                                .map(|target| target.label.to_uppercase())
                                .unwrap_or_else(|| "LINE ACTION".to_string()),
                        ),
                ),
        )
        .child(match mode {
            ReviewLineActionMode::Menu | ReviewLineActionMode::Comment => div()
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(render_markdown_editor(
                    state,
                    AppTextFieldKind::InlineCommentDraft,
                    format!(
                        "inline-comment-{}",
                        target.map(|target| target.stable_key()).unwrap_or_default()
                    ),
                    "Comment on this line...",
                    inline_comment_preview,
                    104.0,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family(mono_font_family())
                                .text_color(fg_subtle())
                                .child("cmd-enter submit"),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .when(!has_waypoint, |el| {
                                    el.child(ghost_button("Add waypoint", {
                                        let state = state.clone();
                                        move |_, _, cx| {
                                            let default_name = {
                                                let app_state = state.read(cx);
                                                default_waymark_name(
                                                    app_state.selected_file_path.as_deref(),
                                                    None,
                                                    app_state.selected_diff_anchor.as_ref(),
                                                )
                                            };
                                            state.update(cx, |state, cx| {
                                                state.add_waymark_for_current_review_location(
                                                    default_name.clone(),
                                                );
                                                state.persist_active_review_session();
                                                cx.notify();
                                            });
                                        }
                                    }))
                                })
                                .child(ghost_button("Cancel", {
                                    let state = state.clone();
                                    move |_, _, cx| {
                                        close_review_line_action(&state, cx);
                                    }
                                }))
                                .child(review_button(
                                    if inline_comment_loading {
                                        "Submitting..."
                                    } else {
                                        "Submit"
                                    },
                                    {
                                        let state = state.clone();
                                        move |_, window, cx| {
                                            trigger_submit_inline_comment(&state, window, cx);
                                        }
                                    },
                                )),
                        ),
                )
                .when_some(inline_comment_error, |el, error| {
                    el.child(error_text(&error))
                })
                .into_any_element(),
        })
        .with_animation(
            ("review-line-action-popup", popup_animation_key),
            Animation::new(Duration::from_millis(140)).with_easing(ease_in_out),
            move |el, delta| {
                el.mt(lerp_px(8.0, 0.0, delta))
                    .opacity(delta.clamp(0.0, 1.0))
                    .border_color(transparent())
                    .bg(lerp_rgba(bg_surface(), bg_overlay(), delta))
            },
        )
}

fn render_markdown_editor(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    id_prefix: impl Into<String>,
    placeholder: &'static str,
    preview: bool,
    min_height: f32,
    cx: &App,
) -> AnyElement {
    let id_prefix = id_prefix.into();
    let text = markdown_field_text(state.read(cx), field).to_string();
    let suggestions = current_emoji_query(&text)
        .map(|query| emoji_shortcode_suggestions(query, 8))
        .unwrap_or_default();

    div()
        .w_full()
        .min_w_0()
        .rounded(radius())
        .border_1()
        .border_color(transparent())
        .bg(bg_surface())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(render_markdown_editor_tabs(state, field, preview))
        .when(!preview, |el| {
            el.child(render_markdown_toolbar(state, field))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .min_h(px(min_height))
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(14.0))
                        .line_height(px(22.0))
                        .text_color(if text.is_empty() {
                            fg_subtle()
                        } else {
                            fg_emphasis()
                        })
                        .child(
                            AppTextInput::new(
                                format!("{id_prefix}-input"),
                                state.clone(),
                                field,
                                placeholder,
                            )
                            .autofocus(true),
                        ),
                )
                .when(!suggestions.is_empty(), |el| {
                    el.child(render_emoji_suggestions(state, field, suggestions))
                })
        })
        .when(preview, |el| {
            el.child(
                div()
                    .w_full()
                    .min_w_0()
                    .min_h(px(min_height))
                    .px(px(12.0))
                    .py(px(10.0))
                    .bg(bg_surface())
                    .child(if text.trim().is_empty() {
                        div()
                            .text_size(px(14.0))
                            .line_height(px(22.0))
                            .text_color(fg_subtle())
                            .child("Nothing to preview.")
                            .into_any_element()
                    } else {
                        render_markdown(&format!("{id_prefix}-preview"), &text).into_any_element()
                    }),
            )
        })
        .into_any_element()
}

fn render_markdown_editor_tabs(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    preview: bool,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .border_b(px(1.0))
        .border_color(border_muted())
        .bg(bg_overlay())
        .child(markdown_editor_tab("Write", !preview, {
            let state = state.clone();
            move |_, _, cx| set_markdown_preview(&state, field, false, cx)
        }))
        .child(markdown_editor_tab("Preview", preview, {
            let state = state.clone();
            move |_, _, cx| set_markdown_preview(&state, field, true, cx)
        }))
}

fn markdown_editor_tab(
    label: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(13.0))
        .py(px(9.0))
        .border_r(px(1.0))
        .border_color(border_muted())
        .bg(if active { bg_surface() } else { transparent() })
        .text_size(px(12.0))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(if active { fg_emphasis() } else { fg_muted() })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(label)
}

fn render_markdown_toolbar(state: &Entity<AppState>, field: AppTextFieldKind) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .px(px(8.0))
        .py(px(6.0))
        .border_b(px(1.0))
        .border_color(border_muted())
        .bg(bg_overlay())
        .children([
            markdown_toolbar_button(state, field, LucideIcon::Bold, "Bold", "**bold**"),
            markdown_toolbar_button(state, field, LucideIcon::Italic, "Italic", "_italic_"),
            markdown_toolbar_button(state, field, LucideIcon::Quote, "Quote", "\n> "),
            markdown_toolbar_button(state, field, LucideIcon::Code, "Inline code", "`code`"),
            markdown_toolbar_button(state, field, LucideIcon::Link, "Link", "[text](url)"),
            markdown_toolbar_button(state, field, LucideIcon::List, "Bulleted list", "\n- "),
            markdown_toolbar_button(
                state,
                field,
                LucideIcon::ListOrdered,
                "Numbered list",
                "\n1. ",
            ),
            markdown_toolbar_button(state, field, LucideIcon::ListTodo, "Task list", "\n- [ ] "),
            markdown_toolbar_button(
                state,
                field,
                LucideIcon::MessageSquareDiff,
                "Suggestion",
                "\n```suggestion\n\n```",
            ),
            markdown_toolbar_button(state, field, LucideIcon::SmilePlus, "Emoji", ":"),
        ])
}

fn markdown_toolbar_button(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    icon: LucideIcon,
    tooltip: &'static str,
    snippet: &'static str,
) -> AnyElement {
    let state = state.clone();
    div()
        .id(tooltip)
        .w(px(26.0))
        .h(px(24.0))
        .rounded(radius_sm())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(move |_, cx| build_static_tooltip(tooltip, cx))
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            append_markdown_snippet(&state, field, snippet, cx);
        })
        .child(lucide_icon(icon, 14.0, fg_muted()))
        .into_any_element()
}

fn render_emoji_suggestions(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    suggestions: Vec<EmojiSuggestion>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .px(px(8.0))
        .py(px(7.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .bg(bg_overlay())
        .children(suggestions.into_iter().map(|suggestion| {
            let state = state.clone();
            let shortcode = suggestion.shortcode.clone();
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(7.0))
                .py(px(4.0))
                .rounded(radius_sm())
                .bg(bg_surface())
                .border_1()
                .border_color(transparent())
                .text_size(px(12.0))
                .text_color(fg_default())
                .cursor_pointer()
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    replace_current_emoji_query(&state, field, &shortcode, cx);
                })
                .child(suggestion.glyph)
                .child(format!(":{}:", suggestion.shortcode))
        }))
}

fn set_markdown_preview(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    preview: bool,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        match field {
            AppTextFieldKind::ReviewBody => state.review_editor_preview = preview,
            AppTextFieldKind::InlineCommentDraft => state.inline_comment_preview = preview,
            _ => {}
        }
        cx.notify();
    });
}

fn append_markdown_snippet(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    snippet: &str,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        let current = markdown_field_text_mut(state, field);
        if !current.is_empty() && !current.ends_with('\n') && snippet.starts_with('\n') {
            current.push('\n');
            current.push_str(snippet.trim_start_matches('\n'));
        } else {
            current.push_str(snippet);
        }
        cx.notify();
    });
}

fn replace_current_emoji_query(
    state: &Entity<AppState>,
    field: AppTextFieldKind,
    shortcode: &str,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        let current = markdown_field_text_mut(state, field);
        let Some(start) = current.rfind(':') else {
            return;
        };
        if current[start + 1..].contains(':') {
            return;
        }
        current.truncate(start);
        current.push(':');
        current.push_str(shortcode);
        current.push_str(": ");
        cx.notify();
    });
}

fn markdown_field_text(state: &AppState, field: AppTextFieldKind) -> &str {
    match field {
        AppTextFieldKind::ReviewBody => state.review_body.as_str(),
        AppTextFieldKind::InlineCommentDraft => state.inline_comment_draft.as_str(),
        AppTextFieldKind::WaymarkDraft => state.waymark_draft.as_str(),
        AppTextFieldKind::PaletteQuery => state.palette_query.as_str(),
        AppTextFieldKind::WaypointSpotlightQuery => state.waypoint_spotlight_query.as_str(),
    }
}

fn markdown_field_text_mut(state: &mut AppState, field: AppTextFieldKind) -> &mut String {
    match field {
        AppTextFieldKind::ReviewBody => &mut state.review_body,
        AppTextFieldKind::InlineCommentDraft => &mut state.inline_comment_draft,
        AppTextFieldKind::WaymarkDraft => &mut state.waymark_draft,
        AppTextFieldKind::PaletteQuery => &mut state.palette_query,
        AppTextFieldKind::WaypointSpotlightQuery => &mut state.waypoint_spotlight_query,
    }
}

fn current_emoji_query(text: &str) -> Option<&str> {
    let start = text.rfind(':')?;
    let query = &text[start + 1..];
    if query.is_empty() || query.contains(':') || query.chars().any(char::is_whitespace) {
        return None;
    }
    Some(query)
}

fn line_action_button(
    label: &str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let animation_id = SharedString::from(format!(
        "line-action-button-{label}-{}",
        usize::from(active)
    ));

    div()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(transparent())
        .bg(if active { waypoint_bg() } else { bg_surface() })
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { waypoint_fg() } else { fg_emphasis() })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(label.to_string())
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(TOGGLE_ANIMATION_MS)).with_easing(ease_in_out),
            move |el, delta| {
                let progress = selected_reveal_progress(active, delta);
                el.bg(mix_rgba(bg_surface(), waypoint_bg(), progress))
                    .border_color(transparent())
                    .text_color(mix_rgba(fg_emphasis(), waypoint_fg(), progress))
            },
        )
}
#[derive(Clone, Default)]
pub(super) struct ReviewThreadUiState {
    active_reply_id: Option<String>,
    editing_comment_id: Option<String>,
    thread_loading_id: Option<String>,
    comment_loading_id: Option<String>,
    action_error: Option<String>,
    inline_preview: bool,
}

pub(super) fn review_thread_ui_state(state: &AppState) -> ReviewThreadUiState {
    ReviewThreadUiState {
        active_reply_id: state.active_review_thread_reply_id.clone(),
        editing_comment_id: state.editing_review_comment_id.clone(),
        thread_loading_id: state.review_thread_action_loading_id.clone(),
        comment_loading_id: state.review_comment_action_loading_id.clone(),
        action_error: state.review_thread_action_error.clone(),
        inline_preview: state.inline_comment_preview,
    }
}

pub(super) fn render_review_thread(
    thread: &PullRequestReviewThread,
    _selected_anchor: Option<&DiffAnchor>,
    unread_comment_ids: &BTreeSet<String>,
    state: &Entity<AppState>,
    cx: &App,
    ui: ReviewThreadUiState,
) -> impl IntoElement {
    let thread_unread_comment_ids = thread
        .comments
        .iter()
        .filter(|comment| unread_comment_ids.contains(&comment.id))
        .map(|comment| comment.id.clone())
        .collect::<Vec<_>>();
    let unread_count = thread_unread_comment_ids.len();
    let state_for_mark_read = state.clone();
    let thread_id = thread.id.clone();
    let reply_open = ui.active_reply_id.as_deref() == Some(thread.id.as_str());
    let thread_loading = ui.thread_loading_id.as_deref() == Some(thread.id.as_str());
    let viewer_login = state.read(cx).viewer_login().unwrap_or("you").to_string();
    let can_resolve = if thread.is_resolved {
        thread.viewer_can_unresolve
    } else {
        thread.viewer_can_resolve
    };
    let comment_count = thread.comments.len();

    div()
        .w_full()
        .max_w(px(REVIEW_THREAD_MAX_WIDTH))
        .rounded(radius_sm())
        .border_1()
        .border_color(transparent())
        .bg(diff_editor_surface())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .rounded_tl(radius_sm())
                .rounded_tr(radius_sm())
                .border_b(px(1.0))
                .border_color(diff_annotation_border())
                .bg(diff_editor_surface())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if thread.is_resolved {
                                    success()
                                } else {
                                    fg_muted()
                                })
                                .child("Review thread"),
                        )
                        .when(thread.is_resolved, |el| el.child(badge_success("resolved")))
                        .when(thread.is_outdated, |el| el.child(badge("outdated"))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .when(unread_count > 0, |el| {
                            el.child(badge(&format!("{unread_count} new")))
                                .child(ghost_button("Mark read", move |_, _, cx| {
                                    state_for_mark_read.update(cx, |state, cx| {
                                        state.mark_review_comments_read(
                                            thread_unread_comment_ids.clone(),
                                        );
                                        cx.notify();
                                    });
                                }))
                        })
                        .when(can_resolve, |el| {
                            let state = state.clone();
                            let thread_id = thread_id.clone();
                            let next_resolved = !thread.is_resolved;
                            el.child(ghost_button(
                                if thread.is_resolved {
                                    "Unresolve"
                                } else {
                                    "Resolve"
                                },
                                move |_, window, cx| {
                                    trigger_set_review_thread_resolution(
                                        &state,
                                        thread_id.clone(),
                                        next_resolved,
                                        window,
                                        cx,
                                    );
                                },
                            ))
                        })
                        .when(thread_loading, |el| el.child(badge("syncing"))),
                ),
        )
        .child(
            div()
                .px(px(16.0))
                .py(px(8.0))
                .flex()
                .flex_col()
                .children(thread.comments.iter().enumerate().map(|(ix, comment)| {
                    render_thread_comment(
                        comment,
                        ix > 0,
                        ix + 1 < comment_count || reply_open || thread.viewer_can_reply,
                        unread_comment_ids.contains(&comment.id),
                        state,
                        ui.clone(),
                        cx,
                    )
                }))
                .when(reply_open, |el| {
                    let state_for_cancel = state.clone();
                    let state_for_submit = state.clone();
                    let thread_id_for_submit = thread_id.clone();
                    el.child(render_thread_reply_editor(
                        state,
                        &thread_id,
                        &viewer_login,
                        ui.inline_preview,
                        thread_loading,
                        move |_, _, cx| {
                            state_for_cancel.update(cx, |state, cx| {
                                state.active_review_thread_reply_id = None;
                                state.inline_comment_draft.clear();
                                state.inline_comment_preview = false;
                                state.review_thread_action_error = None;
                                cx.notify();
                            });
                        },
                        move |_, window, cx| {
                            trigger_submit_thread_reply(
                                &state_for_submit,
                                thread_id_for_submit.clone(),
                                window,
                                cx,
                            );
                        },
                        cx,
                    ))
                })
                .when(thread.viewer_can_reply && !reply_open, |el| {
                    let state = state.clone();
                    let thread_id = thread_id.clone();
                    el.child(render_thread_reply_prompt(
                        &viewer_login,
                        move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                state.active_review_thread_reply_id = Some(thread_id.clone());
                                state.editing_review_comment_id = None;
                                state.inline_comment_draft.clear();
                                state.inline_comment_preview = false;
                                state.review_thread_action_error = None;
                                cx.notify();
                            });
                        },
                    ))
                })
                .when_some(ui.action_error.clone(), |el, error| {
                    el.child(div().ml(px(48.0)).pt(px(4.0)).child(error_text(&error)))
                }),
        )
}

fn render_thread_comment(
    comment: &PullRequestReviewComment,
    connector_above: bool,
    connector_below: bool,
    is_unread: bool,
    state: &Entity<AppState>,
    ui: ReviewThreadUiState,
    cx: &App,
) -> impl IntoElement {
    let is_pending = comment.state == "PENDING";
    let is_editing = ui.editing_comment_id.as_deref() == Some(comment.id.as_str());
    let comment_loading = ui.comment_loading_id.as_deref() == Some(comment.id.as_str());
    let can_update = is_pending && comment.viewer_can_update;
    let can_delete = is_pending && comment.viewer_can_delete;
    let comment_id = comment.id.clone();

    div()
        .py(px(10.0))
        .bg(if is_unread {
            diff_line_hover_bg()
        } else {
            transparent()
        })
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(render_thread_timeline_avatar(
            &comment.author_login,
            comment.author_avatar_url.as_deref(),
            connector_above,
            connector_below,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .min_h(px(24.0))
                                .min_w_0()
                                .gap(px(7.0))
                                .text_size(px(13.0))
                                .line_height(px(19.0))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(comment.author_login.clone()),
                                )
                                .child(
                                    div().text_color(fg_subtle()).child(format_relative_time(
                                        comment
                                            .published_at
                                            .as_deref()
                                            .unwrap_or(&comment.created_at),
                                    )),
                                )
                                .when(is_unread, |el| el.child(badge("new")))
                                .when(is_pending, |el| el.child(pending_comment_status_label())),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .flex_shrink_0()
                                .when(can_update && !is_editing, |el| {
                                    let state = state.clone();
                                    let comment_id = comment_id.clone();
                                    let body = comment.body.clone();
                                    el.child(inline_comment_text_action(
                                        "Edit",
                                        false,
                                        move |_, _, cx| {
                                            state.update(cx, |state, cx| {
                                                state.editing_review_comment_id =
                                                    Some(comment_id.clone());
                                                state.active_review_thread_reply_id = None;
                                                state.inline_comment_draft = body.clone();
                                                state.inline_comment_preview = false;
                                                state.review_thread_action_error = None;
                                                cx.notify();
                                            });
                                        },
                                    ))
                                })
                                .when(can_delete, |el| {
                                    let state = state.clone();
                                    let comment_id = comment_id.clone();
                                    el.child(inline_comment_text_action(
                                        if comment_loading {
                                            "Deleting..."
                                        } else {
                                            "Delete"
                                        },
                                        true,
                                        move |_, window, cx| {
                                            trigger_delete_pending_comment(
                                                &state,
                                                comment_id.clone(),
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
                                }),
                        ),
                )
                .child(if is_editing {
                    let state_for_cancel = state.clone();
                    let state_for_save = state.clone();
                    let comment_id_for_save = comment_id.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(render_markdown_editor(
                            state,
                            AppTextFieldKind::InlineCommentDraft,
                            format!("comment-edit-{comment_id}"),
                            "Edit pending comment...",
                            ui.inline_preview,
                            82.0,
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(6.0))
                                .child(ghost_button("Cancel", move |_, _, cx| {
                                    state_for_cancel.update(cx, |state, cx| {
                                        state.editing_review_comment_id = None;
                                        state.inline_comment_draft.clear();
                                        state.inline_comment_preview = false;
                                        state.review_thread_action_error = None;
                                        cx.notify();
                                    });
                                }))
                                .child(review_button(
                                    if comment_loading { "Saving..." } else { "Save" },
                                    move |_, window, cx| {
                                        trigger_update_pending_comment(
                                            &state_for_save,
                                            comment_id_for_save.clone(),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                        )
                        .into_any_element()
                } else if comment.body.is_empty() {
                    div()
                        .text_size(px(14.0))
                        .line_height(px(22.0))
                        .text_color(fg_muted())
                        .child("No comment body.")
                        .into_any_element()
                } else {
                    div()
                        .max_w(px(760.0))
                        .child(render_markdown(
                            &format!("thread-comment-{}", comment.id),
                            &comment.body,
                        ))
                        .into_any_element()
                }),
        )
}

fn render_thread_timeline_avatar(
    login: &str,
    avatar_url: Option<&str>,
    connector_above: bool,
    connector_below: bool,
) -> impl IntoElement {
    div()
        .relative()
        .w(px(36.0))
        .min_h(px(42.0))
        .flex_shrink_0()
        .flex()
        .justify_center()
        .child(user_avatar(login, avatar_url, 24.0, false))
        .when(connector_above, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(17.5))
                    .w(px(1.0))
                    .h(px(8.0))
                    .bg(border_muted()),
            )
        })
        .when(connector_below, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(30.0))
                    .bottom(px(-10.0))
                    .left(px(17.5))
                    .w(px(1.0))
                    .bg(border_muted()),
            )
        })
}

fn render_thread_reply_prompt(
    viewer_login: &str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .pt(px(8.0))
        .pb(px(10.0))
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(render_thread_timeline_avatar(
            viewer_login,
            None,
            true,
            false,
        ))
        .child(
            div()
                .h(px(36.0))
                .flex_1()
                .min_w_0()
                .rounded(radius_sm())
                .border_1()
                .border_color(transparent())
                .bg(bg_surface())
                .px(px(12.0))
                .flex()
                .items_center()
                .text_size(px(14.0))
                .line_height(px(20.0))
                .text_color(fg_subtle())
                .cursor_pointer()
                .hover(|style| style.bg(control_button_hover_bg()).text_color(fg_muted()))
                .on_mouse_down(MouseButton::Left, on_click)
                .child("Reply..."),
        )
}

fn render_thread_reply_editor(
    state: &Entity<AppState>,
    thread_id: &str,
    viewer_login: &str,
    preview: bool,
    thread_loading: bool,
    on_cancel: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_submit: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let animation_id = thread_reply_editor_animation_id(thread_id);

    div()
        .pt(px(8.0))
        .pb(px(10.0))
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(render_thread_timeline_avatar(
            viewer_login,
            None,
            true,
            false,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .rounded(radius_sm())
                .border_1()
                .border_color(transparent())
                .bg(bg_surface())
                .p(px(10.0))
                .gap(px(8.0))
                .child(render_markdown_editor(
                    state,
                    AppTextFieldKind::InlineCommentDraft,
                    format!("thread-reply-{thread_id}"),
                    "Reply to this thread...",
                    preview,
                    82.0,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(6.0))
                        .child(ghost_button("Cancel", on_cancel))
                        .child(review_button(
                            if thread_loading {
                                "Replying..."
                            } else {
                                "Reply"
                            },
                            on_submit,
                        )),
                ),
        )
        .with_animation(
            ("thread-reply-editor-open", animation_id),
            Animation::new(Duration::from_millis(THREAD_REPLY_EDITOR_OPEN_ANIMATION_MS))
                .with_easing(ease_in_out),
            move |el, delta| {
                let progress = delta.clamp(0.0, 1.0);
                let el = el.opacity(progress).mt(lerp_px(-4.0, 0.0, progress));

                if progress < 0.999 {
                    el.max_h(lerp_px(
                        THREAD_REPLY_PROMPT_REVEAL_HEIGHT,
                        THREAD_REPLY_EDITOR_REVEAL_HEIGHT,
                        progress,
                    ))
                    .overflow_hidden()
                } else {
                    el
                }
            },
        )
}

const THREAD_REPLY_EDITOR_OPEN_ANIMATION_MS: u64 = 170;
const THREAD_REPLY_PROMPT_REVEAL_HEIGHT: f32 = 54.0;
const THREAD_REPLY_EDITOR_REVEAL_HEIGHT: f32 = 284.0;

fn thread_reply_editor_animation_id(thread_id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    thread_id.hash(&mut hasher);
    hasher.finish()
}

fn pending_comment_status_label() -> impl IntoElement {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(999.0))
        .bg(bg_subtle())
        .text_size(px(11.0))
        .line_height(px(16.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg_subtle())
        .child("pending")
}

fn inline_comment_text_action(
    label: &str,
    danger_tone: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    div()
        .px(px(4.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .text_size(px(12.0))
        .line_height(px(16.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg_subtle())
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(hover_bg())
                .text_color(if danger_tone { danger() } else { fg_emphasis() })
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
}
