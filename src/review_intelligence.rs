use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

use gpui::{App, AsyncWindowContext, Entity, Window};
use once_cell::sync::Lazy;

use crate::{
    cache::CacheStore,
    code_tour::{self, build_tour_request_key, tour_code_version_key, CodeTourProvider},
    github::PullRequestDetail,
    local_repo, local_review,
    review_brief::{self, build_review_brief_request_key},
    stacks::{
        cache::{load_ai_review_stack, save_ai_review_stack},
        discover_review_stack,
        model::{Confidence, RepoContext, ReviewStack, StackDiscoveryOptions},
    },
    state::{AppState, DetailState},
};

static REVIEW_INTELLIGENCE_JOB_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static FOREGROUND_REVIEW_INTELLIGENCE_JOBS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewIntelligenceScope {
    All,
    BriefOnly,
    StackOnly,
    TourOnly,
}

impl ReviewIntelligenceScope {
    fn includes_brief(self) -> bool {
        matches!(self, Self::All | Self::BriefOnly)
    }

    fn includes_stack(self) -> bool {
        matches!(self, Self::All | Self::StackOnly)
    }

    fn includes_tour(self) -> bool {
        matches!(self, Self::All | Self::TourOnly)
    }
}

struct ForegroundJobPermit;

impl ForegroundJobPermit {
    fn new() -> Self {
        FOREGROUND_REVIEW_INTELLIGENCE_JOBS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ForegroundJobPermit {
    fn drop(&mut self) {
        FOREGROUND_REVIEW_INTELLIGENCE_JOBS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn run_foreground_blocking<T>(task: impl FnOnce() -> T) -> T {
    let _guard = REVIEW_INTELLIGENCE_JOB_LOCK
        .lock()
        .expect("review intelligence job lock poisoned");
    task()
}

pub fn run_background_blocking<T>(task: impl FnOnce() -> T) -> T {
    loop {
        while FOREGROUND_REVIEW_INTELLIGENCE_JOBS.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(150));
        }

        let _guard = REVIEW_INTELLIGENCE_JOB_LOCK
            .lock()
            .expect("review intelligence job lock poisoned");
        if FOREGROUND_REVIEW_INTELLIGENCE_JOBS.load(Ordering::SeqCst) == 0 {
            return task();
        }
    }
}

pub fn trigger_review_intelligence(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    scope: ReviewIntelligenceScope,
    force: bool,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            run_review_intelligence_flow(model, scope, force, false, cx).await;
        })
        .detach();
}

pub fn refresh_active_review_brief(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    allow_automatic_generation: bool,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            refresh_active_review_brief_flow(model, allow_automatic_generation, cx).await;
        })
        .detach();
}

pub(crate) async fn refresh_active_review_brief_flow(
    model: Entity<AppState>,
    allow_automatic_generation: bool,
    cx: &mut AsyncWindowContext,
) {
    let initial = model
        .read_with(cx, |state, _| {
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            Some((
                state.cache.clone(),
                detail_key,
                detail,
                state.code_tour_settings.loaded,
                state.code_tour_settings.settings.clone(),
                state.code_tour_provider_statuses_loaded,
                state.code_tour_provider_statuses.clone(),
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        detail_key,
        detail,
        settings_loaded,
        existing_settings,
        statuses_loaded,
        existing_statuses,
    )) = initial
    else {
        return;
    };

    if !settings_loaded {
        model
            .update(cx, |state, cx| {
                state.code_tour_settings.loading = true;
                state.code_tour_settings.error = None;
                cx.notify();
            })
            .ok();
    }

    if !statuses_loaded {
        model
            .update(cx, |state, cx| {
                state.code_tour_provider_loading = true;
                state.code_tour_provider_error = None;
                cx.notify();
            })
            .ok();
    }

    let settings_result = if settings_loaded {
        Ok(existing_settings.clone())
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                async move { code_tour::load_code_tour_settings(&cache) }
            })
            .await
    };

    let provider_statuses_result = if statuses_loaded {
        Ok(existing_statuses)
    } else {
        cx.background_executor()
            .spawn(async { code_tour::load_code_tour_provider_statuses() })
            .await
    };

    let settings = settings_result
        .clone()
        .unwrap_or_else(|_| existing_settings.clone());
    let provider = settings.provider;
    let request_key = build_review_brief_request_key(&detail, provider);
    let provider_statuses = provider_statuses_result.clone().unwrap_or_default();

    model
        .update(cx, |state, cx| {
            state.code_tour_settings.loading = false;
            if let Ok(settings) = &settings_result {
                state.code_tour_settings.settings = settings.clone();
                state.code_tour_settings.loaded = true;
                state.code_tour_settings.error = None;
            } else if let Err(error) = &settings_result {
                state.code_tour_settings.error = Some(error.clone());
            }

            state.code_tour_provider_loading = false;
            state.code_tour_provider_statuses_loaded = true;
            if let Ok(statuses) = &provider_statuses_result {
                state.code_tour_provider_statuses = statuses.clone();
                state.code_tour_provider_error = None;
            } else if let Err(error) = &provider_statuses_result {
                state.code_tour_provider_error = Some(error.clone());
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let request_changed =
                    detail_state.review_brief_state.request_key.as_deref() != Some(&request_key);
                detail_state.review_brief_state.request_key = Some(request_key.clone());
                detail_state.review_brief_state.loading = true;
                detail_state.review_brief_state.generating = false;
                detail_state.review_brief_state.progress_text =
                    Some("Checking cached review brief.".to_string());
                detail_state.review_brief_state.error = None;
                detail_state.review_brief_state.message = None;
                detail_state.review_brief_state.success = false;
                if request_changed {
                    detail_state.review_brief_state.document = None;
                }
            }

            cx.notify();
        })
        .ok();

    let cached_brief_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let detail = detail.clone();
            async move { review_brief::load_review_brief(&cache, &detail, provider) }
        })
        .await;

    let provider_ready = provider_statuses
        .iter()
        .find(|status| status.provider == provider)
        .map(|status| status.available && status.authenticated)
        .unwrap_or(false);
    let automatic_generation_enabled = settings.automatically_generates_for(&detail.repository);
    let missing_cached_brief = cached_brief_result
        .as_ref()
        .ok()
        .map(|brief| brief.is_none())
        .unwrap_or(false);
    let cached_brief_error = cached_brief_result.as_ref().err().cloned();
    let should_auto_generate = allow_automatic_generation
        && automatic_generation_enabled
        && provider_ready
        && missing_cached_brief
        && cached_brief_error.is_none()
        && model
            .read_with(cx, |state, _| {
                !state.automatic_brief_request_keys.contains(&request_key)
                    && detail_brief_request_matches(state, &detail_key, provider, &request_key)
            })
            .ok()
            .unwrap_or(false);

    model
        .update(cx, |state, cx| {
            if !detail_brief_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.review_brief_state.loading = should_auto_generate
                    && cached_brief_result
                        .as_ref()
                        .ok()
                        .and_then(|document| document.as_ref())
                        .is_none();
                detail_state.review_brief_state.generating = false;
                detail_state.review_brief_state.progress_text =
                    if detail_state.review_brief_state.loading {
                        Some("Preparing review brief.".to_string())
                    } else {
                        None
                    };
                match &cached_brief_result {
                    Ok(document) => {
                        detail_state.review_brief_state.document = document.clone();
                        detail_state.review_brief_state.error = None;
                    }
                    Err(error) => {
                        detail_state.review_brief_state.document = None;
                        detail_state.review_brief_state.error = Some(error.clone());
                    }
                }
            }

            cx.notify();
        })
        .ok();

    if should_auto_generate {
        model
            .update(cx, |state, _| {
                state.automatic_brief_request_keys.insert(request_key);
            })
            .ok();
        run_review_intelligence_flow(model, ReviewIntelligenceScope::BriefOnly, false, true, cx)
            .await;
    }
}

pub(crate) async fn run_review_intelligence_flow(
    model: Entity<AppState>,
    scope: ReviewIntelligenceScope,
    force: bool,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) {
    let Some(initial) = model
        .read_with(cx, |state, _| {
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let provider = state.selected_tour_provider();
            let open_pull_requests = state
                .active_detail_state()
                .and_then(|detail_state| detail_state.stack_open_pull_requests.clone())
                .unwrap_or_default();
            let existing_local_repository_status = state
                .active_detail_state()
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            Some((
                state.cache.clone(),
                detail_key,
                detail,
                provider,
                state.code_tour_provider_statuses_loaded,
                open_pull_requests,
                existing_local_repository_status,
            ))
        })
        .ok()
        .flatten()
    else {
        return;
    };

    let (
        cache,
        detail_key,
        detail,
        provider,
        statuses_loaded,
        open_pull_requests,
        existing_local_repository_status,
    ) = initial;
    let request_key = review_intelligence_request_key(&detail, provider);
    let code_version_key = tour_code_version_key(&detail);
    let brief_request_key = build_review_brief_request_key(&detail, provider);
    let tour_request_key = build_tour_request_key(&detail, provider);
    let local_review_repository_status =
        local_review::reusable_local_repository_status(&detail, existing_local_repository_status);
    let local_repository_already_ready = matches!(&local_review_repository_status, Ok(Some(_)));

    let should_start = model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return false;
            };
            if detail_state.review_intelligence_loading
                && detail_state.review_intelligence_request_key.as_deref() == Some(&request_key)
            {
                if force {
                    if scope.includes_brief() {
                        let brief_state = &mut detail_state.review_brief_state;
                        brief_state.request_key = Some(brief_request_key.clone());
                        brief_state.loading = false;
                        brief_state.generating = true;
                        brief_state.progress_text =
                            Some("Generation is already in progress.".to_string());
                        brief_state.error = None;
                        brief_state.message = Some("Generation is already in progress.".to_string());
                        brief_state.success = false;
                    }

                    if scope.includes_stack() {
                        detail_state.ai_stack_state.loading = false;
                        detail_state.ai_stack_state.generating = true;
                        detail_state.ai_stack_state.error = None;
                        detail_state.ai_stack_state.message =
                            Some("Generation is already in progress.".to_string());
                        detail_state.ai_stack_state.success = false;
                    }

                    if scope.includes_tour() {
                        set_tour_pipeline_progress(
                            detail_state,
                            provider,
                            &tour_request_key,
                            false,
                            true,
                            "Generation already in progress",
                            &format!(
                                "{} is already preparing intelligence for this pull request.",
                                provider.label()
                            ),
                        );
                    }

                    cx.notify();
                }
                return false;
            }

            detail_state.review_intelligence_request_key = Some(request_key.clone());
            detail_state.review_intelligence_loading = true;
            detail_state.local_repository_loading = !local_repository_already_ready;
            detail_state.local_repository_error = None;
            if let Ok(Some(status)) = local_review_repository_status.as_ref() {
                detail_state.local_repository_status = Some(status.clone());
            }

            if !statuses_loaded {
                state.code_tour_provider_loading = true;
                state.code_tour_provider_error = None;
            }

            if scope.includes_stack() {
                let stack_request_changed =
                    detail_state.ai_stack_state.request_key.as_deref() != Some(&request_key);
                detail_state.ai_stack_state.request_key = Some(request_key.clone());
                detail_state.ai_stack_state.loading = true;
                detail_state.ai_stack_state.generating = false;
                if force || stack_request_changed {
                    detail_state.ai_stack_state.stack = None;
                }
                detail_state.ai_stack_state.error = None;
                detail_state.ai_stack_state.message =
                    Some("Preparing local checkout for AI stack review.".to_string());
                detail_state.ai_stack_state.success = false;
            }

            if scope.includes_brief() {
                let brief_state = &mut detail_state.review_brief_state;
                let brief_request_changed =
                    brief_state.request_key.as_deref() != Some(&brief_request_key);
                brief_state.request_key = Some(brief_request_key.clone());
                if force || brief_request_changed {
                    brief_state.document = None;
                }
                brief_state.loading = !force;
                brief_state.generating = force;
                brief_state.progress_text = Some(
                    "Preparing local checkout for the review brief.".to_string(),
                );
                brief_state.error = None;
                brief_state.message = None;
                brief_state.success = false;
            }

            if scope.includes_tour() {
                let tour_state = detail_state.tour_states.entry(provider).or_default();
                let tour_request_changed =
                    tour_state.request_key.as_deref() != Some(&tour_request_key);
                tour_state.request_key = Some(tour_request_key.clone());
                if force || tour_request_changed {
                    tour_state.document = None;
                }
                tour_state.loading = !force;
                tour_state.generating = force;
                tour_state.progress_summary = Some(if scope.includes_stack() {
                    "Preparing AI tour and stack".to_string()
                } else {
                    "Preparing AI tour".to_string()
                });
                tour_state.progress_detail = Some(
                    "Preparing the local checkout and checking cached intelligence for this pull request."
                        .to_string(),
                );
                tour_state.progress_log.clear();
                tour_state.progress_log_file_path = None;
                tour_state.error = None;
                tour_state.message = None;
                tour_state.success = false;
            }

            cx.notify();
            true
        })
        .ok()
        .unwrap_or(false);

    if !should_start {
        return;
    }

    let local_review_repository_status = match local_review_repository_status {
        Ok(status) => status,
        Err(error) => {
            fail_checkout(
                &model,
                &detail_key,
                scope,
                provider,
                &request_key,
                &error,
                cx,
            )
            .await;
            finish_request(&model, &detail_key, &request_key, cx).await;
            return;
        }
    };

    let _permit = ForegroundJobPermit::new();

    if !statuses_loaded {
        let statuses_result = cx
            .background_executor()
            .spawn(async { code_tour::load_code_tour_provider_statuses() })
            .await;
        model
            .update(cx, |state, cx| {
                state.code_tour_provider_loading = false;
                state.code_tour_provider_statuses_loaded = true;
                match statuses_result {
                    Ok(statuses) => {
                        state.code_tour_provider_statuses = statuses;
                        state.code_tour_provider_error = None;
                    }
                    Err(error) => {
                        state.code_tour_provider_error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
    }

    let local_repo_result = if let Some(status) = local_review_repository_status {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    run_foreground_blocking(|| {
                        local_repo::ensure_local_repository_for_pull_request(
                            &cache,
                            &repository,
                            pull_request_number,
                            head_ref_oid.as_deref(),
                        )
                    })
                }
            })
            .await
    };

    let local_repo_status = match local_repo_result {
        Ok(status) => {
            model
                .update(cx, |state, cx| {
                    if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                        detail_state.local_repository_loading = false;
                        detail_state.local_repository_status = Some(status.clone());
                        detail_state.local_repository_error = None;
                    }
                    cx.notify();
                })
                .ok();
            status
        }
        Err(error) => {
            fail_checkout(
                &model,
                &detail_key,
                scope,
                provider,
                &request_key,
                &error,
                cx,
            )
            .await;
            finish_request(&model, &detail_key, &request_key, cx).await;
            return;
        }
    };

    if scope.includes_stack() {
        generate_or_load_stack(
            &model,
            cache.as_ref(),
            &detail_key,
            &detail,
            provider,
            &request_key,
            &code_version_key,
            &local_repo_status,
            open_pull_requests,
            force,
            scope.includes_tour(),
            &tour_request_key,
            cx,
        )
        .await;
    }

    if scope.includes_brief() {
        generate_or_load_brief(
            &model,
            cache.as_ref(),
            &detail_key,
            &detail,
            provider,
            &brief_request_key,
            &local_repo_status,
            force,
            automatic,
            cx,
        )
        .await;
    }

    if scope.includes_tour() {
        generate_or_load_tour(
            &model,
            cache.as_ref(),
            &detail_key,
            detail.clone(),
            provider,
            tour_request_key,
            &local_repo_status,
            force,
            automatic,
            cx,
        )
        .await;
    }

    finish_request(&model, &detail_key, &request_key, cx).await;
}

async fn generate_or_load_stack(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
    request_key: &str,
    code_version_key: &str,
    local_repo_status: &local_repo::LocalRepositoryStatus,
    open_pull_requests: Vec<crate::stacks::model::StackPullRequestRef>,
    force: bool,
    reflect_tour_progress: bool,
    tour_request_key: &str,
    cx: &mut AsyncWindowContext,
) {
    if !force {
        let cached = cx
            .background_executor()
            .spawn({
                let cache = CacheStore::clone(cache);
                let repository = detail.repository.clone();
                let pr_number = detail.number;
                let code_version_key = code_version_key.to_string();
                async move {
                    load_ai_review_stack(
                        &cache,
                        &repository,
                        pr_number,
                        provider,
                        &code_version_key,
                    )
                }
            })
            .await;

        if let Ok(Some(stack)) = cached {
            set_stack_success(
                model,
                detail_key,
                request_key,
                stack,
                Some("Loaded cached AI stack review.".to_string()),
                cx,
            )
            .await;
            if reflect_tour_progress {
                model
                    .update(cx, |state, cx| {
                        if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                            set_tour_pipeline_progress(
                                detail_state,
                                provider,
                                tour_request_key,
                                true,
                                false,
                                "Loaded cached AI stack review",
                                "Starting the AI tour generation step.",
                            );
                        }
                        cx.notify();
                    })
                    .ok();
            }
            return;
        }
    }

    let Some(working_directory) = local_repo_status.path.as_ref() else {
        set_stack_error(
            model,
            detail_key,
            request_key,
            detail,
            local_repo_status.message.clone(),
            cx,
        )
        .await;
        return;
    };

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.ai_stack_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = true;
                    detail_state.ai_stack_state.message =
                        Some("Generating AI stack review.".to_string());
                }

                if reflect_tour_progress {
                    set_tour_pipeline_progress(
                        detail_state,
                        provider,
                        tour_request_key,
                        false,
                        true,
                        "Generating AI stack review",
                        &format!(
                            "{} is planning review layers first. The AI tour starts after that phase finishes.",
                            provider.label()
                        ),
                    );
                }
            }
            cx.notify();
        })
        .ok();

    let stack_result = cx
        .background_executor()
        .spawn({
            let detail = detail.clone();
            let working_directory = PathBuf::from(working_directory);
            async move {
                run_foreground_blocking(|| {
                    let options = StackDiscoveryOptions {
                        enable_github_native: false,
                        enable_branch_topology: false,
                        enable_local_metadata: false,
                        enable_ai_virtual: true,
                        enable_virtual_commits: false,
                        enable_virtual_semantic: false,
                        ai_provider: Some(provider),
                        ..StackDiscoveryOptions::default()
                    };

                    let repo_context = RepoContext {
                        open_pull_requests,
                        local_repo_path: Some(working_directory),
                        trunk_branch: None,
                    };

                    discover_review_stack(&detail, &repo_context, options)
                        .map_err(|error| error.message)
                })
            }
        })
        .await;

    match stack_result {
        Ok(stack) if !stack_is_ai_unavailable(&stack) => {
            let _ = save_ai_review_stack(cache, &stack, provider, code_version_key);
            set_stack_success(
                model,
                detail_key,
                request_key,
                stack,
                Some("Generated AI stack review.".to_string()),
                cx,
            )
            .await;
        }
        Ok(stack) => {
            let message = stack
                .warnings
                .first()
                .map(|warning| warning.message.clone())
                .unwrap_or_else(|| {
                    "AI stack planning was unavailable. Retry after checkout and provider issues are resolved."
                        .to_string()
                });
            set_stack_transient_failure(model, detail_key, request_key, stack, message, cx).await;
        }
        Err(error) => {
            set_stack_error(model, detail_key, request_key, detail, error, cx).await;
        }
    }
}

async fn generate_or_load_brief(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
    request_key: &str,
    local_repo_status: &local_repo::LocalRepositoryStatus,
    force: bool,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) {
    if !force {
        let cached = cx
            .background_executor()
            .spawn({
                let cache = CacheStore::clone(cache);
                let detail = detail.clone();
                async move { review_brief::load_review_brief(&cache, &detail, provider) }
            })
            .await;

        if let Ok(Some(brief)) = cached {
            set_brief_success(
                model,
                detail_key,
                request_key,
                brief,
                Some("Loaded cached review brief.".to_string()),
                cx,
            )
            .await;
            return;
        }
    }

    let provider_status = model
        .read_with(cx, |state, _| {
            state
                .code_tour_provider_statuses
                .iter()
                .find(|status| status.provider == provider)
                .cloned()
        })
        .ok()
        .flatten();

    match provider_status {
        Some(status) if !status.available || !status.authenticated => {
            set_brief_message(model, detail_key, request_key, status.message, cx).await;
            return;
        }
        None => {
            set_brief_message(
                model,
                detail_key,
                request_key,
                "Still checking provider status.".to_string(),
                cx,
            )
            .await;
            return;
        }
        _ => {}
    }

    let Some(working_directory) = local_repo_status.path.as_ref() else {
        set_brief_error(
            model,
            detail_key,
            request_key,
            local_repo_status.message.clone(),
            cx,
        )
        .await;
        return;
    };

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                set_review_brief_progress(
                    detail_state,
                    request_key,
                    false,
                    true,
                    &format!("{} is preparing the review brief.", provider.label()),
                );
            }
            cx.notify();
        })
        .ok();

    let generation_result = cx
        .background_executor()
        .spawn({
            let cache = CacheStore::clone(cache);
            let detail = detail.clone();
            let working_directory = working_directory.clone();
            async move {
                run_foreground_blocking(|| {
                    let input = review_brief::build_review_brief_generation_input(
                        &detail,
                        provider,
                        &working_directory,
                    );
                    review_brief::generate_review_brief(&cache, input)
                })
            }
        })
        .await;

    match generation_result {
        Ok(brief) => {
            set_brief_success(
                model,
                detail_key,
                request_key,
                brief,
                Some(if automatic {
                    format!(
                        "Cached a {} review brief in the background.",
                        provider.label()
                    )
                } else {
                    format!("Generated a {} review brief.", provider.label())
                }),
                cx,
            )
            .await;
        }
        Err(error) => {
            set_brief_error(model, detail_key, request_key, error, cx).await;
        }
    }
}

async fn generate_or_load_tour(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: PullRequestDetail,
    provider: CodeTourProvider,
    tour_request_key: String,
    local_repo_status: &local_repo::LocalRepositoryStatus,
    force: bool,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) {
    if !force {
        let cached = cx
            .background_executor()
            .spawn({
                let cache = CacheStore::clone(cache);
                let detail = detail.clone();
                async move { code_tour::load_code_tour(&cache, &detail, provider) }
            })
            .await;

        if let Ok(Some(tour)) = cached {
            model
                .update(cx, |state, cx| {
                    if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                        let tour_state = detail_state.tour_states.entry(provider).or_default();
                        if tour_state.request_key.as_deref() == Some(&tour_request_key) {
                            tour_state.loading = false;
                            tour_state.generating = false;
                            tour_state.document = Some(tour);
                            tour_state.error = None;
                            tour_state.message = Some("Loaded cached AI tour.".to_string());
                            tour_state.success = true;
                        }
                    }
                    cx.notify();
                })
                .ok();
            return;
        }
    }

    crate::views::ai_tour::generate_tour_flow(
        model.clone(),
        Some((detail_key.to_string(), detail, provider, tour_request_key)),
        Some(local_repo_status.clone()),
        automatic,
        cx,
    )
    .await;
}

async fn fail_checkout(
    model: &Entity<AppState>,
    detail_key: &str,
    scope: ReviewIntelligenceScope,
    provider: CodeTourProvider,
    request_key: &str,
    error: &str,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_error = Some(error.to_string());

                if scope.includes_brief()
                    && detail_state.review_brief_state.request_key.as_deref() == Some(request_key)
                {
                    detail_state.review_brief_state.loading = false;
                    detail_state.review_brief_state.generating = false;
                    detail_state.review_brief_state.error = Some(error.to_string());
                    detail_state.review_brief_state.progress_text = None;
                    detail_state.review_brief_state.message = None;
                    detail_state.review_brief_state.success = false;
                }

                if scope.includes_stack()
                    && detail_state.ai_stack_state.request_key.as_deref() == Some(request_key)
                {
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = false;
                    detail_state.ai_stack_state.error = Some(error.to_string());
                    detail_state.ai_stack_state.message = None;
                    detail_state.ai_stack_state.success = false;
                }

                if scope.includes_tour() {
                    let tour_state = detail_state.tour_states.entry(provider).or_default();
                    tour_state.loading = false;
                    tour_state.generating = false;
                    tour_state.error = Some(error.to_string());
                    tour_state.message = None;
                    tour_state.success = false;
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_brief_success(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    brief: review_brief::ReviewBrief,
    message: Option<String>,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.review_brief_state.request_key.as_deref() == Some(request_key) {
                    detail_state.review_brief_state.document = Some(brief);
                    detail_state.review_brief_state.loading = false;
                    detail_state.review_brief_state.generating = false;
                    detail_state.review_brief_state.progress_text = None;
                    detail_state.review_brief_state.error = None;
                    detail_state.review_brief_state.message = message;
                    detail_state.review_brief_state.success = true;
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_brief_error(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                set_review_brief_error(detail_state, request_key, error.clone());
            }
            cx.notify();
        })
        .ok();
}

async fn set_brief_message(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    message: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.review_brief_state.request_key.as_deref() == Some(request_key) {
                    detail_state.review_brief_state.loading = false;
                    detail_state.review_brief_state.generating = false;
                    detail_state.review_brief_state.progress_text = None;
                    detail_state.review_brief_state.error = None;
                    detail_state.review_brief_state.message = Some(message.clone());
                    detail_state.review_brief_state.success = false;
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_stack_success(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    stack: ReviewStack,
    message: Option<String>,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.ai_stack_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.stack = Some(std::sync::Arc::new(stack));
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = false;
                    detail_state.ai_stack_state.error = None;
                    detail_state.ai_stack_state.message = message;
                    detail_state.ai_stack_state.success = true;
                    state.review_stack_cache.borrow_mut().clear();
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_stack_transient_failure(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    stack: ReviewStack,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.ai_stack_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.stack = Some(std::sync::Arc::new(stack));
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = false;
                    detail_state.ai_stack_state.error = Some(error);
                    detail_state.ai_stack_state.message = None;
                    detail_state.ai_stack_state.success = false;
                    state.review_stack_cache.borrow_mut().clear();
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_stack_error(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    detail: &PullRequestDetail,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    let stack = ai_stack_for_error(detail, &error);
    set_stack_transient_failure(model, detail_key, request_key, stack, error, cx).await;
}

async fn finish_request(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.review_intelligence_request_key.as_deref() == Some(request_key) {
                    detail_state.review_intelligence_loading = false;
                    detail_state.review_intelligence_request_key = None;
                }
            }
            cx.notify();
        })
        .ok();
}

fn set_tour_pipeline_progress(
    detail_state: &mut DetailState,
    provider: CodeTourProvider,
    tour_request_key: &str,
    loading: bool,
    generating: bool,
    summary: &str,
    detail: &str,
) {
    let tour_state = detail_state.tour_states.entry(provider).or_default();
    if tour_state
        .request_key
        .as_deref()
        .is_some_and(|current| current != tour_request_key)
    {
        return;
    }

    tour_state.request_key = Some(tour_request_key.to_string());
    tour_state.loading = loading;
    tour_state.generating = generating;
    tour_state.progress_summary = Some(summary.to_string());
    tour_state.progress_detail = Some(detail.to_string());
    tour_state.error = None;
    tour_state.message = None;
    tour_state.success = false;
}

fn set_review_brief_progress(
    detail_state: &mut DetailState,
    request_key: &str,
    loading: bool,
    generating: bool,
    progress_text: &str,
) {
    let brief_state = &mut detail_state.review_brief_state;
    if brief_state
        .request_key
        .as_deref()
        .is_some_and(|current| current != request_key)
    {
        return;
    }

    brief_state.request_key = Some(request_key.to_string());
    brief_state.loading = loading;
    brief_state.generating = generating;
    brief_state.progress_text = Some(progress_text.to_string());
    brief_state.error = None;
    brief_state.message = None;
    brief_state.success = false;
}

fn set_review_brief_error(detail_state: &mut DetailState, request_key: &str, error: String) {
    if detail_state.review_brief_state.request_key.as_deref() != Some(request_key) {
        return;
    }

    detail_state.review_brief_state.loading = false;
    detail_state.review_brief_state.generating = false;
    detail_state.review_brief_state.progress_text = None;
    detail_state.review_brief_state.error = Some(error);
    detail_state.review_brief_state.message = None;
    detail_state.review_brief_state.success = false;
}

pub fn review_intelligence_request_key(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> String {
    format!(
        "{}:{}#{}:{}",
        provider.slug(),
        detail.repository,
        detail.number,
        tour_code_version_key(detail)
    )
}

fn detail_brief_request_matches(
    state: &AppState,
    detail_key: &str,
    provider: CodeTourProvider,
    request_key: &str,
) -> bool {
    state
        .detail_states
        .get(detail_key)
        .and_then(|detail_state| detail_state.snapshot.as_ref())
        .and_then(|snapshot| snapshot.detail.as_ref())
        .map(|detail| build_review_brief_request_key(detail, provider) == request_key)
        .unwrap_or(false)
}

fn stack_is_ai_unavailable(stack: &ReviewStack) -> bool {
    stack
        .warnings
        .iter()
        .any(|warning| warning.code == "ai-virtual-stack-unavailable")
}

fn ai_stack_for_error(detail: &PullRequestDetail, message: &str) -> ReviewStack {
    crate::stacks::providers::ai_virtual::ai_unavailable_stack(
        detail,
        &format!("AI stack planning failed. {message}"),
        Some(serde_json::json!({ "error": message })),
    )
    .unwrap_or_else(|_| ReviewStack {
        id: format!("stack-error:{}#{}", detail.repository, detail.number),
        repository: detail.repository.clone(),
        selected_pr_number: detail.number,
        source: crate::stacks::model::StackSource::VirtualAi,
        kind: crate::stacks::model::StackKind::Virtual,
        confidence: Confidence::Low,
        trunk_branch: Some(detail.base_ref_name.clone()),
        base_oid: detail.base_ref_oid.clone(),
        head_oid: detail.head_ref_oid.clone(),
        layers: Vec::new(),
        atoms: Vec::new(),
        warnings: vec![crate::stacks::model::StackWarning::new(
            "ai-virtual-stack-unavailable",
            "AI stack planning failed and Remiss did not generate a non-AI stack.",
        )],
        provider: None,
        generated_at_ms: crate::stacks::model::stack_now_ms(),
        generator_version: crate::stacks::model::STACK_GENERATOR_VERSION.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        review_intelligence_request_key, set_review_brief_error, set_review_brief_progress,
        set_tour_pipeline_progress,
    };
    use crate::{code_tour::CodeTourProvider, github::PullRequestDetail, state::DetailState};

    #[test]
    fn review_intelligence_request_key_ignores_metadata_updates_when_head_matches() {
        let first = detail("2026-04-17T10:00:00Z", Some("head123"), "diff-one");
        let second = detail("2026-04-17T11:00:00Z", Some("head123"), "diff-two");

        assert_eq!(
            review_intelligence_request_key(&first, CodeTourProvider::Codex),
            review_intelligence_request_key(&second, CodeTourProvider::Codex)
        );
    }

    #[test]
    fn review_intelligence_request_key_varies_by_provider() {
        let detail = detail("2026-04-17T10:00:00Z", Some("head123"), "diff-one");

        assert_ne!(
            review_intelligence_request_key(&detail, CodeTourProvider::Codex),
            review_intelligence_request_key(&detail, CodeTourProvider::Copilot)
        );
    }

    #[test]
    fn tour_pipeline_progress_marks_visible_generation_state() {
        let mut detail_state = DetailState::default();

        set_tour_pipeline_progress(
            &mut detail_state,
            CodeTourProvider::Copilot,
            "tour-key",
            false,
            true,
            "Generating AI stack review",
            "Copilot is planning review layers first.",
        );

        let tour_state = detail_state
            .tour_states
            .get(&CodeTourProvider::Copilot)
            .expect("tour state should be created");
        assert_eq!(tour_state.request_key.as_deref(), Some("tour-key"));
        assert!(!tour_state.loading);
        assert!(tour_state.generating);
        assert_eq!(
            tour_state.progress_summary.as_deref(),
            Some("Generating AI stack review")
        );
        assert_eq!(
            tour_state.progress_detail.as_deref(),
            Some("Copilot is planning review layers first.")
        );
    }

    #[test]
    fn tour_pipeline_progress_ignores_stale_tour_request() {
        let mut detail_state = DetailState::default();
        detail_state
            .tour_states
            .entry(CodeTourProvider::Copilot)
            .or_default()
            .request_key = Some("newer-tour-key".to_string());

        set_tour_pipeline_progress(
            &mut detail_state,
            CodeTourProvider::Copilot,
            "older-tour-key",
            false,
            true,
            "Generating AI stack review",
            "Copilot is planning review layers first.",
        );

        let tour_state = detail_state
            .tour_states
            .get(&CodeTourProvider::Copilot)
            .expect("tour state should exist");
        assert_eq!(tour_state.request_key.as_deref(), Some("newer-tour-key"));
        assert!(!tour_state.generating);
        assert!(tour_state.progress_summary.is_none());
    }

    #[test]
    fn review_brief_progress_ignores_stale_request() {
        let mut detail_state = DetailState::default();
        detail_state.review_brief_state.request_key = Some("newer-brief-key".to_string());

        set_review_brief_progress(
            &mut detail_state,
            "older-brief-key",
            false,
            true,
            "Generating review brief.",
        );

        assert_eq!(
            detail_state.review_brief_state.request_key.as_deref(),
            Some("newer-brief-key")
        );
        assert!(!detail_state.review_brief_state.generating);
        assert!(detail_state.review_brief_state.progress_text.is_none());
    }

    #[test]
    fn review_brief_failure_then_retry_clears_error_and_marks_generating() {
        let mut detail_state = DetailState::default();
        detail_state.review_brief_state.request_key = Some("brief-key".to_string());

        set_review_brief_error(
            &mut detail_state,
            "brief-key",
            "Provider returned invalid JSON.".to_string(),
        );

        assert_eq!(
            detail_state.review_brief_state.error.as_deref(),
            Some("Provider returned invalid JSON.")
        );
        assert!(!detail_state.review_brief_state.generating);

        set_review_brief_progress(
            &mut detail_state,
            "brief-key",
            false,
            true,
            "Regenerating review brief.",
        );

        assert!(detail_state.review_brief_state.error.is_none());
        assert!(detail_state.review_brief_state.generating);
        assert_eq!(
            detail_state.review_brief_state.progress_text.as_deref(),
            Some("Regenerating review brief.")
        );
    }

    fn detail(updated_at: &str, head_ref_oid: Option<&str>, raw_diff: &str) -> PullRequestDetail {
        PullRequestDetail {
            id: "pr1".to_string(),
            repository: "acme/api".to_string(),
            number: 42,
            title: "Test PR".to_string(),
            body: String::new(),
            url: "https://example.com/pr/42".to_string(),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature/test".to_string(),
            base_ref_oid: Some("base123".to_string()),
            head_ref_oid: head_ref_oid.map(str::to_string),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-04-17T00:00:00Z".to_string(),
            updated_at: updated_at.to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: std::collections::BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            viewer_pending_review: None,
            files: Vec::new(),
            raw_diff: raw_diff.to_string(),
            parsed_diff: Vec::new(),
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        }
    }
}
