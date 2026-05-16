use super::*;

pub fn ensure_selected_file_content_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            load_pull_request_file_content_flow(model, None, cx).await;
        })
        .detach();
}

pub async fn load_pull_request_file_content_flow(
    model: Entity<AppState>,
    requested_path: Option<String>,
    cx: &mut AsyncWindowContext,
) {
    let request = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let lsp_session_manager = state.lsp_session_manager.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            let selected_path = AppState::select_changed_file_path_for_detail(
                &detail,
                requested_path
                    .clone()
                    .or_else(|| state.selected_file_path.clone()),
            )?;
            let selected_file = detail
                .files
                .iter()
                .find(|file| file.path == selected_path)
                .cloned()?;
            let parsed = find_parsed_diff_file(&detail.parsed_diff, &selected_file.path).cloned();
            let request = build_file_content_request(&detail, &selected_file, parsed.as_ref())?;
            let detail_state = state.detail_states.get(&detail_key);

            let file_content_loaded =
                is_local_checkout_file_loaded(detail_state, &request.path, &request.request_key);
            let lsp_loaded = is_lsp_status_loaded(detail_state, &selected_file.path);
            let already_loaded = file_content_loaded && lsp_loaded;

            Some((
                cache,
                lsp_session_manager,
                detail_key,
                detail,
                selected_file,
                request,
                already_loaded,
                existing_local_repo_status,
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        lsp_session_manager,
        detail_key,
        detail,
        selected_file,
        request,
        already_loaded,
        existing_local_repo_status,
    )) = request
    else {
        return;
    };

    log_checkout_flow_event(
        &detail,
        format!(
            "diff file-content flow start; path={}; reference={}; local_reference={}; already_loaded={}; existing_status={}",
            request.path,
            request.reference,
            request.local_reference,
            already_loaded,
            summarize_optional_local_repo_status(existing_local_repo_status.as_ref()),
        ),
    );

    if already_loaded {
        log_checkout_flow_event(
            &detail,
            format!(
                "diff file-content flow skipped; path={}; request_key already loaded",
                request.path
            ),
        );
        return;
    }

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let file_state = detail_state
                    .file_content_states
                    .entry(request.path.clone())
                    .or_default();
                file_state.request_key = Some(request.request_key.clone());
                file_state.document = None;
                file_state.prepared = None;
                file_state.loading = true;
                file_state.error = None;
                detail_state.local_repository_loading = existing_local_repo_status
                    .as_ref()
                    .map(|status| !status.ready_for_snapshot_features())
                    .unwrap_or(true);
                detail_state.local_repository_error = None;
                detail_state
                    .lsp_loading_paths
                    .insert(selected_file.path.clone());
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };
    log_local_repo_result(&detail, "diff file-content flow", &local_repo_result);

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let local_load_result = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                log_checkout_flow_event(
                    &detail,
                    format!(
                        "diff file-content local document load start; root={root}; path={}; local_reference={}; prefer_worktree={}",
                        request.path,
                        request.local_reference,
                        request.prefer_worktree && status.should_prefer_worktree_contents(),
                    ),
                );
                cx.background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = detail.repository.clone();
                        let path = request.path.clone();
                        let reference = request.local_reference.clone();
                        let prefer_worktree =
                            request.prefer_worktree && status.should_prefer_worktree_contents();
                        let root = std::path::PathBuf::from(root);
                        async move {
                            local_documents::load_local_repository_file_content(
                                &cache,
                                &repository,
                                &root,
                                &reference,
                                &path,
                                prefer_worktree,
                            )
                        }
                    })
                    .await
            } else {
                Err(status.message.clone())
            }
        } else {
            Err(local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
        }
    } else {
        Err(local_repo_error
            .clone()
            .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
    };
    log_document_result(
        &detail,
        "diff file-content local document",
        &request.path,
        &request.local_reference,
        &local_load_result,
    );

    let load_result = match local_load_result {
        Ok(document) => Ok(document),
        Err(local_error) => {
            log_checkout_flow_event(
                &detail,
                format!(
                    "diff file-content GitHub fallback start; path={}; reference={}; local_error={}",
                    request.path,
                    request.reference,
                    sanitize_checkout_log_text(&local_error),
                ),
            );
            cx.background_executor()
                .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let path = request.path.clone();
                let reference = request.reference.clone();
                async move {
                    github::load_pull_request_file_content(&cache, &repository, &reference, &path)
                        .map_err(|github_error| {
                            format!(
                                "{local_error}\nGitHub fallback also failed for {repository}@{reference}:{path}: {github_error}"
                            )
                        })
                }
            })
            .await
        }
    };
    log_document_result(
        &detail,
        "diff file-content final document",
        &request.path,
        &request.reference,
        &load_result,
    );
    let lsp_status = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                log_checkout_flow_event(
                    &detail,
                    format!(
                        "diff file-content lsp status start; root={root}; path={}",
                        selected_file.path
                    ),
                );
                cx.background_executor()
                    .spawn({
                        let lsp_session_manager = lsp_session_manager.clone();
                        let root = std::path::PathBuf::from(root);
                        let file_path = selected_file.path.clone();
                        async move { lsp_session_manager.status_for_file(&root, &file_path) }
                    })
                    .await
            } else {
                lsp::LspServerStatus::checkout_unavailable(status.message.clone())
            }
        } else {
            lsp::LspServerStatus::checkout_unavailable(
                local_repo_error
                    .clone()
                    .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
            )
        }
    } else {
        lsp::LspServerStatus::checkout_unavailable(
            local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
        )
    };
    log_lsp_result(
        &detail,
        "diff file-content",
        &selected_file.path,
        &lsp_status,
    );

    let prepared_result = load_result.map(|document| {
        let prepared = prepare_file_content(&selected_file.path, &request.reference, &document);
        (document, prepared)
    });

    model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return;
            };
            let Some(file_state) = detail_state.file_content_states.get_mut(&request.path) else {
                return;
            };
            if file_state.request_key.as_deref() != Some(&request.request_key) {
                return;
            }

            file_state.loading = false;
            detail_state.local_repository_loading = false;
            detail_state.local_repository_status = local_repo_status.clone();
            detail_state.local_repository_error = local_repo_error.clone();
            detail_state.lsp_loading_paths.remove(&selected_file.path);
            detail_state
                .lsp_statuses
                .insert(selected_file.path.clone(), lsp_status.clone());
            match prepared_result {
                Ok((document, prepared)) => {
                    file_state.document = Some(document);
                    file_state.prepared = Some(prepared);
                    file_state.error = None;
                }
                Err(error) => {
                    file_state.document = None;
                    file_state.prepared = None;
                    file_state.error = Some(error);
                }
            }

            cx.notify();
        })
        .ok();
}

pub async fn load_structural_diff_flow(
    model: Entity<AppState>,
    requested_path: Option<String>,
    cx: &mut AsyncWindowContext,
) {
    let initial = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            let selected_path = AppState::select_changed_file_path_for_detail(
                &detail,
                requested_path
                    .clone()
                    .or_else(|| state.selected_file_path.clone()),
            )?;
            let selected_file = detail
                .files
                .iter()
                .find(|file| file.path == selected_path)
                .cloned()?;
            let parsed = find_parsed_diff_file(&detail.parsed_diff, &selected_file.path).cloned();

            Some((
                cache,
                detail_key,
                detail,
                selected_file,
                parsed,
                existing_local_repo_status,
            ))
        })
        .ok()
        .flatten();

    let Some((cache, detail_key, detail, selected_file, parsed, existing_local_repo_status)) =
        initial
    else {
        return;
    };

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = existing_local_repo_status
                    .as_ref()
                    .map(|status| !status.ready_for_snapshot_features())
                    .unwrap_or(true);
                detail_state.local_repository_error = None;
            }
            cx.notify();
        })
        .ok();

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let Some(local_repo_status) =
        local_repo_status.filter(|status| status.ready_for_snapshot_features())
    else {
        model
            .update(cx, |state, cx| {
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    detail_state.local_repository_loading = false;
                    detail_state.local_repository_status = local_repo_result.as_ref().ok().cloned();
                    detail_state.local_repository_error = local_repo_error.clone();
                    let structural_state = detail_state
                        .structural_diff_states
                        .entry(selected_file.path.clone())
                        .or_default();
                    structural_state.loading = false;
                    structural_state.diff = None;
                    structural_state.error = Some(local_repo_error.clone().unwrap_or_else(|| {
                        "Local checkout is not ready for structural diffs.".to_string()
                    }));
                    structural_state.terminal_error = false;
                }
                cx.notify();
            })
            .ok();
        return;
    };

    let Some(head_oid) = checkout_head_oid(&local_repo_status) else {
        model
            .update(cx, |state, cx| {
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    detail_state.local_repository_loading = false;
                    detail_state.local_repository_status = Some(local_repo_status.clone());
                    detail_state.local_repository_error = Some(local_repo_status.message.clone());
                    let structural_state = detail_state
                        .structural_diff_states
                        .entry(selected_file.path.clone())
                        .or_default();
                    structural_state.loading = false;
                    structural_state.diff = None;
                    structural_state.error = Some(local_repo_status.message.clone());
                    structural_state.terminal_error = false;
                }
                cx.notify();
            })
            .ok();
        return;
    };

    let Some(request) =
        build_structural_diff_request(&detail, &selected_file, parsed.as_ref(), &head_oid)
    else {
        model
            .update(cx, |state, cx| {
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    detail_state.local_repository_loading = false;
                    detail_state.local_repository_status = Some(local_repo_status.clone());
                    detail_state.local_repository_error = None;
                    let structural_state = detail_state
                        .structural_diff_states
                        .entry(selected_file.path.clone())
                        .or_default();
                    structural_state.loading = false;
                    structural_state.diff = None;
                    structural_state.error = Some(
                        "Structural diff needs PR base and checkout head commits.".to_string(),
                    );
                    structural_state.terminal_error = false;
                }
                cx.notify();
            })
            .ok();
        return;
    };

    let already_loaded = model
        .read_with(cx, |state, _| {
            state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.structural_diff_states.get(&request.path))
                .map(|file_state| {
                    should_reuse_structural_diff_state(file_state, &request.request_key)
                })
                .unwrap_or(false)
        })
        .ok()
        .unwrap_or(false);

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_status = Some(local_repo_status.clone());
                detail_state.local_repository_error = None;
            }
            cx.notify();
        })
        .ok();

    if already_loaded {
        return;
    }

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let structural_state = detail_state
                    .structural_diff_states
                    .entry(request.path.clone())
                    .or_default();
                structural_state.request_key = Some(request.request_key.clone());
                structural_state.diff = None;
                structural_state.loading = true;
                structural_state.error = None;
                structural_state.terminal_error = false;
            }
            cx.notify();
        })
        .ok();

    let cached_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let cache_key = request.cache_key.clone();
            async move { load_cached_structural_diff(cache.as_ref(), &cache_key) }
        })
        .await;

    if let Ok(Some(cached)) = cached_result {
        let result = structural_result_from_cached(cached);
        model
            .update(cx, |state, cx| {
                let active_pr_key = state.active_pr_key.clone();
                apply_structural_diff_file_result(
                    state,
                    active_pr_key.as_deref(),
                    &detail_key,
                    &request,
                    result,
                );
                cx.notify();
            })
            .ok();
        return;
    }

    let Some(checkout_root) = local_repo_status.path.as_deref().map(PathBuf::from) else {
        return;
    };

    let result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = detail.repository.clone();
            let checkout_root = checkout_root.clone();
            let request = request.clone();
            async move {
                build_and_cache_structural_diff(
                    cache.as_ref(),
                    repository.as_str(),
                    checkout_root.as_path(),
                    &request,
                )
            }
        })
        .await;

    model
        .update(cx, |state, cx| {
            let active_pr_key = state.active_pr_key.clone();
            apply_structural_diff_file_result(
                state,
                active_pr_key.as_deref(),
                &detail_key,
                &request,
                result,
            );
            cx.notify();
        })
        .ok();
}

pub async fn warm_structural_diffs_flow(model: Entity<AppState>, cx: &mut AsyncWindowContext) {
    let initial = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());

            Some((cache, detail_key, detail, existing_local_repo_status))
        })
        .ok()
        .flatten();

    let Some((cache, detail_key, detail, existing_local_repo_status)) = initial else {
        return;
    };

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let Some(local_repo_status) =
        local_repo_status.filter(|status| status.ready_for_snapshot_features())
    else {
        model
            .update(cx, |state, cx| {
                if state.active_pr_key.as_deref() != Some(detail_key.as_str()) {
                    return;
                }
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    detail_state.local_repository_loading = false;
                    detail_state.local_repository_status = local_repo_result.as_ref().ok().cloned();
                    detail_state.local_repository_error = local_repo_error.clone();
                    detail_state.structural_diff_warmup.loading = false;
                }
                cx.notify();
            })
            .ok();
        return;
    };

    let Some(head_oid) = checkout_head_oid(&local_repo_status) else {
        return;
    };
    let Some(checkout_root) = local_repo_status.path.as_deref().map(PathBuf::from) else {
        return;
    };

    let requests = detail
        .files
        .iter()
        .filter_map(|file| {
            let parsed = find_parsed_diff_file(&detail.parsed_diff, &file.path);
            build_structural_diff_request(&detail, file, parsed, &head_oid)
        })
        .collect::<Vec<_>>();
    let total = requests.len();
    if total == 0 {
        return;
    }

    let warmup_key = structural_diff_warmup_request_key(&detail, &head_oid);
    let preloaded = model
        .read_with(cx, |state, _| {
            state
                .detail_states
                .get(&detail_key)
                .map(|detail_state| {
                    requests
                        .iter()
                        .map(|request| {
                            (
                                request.request_key.clone(),
                                detail_state
                                    .structural_diff_states
                                    .get(&request.path)
                                    .and_then(|file_state| {
                                        structural_diff_state_terminal_status(
                                            file_state,
                                            &request.request_key,
                                        )
                                    }),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .ok()
        .unwrap_or_default();

    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut pending = VecDeque::new();
    for request in requests {
        let status = preloaded
            .iter()
            .find(|(request_key, _)| request_key == &request.request_key)
            .and_then(|(_, status)| *status);
        match status {
            Some(StructuralDiffTerminalStatus::Ready) => completed += 1,
            Some(StructuralDiffTerminalStatus::Error) => failed += 1,
            None => pending.push_back(request),
        }
    }

    let should_run = model
        .update(cx, |state, cx| {
            if state.active_pr_key.as_deref() != Some(detail_key.as_str()) {
                return false;
            }

            let detail_state = state.detail_states.entry(detail_key.clone()).or_default();
            if detail_state.structural_diff_warmup.request_key.as_deref()
                == Some(warmup_key.as_str())
                && (detail_state.structural_diff_warmup.loading
                    || detail_state.structural_diff_warmup.completed
                        + detail_state.structural_diff_warmup.failed
                        >= total)
            {
                return false;
            }

            detail_state.local_repository_loading = false;
            detail_state.local_repository_status = Some(local_repo_status.clone());
            detail_state.local_repository_error = None;
            detail_state.structural_diff_warmup.request_key = Some(warmup_key.clone());
            detail_state.structural_diff_warmup.total = total;
            detail_state.structural_diff_warmup.completed = completed;
            detail_state.structural_diff_warmup.failed = failed;
            detail_state.structural_diff_warmup.loading = !pending.is_empty();

            for request in &pending {
                let structural_state = detail_state
                    .structural_diff_states
                    .entry(request.path.clone())
                    .or_default();
                structural_state.request_key = Some(request.request_key.clone());
                structural_state.diff = None;
                structural_state.loading = true;
                structural_state.error = None;
                structural_state.terminal_error = false;
            }

            cx.notify();
            !pending.is_empty()
        })
        .ok()
        .unwrap_or(false);

    if !should_run {
        return;
    }

    while !pending.is_empty() {
        let mut batch = Vec::new();
        for _ in 0..STRUCTURAL_DIFF_WARMUP_CONCURRENCY {
            let Some(request) = pending.pop_front() else {
                break;
            };
            let task = cx.background_executor().spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let checkout_root = checkout_root.clone();
                let request = request.clone();
                async move {
                    load_cached_structural_diff(cache.as_ref(), &request.cache_key)
                        .ok()
                        .flatten()
                        .map(structural_result_from_cached)
                        .unwrap_or_else(|| {
                            build_and_cache_structural_diff(
                                cache.as_ref(),
                                repository.as_str(),
                                checkout_root.as_path(),
                                &request,
                            )
                        })
                }
            });
            batch.push((request, task));
        }

        for (request, task) in batch {
            let result = task.await;
            model
                .update(cx, |state, cx| {
                    let active_pr_key = state.active_pr_key.clone();
                    if active_pr_key.as_deref() != Some(detail_key.as_str()) {
                        return;
                    }
                    let warmup_matches = state
                        .detail_states
                        .get(&detail_key)
                        .map(|detail_state| {
                            detail_state.structural_diff_warmup.request_key.as_deref()
                                == Some(warmup_key.as_str())
                        })
                        .unwrap_or(false);
                    if !warmup_matches {
                        return;
                    }

                    let is_ready = matches!(&result, StructuralDiffBuildResult::Ready(_));
                    let is_terminal_error =
                        matches!(&result, StructuralDiffBuildResult::TerminalError(_));
                    let should_count = apply_structural_diff_file_result(
                        state,
                        active_pr_key.as_deref(),
                        &detail_key,
                        &request,
                        result,
                    );
                    if should_count {
                        let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                            return;
                        };
                        if detail_state.structural_diff_warmup.request_key.as_deref()
                            != Some(warmup_key.as_str())
                        {
                            return;
                        }
                        if is_ready {
                            detail_state.structural_diff_warmup.completed += 1;
                        } else if is_terminal_error {
                            detail_state.structural_diff_warmup.failed += 1;
                        }
                    }
                    cx.notify();
                })
                .ok();
        }
    }

    model
        .update(cx, |state, cx| {
            if state.active_pr_key.as_deref() != Some(detail_key.as_str()) {
                return;
            }
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return;
            };
            if detail_state.structural_diff_warmup.request_key.as_deref()
                != Some(warmup_key.as_str())
            {
                return;
            }

            detail_state.structural_diff_warmup.loading = false;
            cx.notify();
        })
        .ok();
}

pub async fn load_source_file_tree_flow(model: Entity<AppState>, cx: &mut AsyncWindowContext) {
    let request = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let reference = detail
                .head_ref_oid
                .clone()
                .unwrap_or_else(|| detail.head_ref_name.clone());
            if reference.is_empty() {
                return None;
            }

            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            let request_key = source_file_tree_request_key(&detail, &reference);
            let already_loaded = state
                .detail_states
                .get(&detail_key)
                .map(|detail_state| {
                    detail_state.source_file_tree.request_key.as_deref()
                        == Some(request_key.as_str())
                        && (detail_state.source_file_tree.loading
                            || detail_state.source_file_tree.rows.is_some())
                })
                .unwrap_or(false);

            Some((
                cache,
                detail_key,
                detail,
                reference,
                request_key,
                already_loaded,
                existing_local_repo_status,
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        detail_key,
        detail,
        reference,
        request_key,
        already_loaded,
        existing_local_repo_status,
    )) = request
    else {
        return;
    };

    log_checkout_flow_event(
        &detail,
        format!(
            "source file-tree flow start; reference={reference}; already_loaded={already_loaded}; existing_status={}",
            summarize_optional_local_repo_status(existing_local_repo_status.as_ref()),
        ),
    );

    if already_loaded {
        log_checkout_flow_event(
            &detail,
            format!(
                "source file-tree flow skipped; reference={reference}; request_key already loaded"
            ),
        );
        return;
    }

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.source_file_tree.request_key = Some(request_key.clone());
                detail_state.source_file_tree.rows = None;
                detail_state.source_file_tree.file_count = 0;
                detail_state.source_file_tree.loading = true;
                detail_state.source_file_tree.error = None;
                detail_state.local_repository_loading = existing_local_repo_status
                    .as_ref()
                    .map(|status| !status.ready_for_snapshot_features())
                    .unwrap_or(true);
                detail_state.local_repository_error = None;
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };
    log_local_repo_result(&detail, "source file-tree flow", &local_repo_result);

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let tree_result = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                log_checkout_flow_event(
                    &detail,
                    format!("source file-tree list start; root={root}; reference={reference}"),
                );
                cx.background_executor()
                    .spawn({
                        let root = std::path::PathBuf::from(root);
                        let reference = reference.clone();
                        async move { local_documents::list_local_repository_files(&root, &reference) }
                    })
                    .await
            } else {
                Err(status.message.clone())
            }
        } else {
            Err(local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
        }
    } else {
        Err(local_repo_error
            .clone()
            .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
    };
    match &tree_result {
        Ok(paths) => log_checkout_flow_event(
            &detail,
            format!(
                "source file-tree list finish; reference={reference}; path_count={}",
                paths.len()
            ),
        ),
        Err(error) => log_checkout_flow_event(
            &detail,
            format!(
                "source file-tree list failed; reference={reference}; error={}",
                sanitize_checkout_log_text(error)
            ),
        ),
    }

    let tree_result = tree_result.map(|paths| {
        let file_count = paths.len();
        let rows = Arc::new(build_repository_file_tree_rows(&paths, &detail.files));
        (rows, file_count)
    });

    model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return;
            };
            if detail_state.source_file_tree.request_key.as_deref() != Some(request_key.as_str()) {
                return;
            }

            detail_state.source_file_tree.loading = false;
            detail_state.local_repository_loading = false;
            detail_state.local_repository_status = local_repo_status.clone();
            detail_state.local_repository_error = local_repo_error.clone();
            match tree_result {
                Ok((rows, file_count)) => {
                    detail_state.source_file_tree.rows = Some(rows);
                    detail_state.source_file_tree.file_count = file_count;
                    detail_state.source_file_tree.error = None;
                }
                Err(error) => {
                    detail_state.source_file_tree.rows = None;
                    detail_state.source_file_tree.file_count = 0;
                    detail_state.source_file_tree.error = Some(error);
                }
            }

            cx.notify();
        })
        .ok();
}

fn source_file_tree_request_key(detail: &PullRequestDetail, reference: &str) -> String {
    format!(
        "{}:{}:{reference}:source-file-tree",
        detail.updated_at, detail.repository
    )
}

fn log_checkout_flow_event(detail: &PullRequestDetail, message: impl AsRef<str>) {
    let _ = local_repo::log_checkout_event(
        &detail.repository,
        detail.number,
        detail.head_ref_oid.as_deref(),
        message,
    );
}

fn log_local_repo_result(
    detail: &PullRequestDetail,
    stage: &str,
    result: &Result<local_repo::LocalRepositoryStatus, String>,
) {
    match result {
        Ok(status) => log_checkout_flow_event(
            detail,
            format!(
                "{stage}: local repo result ok; {}",
                summarize_local_repo_status(status)
            ),
        ),
        Err(error) => log_checkout_flow_event(
            detail,
            format!("{stage}: local repo result error; error={error}"),
        ),
    }
}

fn summarize_optional_local_repo_status(
    status: Option<&local_repo::LocalRepositoryStatus>,
) -> String {
    status
        .map(summarize_local_repo_status)
        .unwrap_or_else(|| "<none>".to_string())
}

fn summarize_local_repo_status(status: &local_repo::LocalRepositoryStatus) -> String {
    format!(
        "source={}; path={}; valid={}; current_head={}; expected_head={}; matches_expected_head={}; clean={}; ready={}; message=\"{}\"",
        status.source,
        status.path.as_deref().unwrap_or("<none>"),
        status.is_valid_repository,
        status.current_head_oid.as_deref().unwrap_or("<none>"),
        status.expected_head_oid.as_deref().unwrap_or("<none>"),
        status.matches_expected_head,
        status.is_worktree_clean,
        status.ready_for_local_features,
        sanitize_checkout_log_text(&status.message),
    )
}

fn log_document_result(
    detail: &PullRequestDetail,
    stage: &str,
    path: &str,
    reference: &str,
    result: &Result<RepositoryFileContent, String>,
) {
    match result {
        Ok(document) => log_checkout_flow_event(
            detail,
            format!(
                "{stage}: document load ok; path={path}; reference={reference}; source={}; size_bytes={}; binary={}",
                document.source, document.size_bytes, document.is_binary
            ),
        ),
        Err(error) => log_checkout_flow_event(
            detail,
            format!(
                "{stage}: document load error; path={path}; reference={reference}; error={}",
                sanitize_checkout_log_text(error)
            ),
        ),
    }
}

fn log_lsp_result(
    detail: &PullRequestDetail,
    stage: &str,
    path: &str,
    status: &lsp::LspServerStatus,
) {
    log_checkout_flow_event(
        detail,
        format!(
            "{stage}: lsp status finish; path={path}; state={:?}; language={}; command={}; message=\"{}\"",
            status.state,
            status.language_id.as_deref().unwrap_or("<none>"),
            status.command.as_deref().unwrap_or("<none>"),
            sanitize_checkout_log_text(&status.message),
        ),
    );
}

fn sanitize_checkout_log_text(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

pub async fn load_local_source_file_content_flow(
    model: Entity<AppState>,
    requested_path: String,
    cx: &mut AsyncWindowContext,
) {
    let request = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let lsp_session_manager = state.lsp_session_manager.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            let request = build_head_file_content_request(&detail, &requested_path)?;
            let detail_state = state.detail_states.get(&detail_key);

            let file_content_loaded =
                is_local_checkout_file_loaded(detail_state, &request.path, &request.request_key);
            let lsp_loaded = is_lsp_status_loaded(detail_state, &request.path);
            let already_loaded = file_content_loaded && lsp_loaded;

            Some((
                cache,
                lsp_session_manager,
                detail_key,
                detail,
                request,
                already_loaded,
                existing_local_repo_status,
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        lsp_session_manager,
        detail_key,
        detail,
        request,
        already_loaded,
        existing_local_repo_status,
    )) = request
    else {
        return;
    };

    log_checkout_flow_event(
        &detail,
        format!(
            "source content flow start; path={}; reference={}; local_reference={}; already_loaded={}; existing_status={}",
            request.path,
            request.reference,
            request.local_reference,
            already_loaded,
            summarize_optional_local_repo_status(existing_local_repo_status.as_ref()),
        ),
    );

    if already_loaded {
        log_checkout_flow_event(
            &detail,
            format!(
                "source content flow skipped; path={}; request_key already loaded",
                request.path
            ),
        );
        return;
    }

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let file_state = detail_state
                    .file_content_states
                    .entry(request.path.clone())
                    .or_default();
                file_state.request_key = Some(request.request_key.clone());
                file_state.document = None;
                file_state.prepared = None;
                file_state.loading = true;
                file_state.error = None;
                detail_state.local_repository_loading = existing_local_repo_status
                    .as_ref()
                    .map(|status| !status.ready_for_snapshot_features())
                    .unwrap_or(true);
                detail_state.local_repository_error = None;
                detail_state.lsp_loading_paths.insert(request.path.clone());
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };
    log_local_repo_result(&detail, "source content flow", &local_repo_result);

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let local_load_result = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                log_checkout_flow_event(
                    &detail,
                    format!(
                        "source content local document load start; root={root}; path={}; local_reference={}; prefer_worktree={}",
                        request.path,
                        request.local_reference,
                        request.prefer_worktree && status.should_prefer_worktree_contents(),
                    ),
                );
                cx.background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = detail.repository.clone();
                        let path = request.path.clone();
                        let reference = request.local_reference.clone();
                        let prefer_worktree =
                            request.prefer_worktree && status.should_prefer_worktree_contents();
                        let root = std::path::PathBuf::from(root);
                        async move {
                            local_documents::load_local_repository_file_content(
                                &cache,
                                &repository,
                                &root,
                                &reference,
                                &path,
                                prefer_worktree,
                            )
                        }
                    })
                    .await
            } else {
                Err(status.message.clone())
            }
        } else {
            Err(local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
        }
    } else {
        Err(local_repo_error
            .clone()
            .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
    };
    log_document_result(
        &detail,
        "source content local document",
        &request.path,
        &request.local_reference,
        &local_load_result,
    );

    let load_result = match local_load_result {
        Ok(document) => Ok(document),
        Err(local_error) => {
            log_checkout_flow_event(
                &detail,
                format!(
                    "source content GitHub fallback start; path={}; reference={}; local_error={}",
                    request.path,
                    request.reference,
                    sanitize_checkout_log_text(&local_error),
                ),
            );
            cx.background_executor()
                .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let path = request.path.clone();
                let reference = request.reference.clone();
                async move {
                    github::load_pull_request_file_content(&cache, &repository, &reference, &path)
                        .map_err(|github_error| {
                            format!(
                                "{local_error}\nGitHub fallback also failed for {repository}@{reference}:{path}: {github_error}"
                            )
                        })
                }
            })
            .await
        }
    };
    log_document_result(
        &detail,
        "source content final document",
        &request.path,
        &request.reference,
        &load_result,
    );
    let lsp_status = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                log_checkout_flow_event(
                    &detail,
                    format!(
                        "source content lsp status start; root={root}; path={}",
                        request.path
                    ),
                );
                cx.background_executor()
                    .spawn({
                        let lsp_session_manager = lsp_session_manager.clone();
                        let root = std::path::PathBuf::from(root);
                        let file_path = request.path.clone();
                        async move { lsp_session_manager.status_for_file(&root, &file_path) }
                    })
                    .await
            } else {
                lsp::LspServerStatus::checkout_unavailable(status.message.clone())
            }
        } else {
            lsp::LspServerStatus::checkout_unavailable(
                local_repo_error
                    .clone()
                    .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
            )
        }
    } else {
        lsp::LspServerStatus::checkout_unavailable(
            local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
        )
    };
    log_lsp_result(&detail, "source content", &request.path, &lsp_status);

    let prepared_result = load_result.map(|document| {
        let prepared = prepare_file_content(&request.path, &request.reference, &document);
        (document, prepared)
    });

    model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return;
            };
            let Some(file_state) = detail_state.file_content_states.get_mut(&request.path) else {
                return;
            };
            if file_state.request_key.as_deref() != Some(&request.request_key) {
                return;
            }

            file_state.loading = false;
            detail_state.local_repository_loading = false;
            detail_state.local_repository_status = local_repo_status.clone();
            detail_state.local_repository_error = local_repo_error.clone();
            detail_state.lsp_loading_paths.remove(&request.path);
            detail_state
                .lsp_statuses
                .insert(request.path.clone(), lsp_status.clone());
            match prepared_result {
                Ok((document, prepared)) => {
                    file_state.document = Some(document);
                    file_state.prepared = Some(prepared);
                    file_state.error = None;
                }
                Err(error) => {
                    file_state.document = None;
                    file_state.prepared = None;
                    file_state.error = Some(error);
                }
            }

            cx.notify();
        })
        .ok();
}

pub async fn load_temp_source_file_content_flow(
    model: Entity<AppState>,
    target: TempSourceTarget,
    cx: &mut AsyncWindowContext,
) {
    let request = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let lsp_session_manager = state.lsp_session_manager.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repo_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            let request = build_temp_source_file_content_request(&detail, &target)?;
            let detail_state = state.detail_states.get(&detail_key);
            let content_loaded = state.temp_source_window.request_key.as_deref()
                == Some(request.request_key.as_str())
                && state.temp_source_window.prepared.is_some()
                && state.temp_source_window.error.is_none();
            let lsp_loaded = target.side == TempSourceSide::Base
                || is_lsp_status_loaded(detail_state, &target.path);
            let already_loaded = content_loaded && lsp_loaded;

            Some((
                cache,
                lsp_session_manager,
                detail_key,
                detail,
                request,
                already_loaded,
                existing_local_repo_status,
                target.clone(),
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        lsp_session_manager,
        detail_key,
        detail,
        request,
        already_loaded,
        existing_local_repo_status,
        target,
    )) = request
    else {
        return;
    };

    if already_loaded {
        model
            .update(cx, |state, cx| {
                if state.temp_source_window.request_key.as_deref() == Some(&request.request_key) {
                    state.temp_source_window.loading = false;
                    state.temp_source_window.error = None;
                    cx.notify();
                }
            })
            .ok();
        return;
    }

    model
        .update(cx, |state, cx| {
            state.temp_source_window.target = Some(target.clone());
            state.temp_source_window.request_key = Some(request.request_key.clone());
            state.temp_source_window.document = None;
            state.temp_source_window.prepared = None;
            state.temp_source_window.loading = true;
            state.temp_source_window.error = None;

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = existing_local_repo_status
                    .as_ref()
                    .map(|status| !status.ready_for_snapshot_features())
                    .unwrap_or(true);
                detail_state.local_repository_error = None;
                if target.side == TempSourceSide::Head {
                    detail_state.lsp_loading_paths.insert(target.path.clone());
                }
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = if let Some(status) = existing_local_repo_status
        .clone()
        .filter(|status| status.ready_for_snapshot_features())
    {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    local_repo::load_or_prepare_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                }
            })
            .await
    };

    let local_repo_status = local_repo_result.as_ref().ok().cloned();
    let local_repo_error = local_repo_result
        .as_ref()
        .ok()
        .and_then(|status| {
            if status.ready_for_snapshot_features() {
                None
            } else {
                Some(status.message.clone())
            }
        })
        .or_else(|| local_repo_result.as_ref().err().cloned());

    let local_load_result = if let Some(status) = local_repo_status.as_ref() {
        if status.ready_for_snapshot_features() {
            if let Some(root) = status.path.as_deref() {
                cx.background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = detail.repository.clone();
                        let path = request.path.clone();
                        let reference = request.local_reference.clone();
                        let prefer_worktree =
                            request.prefer_worktree && status.should_prefer_worktree_contents();
                        let root = std::path::PathBuf::from(root);
                        async move {
                            local_documents::load_local_repository_file_content(
                                &cache,
                                &repository,
                                &root,
                                &reference,
                                &path,
                                prefer_worktree,
                            )
                        }
                    })
                    .await
            } else {
                Err(status.message.clone())
            }
        } else {
            Err(local_repo_error
                .clone()
                .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
        }
    } else {
        Err(local_repo_error
            .clone()
            .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()))
    };

    let load_result = match local_load_result {
        Ok(document) => Ok(document),
        Err(local_error) => cx
            .background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let path = request.path.clone();
                let reference = request.reference.clone();
                async move {
                    github::load_pull_request_file_content(&cache, &repository, &reference, &path)
                        .map_err(|github_error| {
                            format!(
                                "{local_error}\nGitHub fallback also failed for {repository}@{reference}:{path}: {github_error}"
                            )
                        })
                }
            })
            .await,
    };

    let lsp_status = if target.side == TempSourceSide::Head {
        Some(if let Some(status) = local_repo_status.as_ref() {
            if status.ready_for_snapshot_features() {
                if let Some(root) = status.path.as_deref() {
                    cx.background_executor()
                        .spawn({
                            let lsp_session_manager = lsp_session_manager.clone();
                            let root = std::path::PathBuf::from(root);
                            let file_path = target.path.clone();
                            async move { lsp_session_manager.status_for_file(&root, &file_path) }
                        })
                        .await
                } else {
                    lsp::LspServerStatus::checkout_unavailable(status.message.clone())
                }
            } else {
                lsp::LspServerStatus::checkout_unavailable(
                    local_repo_error
                        .clone()
                        .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
                )
            }
        } else {
            lsp::LspServerStatus::checkout_unavailable(
                local_repo_error
                    .clone()
                    .unwrap_or_else(|| "Local checkout is not ready yet.".to_string()),
            )
        })
    } else {
        None
    };

    let prepared_result = load_result.map(|document| {
        let prepared = prepare_file_content(&request.path, &request.reference, &document);
        (document, prepared)
    });

    model
        .update(cx, |state, cx| {
            if state.temp_source_window.request_key.as_deref() != Some(&request.request_key) {
                return;
            }

            state.temp_source_window.loading = false;
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_status = local_repo_status.clone();
                detail_state.local_repository_error = local_repo_error.clone();
                if target.side == TempSourceSide::Head {
                    detail_state.lsp_loading_paths.remove(&target.path);
                }
                if let Some(lsp_status) = lsp_status.clone() {
                    detail_state
                        .lsp_statuses
                        .insert(target.path.clone(), lsp_status);
                }
            }

            match prepared_result {
                Ok((document, prepared)) => {
                    state.temp_source_window.document = Some(document);
                    state.temp_source_window.prepared = Some(prepared);
                    state.temp_source_window.error = None;
                }
                Err(error) => {
                    state.temp_source_window.document = None;
                    state.temp_source_window.prepared = None;
                    state.temp_source_window.error = Some(error);
                }
            }

            cx.notify();
        })
        .ok();
}

#[derive(Clone)]
struct FileContentRequest {
    path: String,
    reference: String,
    local_reference: String,
    prefer_worktree: bool,
    request_key: String,
}

const STRUCTURAL_DIFF_WARMUP_CONCURRENCY: usize = 2;

fn build_file_content_request(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
) -> Option<FileContentRequest> {
    let (path, reference, local_reference, prefer_worktree) = if file.change_type == "DELETED" {
        (
            parsed
                .and_then(|parsed| parsed.previous_path.clone())
                .unwrap_or_else(|| file.path.clone()),
            detail
                .base_ref_oid
                .clone()
                .unwrap_or_else(|| detail.base_ref_name.clone()),
            detail
                .base_ref_oid
                .clone()
                .unwrap_or_else(|| detail.base_ref_name.clone()),
            false,
        )
    } else {
        (
            file.path.clone(),
            detail
                .head_ref_oid
                .clone()
                .unwrap_or_else(|| detail.head_ref_name.clone()),
            detail
                .head_ref_oid
                .clone()
                .unwrap_or_else(|| "HEAD".to_string()),
            true,
        )
    };

    if path.is_empty() || reference.is_empty() || local_reference.is_empty() {
        return None;
    }

    Some(FileContentRequest {
        request_key: format!(
            "{}:{reference}:{path}:{}",
            detail.updated_at, detail.repository
        ),
        path,
        reference,
        local_reference,
        prefer_worktree,
    })
}

pub(super) fn should_reuse_structural_diff_state(
    file_state: &StructuralDiffFileState,
    request_key: &str,
) -> bool {
    file_state.request_key.as_deref() == Some(request_key)
        && (file_state.loading || file_state.diff.is_some() || file_state.terminal_error)
}

pub(super) fn structural_diff_state_terminal_status(
    file_state: &StructuralDiffFileState,
    request_key: &str,
) -> Option<StructuralDiffTerminalStatus> {
    if file_state.request_key.as_deref() != Some(request_key) {
        return None;
    }
    if file_state.diff.is_some() {
        return Some(StructuralDiffTerminalStatus::Ready);
    }
    if file_state.terminal_error {
        return Some(StructuralDiffTerminalStatus::Error);
    }
    None
}

pub(super) fn should_apply_structural_diff_update(
    active_pr_key: Option<&str>,
    detail_key: &str,
    current_request_key: Option<&str>,
    request_key: &str,
) -> bool {
    active_pr_key == Some(detail_key) && current_request_key == Some(request_key)
}

fn apply_structural_diff_file_result(
    state: &mut AppState,
    active_pr_key: Option<&str>,
    detail_key: &str,
    request: &StructuralDiffRequest,
    result: StructuralDiffBuildResult,
) -> bool {
    let Some(detail_state) = state.detail_states.get_mut(detail_key) else {
        return false;
    };
    let structural_state = detail_state
        .structural_diff_states
        .entry(request.path.clone())
        .or_default();
    if !should_apply_structural_diff_update(
        active_pr_key,
        detail_key,
        structural_state.request_key.as_deref(),
        &request.request_key,
    ) {
        return false;
    }

    structural_state.loading = false;
    match result {
        StructuralDiffBuildResult::Ready(diff) => {
            structural_state.diff = Some(Arc::new(diff));
            structural_state.error = None;
            structural_state.terminal_error = false;
        }
        StructuralDiffBuildResult::TerminalError(error) => {
            structural_state.diff = None;
            structural_state.error = Some(error);
            structural_state.terminal_error = true;
        }
        StructuralDiffBuildResult::TransientError(error) => {
            structural_state.diff = None;
            structural_state.error = Some(error);
            structural_state.terminal_error = false;
        }
    }

    true
}

fn build_head_file_content_request(
    detail: &PullRequestDetail,
    path: &str,
) -> Option<FileContentRequest> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    let reference = detail
        .head_ref_oid
        .clone()
        .unwrap_or_else(|| detail.head_ref_name.clone());
    let local_reference = detail
        .head_ref_oid
        .clone()
        .unwrap_or_else(|| "HEAD".to_string());

    if reference.is_empty() || local_reference.is_empty() {
        return None;
    }

    Some(FileContentRequest {
        request_key: format!(
            "{}:{reference}:{path}:{}",
            detail.updated_at, detail.repository
        ),
        path: path.to_string(),
        reference,
        local_reference,
        prefer_worktree: true,
    })
}

fn build_temp_source_file_content_request(
    detail: &PullRequestDetail,
    target: &TempSourceTarget,
) -> Option<FileContentRequest> {
    let path = target.path.trim();
    let reference = target.reference.trim();
    if path.is_empty() || reference.is_empty() {
        return None;
    }

    let local_reference = match target.side {
        TempSourceSide::Head => detail
            .head_ref_oid
            .clone()
            .unwrap_or_else(|| "HEAD".to_string()),
        TempSourceSide::Base => detail
            .base_ref_oid
            .clone()
            .unwrap_or_else(|| detail.base_ref_name.clone()),
    };
    let local_reference = local_reference.trim().to_string();
    if local_reference.is_empty() {
        return None;
    }

    Some(FileContentRequest {
        request_key: crate::temp_source_window::temp_source_request_key(detail, target),
        path: path.to_string(),
        reference: reference.to_string(),
        local_reference,
        prefer_worktree: target.side == TempSourceSide::Head,
    })
}

fn is_local_checkout_file_loaded(
    detail_state: Option<&DetailState>,
    path: &str,
    request_key: &str,
) -> bool {
    detail_state
        .and_then(|detail_state| detail_state.file_content_states.get(path))
        .map(|file_state| {
            file_state.request_key.as_deref() == Some(request_key)
                && (file_state.loading
                    || file_state
                        .document
                        .as_ref()
                        .map(|document| document.source == REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT)
                        .unwrap_or(false))
        })
        .unwrap_or(false)
}

fn is_lsp_status_loaded(detail_state: Option<&DetailState>, path: &str) -> bool {
    detail_state
        .map(|detail_state| {
            detail_state.lsp_loading_paths.contains(path)
                || detail_state.lsp_statuses.contains_key(path)
        })
        .unwrap_or(false)
}

fn prepare_file_content(
    file_path: &str,
    reference: &str,
    document: &RepositoryFileContent,
) -> PreparedFileContent {
    let lines = document.content.as_deref().unwrap_or_default();
    let text_lines = if lines.is_empty() {
        Vec::new()
    } else {
        lines.lines().map(str::to_string).collect::<Vec<_>>()
    };
    let spans = if document.is_binary || document.size_bytes > syntax::MAX_HIGHLIGHT_BYTES {
        text_lines
            .iter()
            .map(|_| Vec::new())
            .collect::<Vec<Vec<SyntaxSpan>>>()
    } else {
        syntax::highlight_lines(file_path, text_lines.iter().map(|line| line.as_str()))
    };

    let prepared_lines = text_lines
        .into_iter()
        .zip(spans)
        .enumerate()
        .map(|(index, (text, spans))| PreparedFileLine {
            line_number: index + 1,
            text,
            spans,
        })
        .collect::<Vec<_>>();

    PreparedFileContent {
        path: file_path.to_string(),
        reference: reference.to_string(),
        is_binary: document.is_binary,
        size_bytes: document.size_bytes,
        text: Arc::<str>::from(document.content.as_deref().unwrap_or_default()),
        lines: Arc::new(prepared_lines),
    }
}
