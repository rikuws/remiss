use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use gpui::{App, AsyncWindowContext, Entity, Window};
use once_cell::sync::Lazy;
use serde_json::json;

use crate::{
    cache::CacheStore,
    github::PullRequestDetail,
    local_repo, local_review,
    review_ai::{self, review_code_version_key, ReviewAiProgressUpdate, ReviewAiProvider},
    review_brief::{self, build_review_brief_request_key},
    review_memory,
    review_partner::{self, build_review_partner_request_key},
    semantic_review, sentry_diagnostics,
    stacks::{
        atoms::extract_change_atoms,
        cache::{load_ai_review_stack, save_ai_review_stack},
        discovery::discover_review_stack,
        model::{Confidence, RepoContext, ReviewStack, StackDiscoveryOptions, StackPullRequestRef},
        title_polish,
    },
    state::{AppState, DetailState},
    structural_diff::checkout_head_oid,
    structural_evidence,
};

static REVIEW_INTELLIGENCE_JOB_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static FOREGROUND_REVIEW_INTELLIGENCE_JOBS: AtomicUsize = AtomicUsize::new(0);

struct GeneratedReviewStack {
    stack: ReviewStack,
    semantic_review: Option<semantic_review::RemissSemanticReview>,
}

struct ReviewIntelligenceFlowInitial {
    cache: Arc<CacheStore>,
    detail_key: String,
    detail: PullRequestDetail,
    provider: ReviewAiProvider,
    lsp_session_manager: Arc<crate::lsp::LspSessionManager>,
    statuses_loaded: bool,
    open_pull_requests: Vec<StackPullRequestRef>,
    existing_local_repository_status: Option<local_repo::LocalRepositoryStatus>,
}

struct ReviewIntelligenceRequestPlan {
    request_key: String,
    stack_code_version_key: String,
    brief_request_key: String,
    partner_request_key: String,
    local_repository_status: Result<Option<local_repo::LocalRepositoryStatus>, String>,
    local_repository_already_ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewIntelligenceScope {
    All,
    BriefOnly,
    StackOnly,
}

impl ReviewIntelligenceScope {
    fn includes_brief(self) -> bool {
        matches!(self, Self::All | Self::BriefOnly)
    }

    fn includes_stack(self) -> bool {
        matches!(self, Self::All | Self::StackOnly)
    }

    fn includes_partner(self) -> bool {
        self.includes_stack()
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
    if !state.read(cx).review_ai_features_enabled() {
        return;
    }

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
    if !state.read(cx).review_ai_features_enabled() {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            refresh_active_review_brief_flow(model, allow_automatic_generation, cx).await;
        })
        .detach();
}

pub fn refresh_active_review_partner(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    allow_automatic_generation: bool,
) {
    if !state.read(cx).review_ai_features_enabled() {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            refresh_active_review_partner_flow(model, allow_automatic_generation, cx).await;
        })
        .detach();
}

pub fn request_active_review_partner_focus(
    model: &Entity<AppState>,
    target: review_partner::ReviewPartnerFocusTarget,
    cx: &mut App,
) {
    let request = {
        let state = model.read(cx);
        if !state.review_ai_features_enabled() {
            return;
        }
        let Some(detail_key) = state.active_pr_key.clone() else {
            return;
        };
        let Some(detail_state) = state.detail_states.get(&detail_key) else {
            return;
        };
        let Some(document) = detail_state.review_partner_state.document.clone() else {
            return;
        };
        if document.focus_record(&target.key).is_some()
            || detail_state
                .review_partner_state
                .loading_focus_keys
                .contains(&target.key)
        {
            return;
        }
        let Some(local_repo_status) = detail_state.local_repository_status.clone() else {
            return;
        };
        let Some(working_directory) = local_repo_status.path.clone() else {
            return;
        };
        let Some(request_key) = detail_state.review_partner_state.request_key.clone() else {
            return;
        };
        (
            detail_key,
            request_key,
            document,
            working_directory,
            CacheStore::clone(state.cache.as_ref()),
        )
    };

    let (detail_key, request_key, document, working_directory, cache) = request;
    let focus_key = target.key.clone();
    model.update(cx, |state, cx| {
        if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
            if detail_state.review_partner_state.request_key.as_deref() == Some(&request_key) {
                detail_state
                    .review_partner_state
                    .loading_focus_keys
                    .insert(focus_key.clone());
                detail_state
                    .review_partner_state
                    .focus_errors
                    .remove(&focus_key);
                cx.notify();
            }
        }
    });

    let model = model.clone();
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn({
                let document = document.clone();
                let target = target.clone();
                let working_directory = working_directory.clone();
                async move {
                    run_foreground_blocking(|| {
                        review_partner::generate_review_partner_focus_record(
                            document.as_ref(),
                            target,
                            &working_directory,
                        )
                    })
                }
            })
            .await;

        let mut document_to_save = None;
        model
            .update(cx, |state, cx| {
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    if detail_state.review_partner_state.request_key.as_deref()
                        != Some(&request_key)
                    {
                        return;
                    }

                    detail_state
                        .review_partner_state
                        .loading_focus_keys
                        .remove(&focus_key);
                    match result {
                        Ok(record) => {
                            if let Some(current) =
                                detail_state.review_partner_state.document.as_ref()
                            {
                                let mut next = current.as_ref().clone();
                                review_partner::upsert_focus_record(
                                    &mut next,
                                    target.clone(),
                                    record,
                                );
                                document_to_save = Some(next.clone());
                                detail_state.review_partner_state.document =
                                    Some(std::sync::Arc::new(next));
                            }
                            detail_state
                                .review_partner_state
                                .focus_errors
                                .remove(&focus_key);
                        }
                        Err(error) => {
                            if let Some(current) =
                                detail_state.review_partner_state.document.as_ref()
                            {
                                let error_for_sentry = error.clone();
                                sentry_diagnostics::capture_ai_failure(
                                    "review_partner_focus",
                                    Some(current.provider.slug()),
                                    &error_for_sentry,
                                    |scope| {
                                        scope.set_tag("ai.phase", "focus_generation");
                                        scope.set_tag("ai.fallback", true);
                                        scope.set_tag("pr.repository", &current.stack.repository);
                                        scope.set_extra(
                                            "repository",
                                            json!(&current.stack.repository),
                                        );
                                        scope.set_extra(
                                            "pullRequestNumber",
                                            json!(current.stack.selected_pr_number),
                                        );
                                        scope.set_extra("requestKey", json!(&request_key));
                                        scope.set_extra(
                                            "codeVersionKey",
                                            json!(&current.code_version_key),
                                        );
                                        scope.set_extra("focusKey", json!(&focus_key));
                                        scope.set_extra("focusTitle", json!(&target.title));
                                        scope.set_extra("focusSubtitle", json!(&target.subtitle));
                                        scope.set_extra(
                                            "workingDirectory",
                                            json!(&working_directory),
                                        );
                                    },
                                );
                                let input = review_partner::GenerateReviewPartnerInput {
                                    provider: current.provider,
                                    working_directory: working_directory.clone(),
                                    repository: current.stack.repository.clone(),
                                    number: current.stack.selected_pr_number,
                                    code_version_key: current.code_version_key.clone(),
                                    title: String::new(),
                                    body: String::new(),
                                    url: String::new(),
                                    base_ref_name: String::new(),
                                    head_ref_name: String::new(),
                                    comments: Vec::new(),
                                    latest_reviews: Vec::new(),
                                    review_threads: Vec::new(),
                                    stack: current.stack.clone(),
                                    structural_evidence: current.structural_evidence.clone(),
                                    semantic_review: current.semantic_review.clone(),
                                    review_memory: current.review_memory.clone(),
                                    context: current.context.clone(),
                                    focus_targets: vec![target.clone()],
                                };
                                let record = review_partner::fallback_focus_record(
                                    &input,
                                    &target,
                                    Some(format!("AI focus context unavailable: {error}")),
                                );
                                let mut next = current.as_ref().clone();
                                review_partner::upsert_focus_record(
                                    &mut next,
                                    target.clone(),
                                    record,
                                );
                                // Keep timeout/error fallbacks session-local. Persisting them makes a
                                // temporary provider failure look like a stable generated explanation
                                // the next time this review is opened.
                                detail_state.review_partner_state.document =
                                    Some(std::sync::Arc::new(next));
                            }
                            detail_state
                                .review_partner_state
                                .focus_errors
                                .insert(focus_key.clone(), error);
                        }
                    }
                    cx.notify();
                }
            })
            .ok();

        if let Some(document) = document_to_save {
            let _ = review_partner::save_review_partner_context(&cache, &document);
        }
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
            if !state.review_ai_features_enabled() {
                return None;
            }
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            Some((
                state.cache.clone(),
                detail_key,
                detail,
                state.review_ai_settings.loaded,
                state.review_ai_settings.settings.clone(),
                state.review_ai_provider_statuses_loaded,
                state.review_ai_provider_statuses.clone(),
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
                state.review_ai_settings.loading = true;
                state.review_ai_settings.error = None;
                cx.notify();
            })
            .ok();
    }

    if !statuses_loaded {
        model
            .update(cx, |state, cx| {
                state.review_ai_provider_loading = true;
                state.review_ai_provider_error = None;
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
                async move { review_ai::load_review_ai_settings(&cache) }
            })
            .await
    };

    let provider_statuses_result = if statuses_loaded {
        Ok(existing_statuses)
    } else {
        cx.background_executor()
            .spawn(async { review_ai::load_review_ai_provider_statuses() })
            .await
    };

    let settings = settings_result
        .clone()
        .unwrap_or_else(|_| existing_settings.clone());
    if !settings.experimental_features_enabled() {
        model
            .update(cx, |state, cx| {
                state.review_ai_settings.loading = false;
                state.review_ai_provider_loading = false;
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    detail_state.review_brief_state.loading = false;
                    detail_state.review_brief_state.generating = false;
                    detail_state.review_brief_state.progress_text = None;
                }
                cx.notify();
            })
            .ok();
        return;
    }

    let provider = settings.provider;
    let request_key = build_review_brief_request_key(&detail, provider);
    let provider_statuses = provider_statuses_result.clone().unwrap_or_default();

    model
        .update(cx, |state, cx| {
            state.review_ai_settings.loading = false;
            if let Ok(settings) = &settings_result {
                state.review_ai_settings.settings = settings.clone();
                state.review_ai_settings.loaded = true;
                state.review_ai_settings.error = None;
            } else if let Err(error) = &settings_result {
                state.review_ai_settings.error = Some(error.clone());
            }

            state.review_ai_provider_loading = false;
            state.review_ai_provider_statuses_loaded = true;
            if let Ok(statuses) = &provider_statuses_result {
                state.review_ai_provider_statuses = statuses.clone();
                state.review_ai_provider_error = None;
            } else if let Err(error) = &provider_statuses_result {
                state.review_ai_provider_error = Some(error.clone());
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

pub(crate) async fn refresh_active_review_partner_flow(
    model: Entity<AppState>,
    allow_automatic_generation: bool,
    cx: &mut AsyncWindowContext,
) {
    let initial = model
        .read_with(cx, |state, _| {
            if !state.review_ai_features_enabled() {
                return None;
            }
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            Some((
                detail_key,
                detail,
                state.cache.clone(),
                state.review_ai_settings.loaded,
                state.review_ai_settings.settings.clone(),
                state.review_ai_provider_statuses_loaded,
                state.review_ai_provider_statuses.clone(),
            ))
        })
        .ok()
        .flatten();

    let Some((
        detail_key,
        detail,
        cache,
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
                state.review_ai_settings.loading = true;
                state.review_ai_settings.error = None;
                cx.notify();
            })
            .ok();
    }

    if !statuses_loaded {
        model
            .update(cx, |state, cx| {
                state.review_ai_provider_loading = true;
                state.review_ai_provider_error = None;
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
                async move { review_ai::load_review_ai_settings(&cache) }
            })
            .await
    };

    let provider_statuses_result = if statuses_loaded {
        Ok(existing_statuses)
    } else {
        cx.background_executor()
            .spawn(async { review_ai::load_review_ai_provider_statuses() })
            .await
    };

    let settings = settings_result
        .clone()
        .unwrap_or_else(|_| existing_settings.clone());
    if !settings.experimental_features_enabled() {
        model
            .update(cx, |state, cx| {
                state.review_ai_settings.loading = false;
                state.review_ai_provider_loading = false;
                if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                    detail_state.review_partner_state.loading = false;
                    detail_state.review_partner_state.generating = false;
                    detail_state.review_partner_state.progress_text = None;
                }
                cx.notify();
            })
            .ok();
        return;
    }

    let provider = settings.provider;
    let request_key = build_review_partner_request_key(&detail, provider);
    let provider_statuses = provider_statuses_result.clone().unwrap_or_default();
    let provider_ready = provider_statuses
        .iter()
        .find(|status| status.provider == provider)
        .map(|status| status.available && status.authenticated)
        .unwrap_or(false);
    let should_auto_generate = allow_automatic_generation
        && settings.automatically_generates_for(&detail.repository)
        && provider_ready
        && model
            .read_with(cx, |state, _| {
                !state.automatic_partner_request_keys.contains(&request_key)
                    && detail_partner_request_matches(state, &detail_key, provider, &request_key)
            })
            .ok()
            .unwrap_or(false);

    model
        .update(cx, |state, cx| {
            state.review_ai_settings.loading = false;
            if let Ok(settings) = &settings_result {
                state.review_ai_settings.settings = settings.clone();
                state.review_ai_settings.loaded = true;
                state.review_ai_settings.error = None;
            } else if let Err(error) = &settings_result {
                state.review_ai_settings.error = Some(error.clone());
            }

            state.review_ai_provider_loading = false;
            state.review_ai_provider_statuses_loaded = true;
            if let Ok(statuses) = &provider_statuses_result {
                state.review_ai_provider_statuses = statuses.clone();
                state.review_ai_provider_error = None;
            } else if let Err(error) = &provider_statuses_result {
                state.review_ai_provider_error = Some(error.clone());
            }

            cx.notify();
        })
        .ok();

    if should_auto_generate {
        model
            .update(cx, |state, _| {
                state.automatic_partner_request_keys.insert(request_key);
            })
            .ok();
        run_review_intelligence_flow(model, ReviewIntelligenceScope::StackOnly, false, true, cx)
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
    let Some(initial) = read_review_intelligence_initial(&model, automatic, cx) else {
        return;
    };

    let request = build_review_intelligence_request_plan(
        &initial.detail,
        initial.provider,
        initial.existing_local_repository_status,
    );
    if !start_review_intelligence_request(
        &model,
        &initial.detail_key,
        &request,
        scope,
        force,
        initial.statuses_loaded,
        cx,
    ) {
        return;
    }

    let local_review_repository_status = match request.local_repository_status {
        Ok(status) => status,
        Err(error) => {
            fail_checkout(
                &model,
                &initial.detail_key,
                scope,
                initial.provider,
                &request.request_key,
                &error,
                cx,
            )
            .await;
            finish_request(&model, &initial.detail_key, &request.request_key, cx).await;
            return;
        }
    };

    let _permit = ForegroundJobPermit::new();

    load_review_ai_provider_statuses_if_needed(&model, initial.statuses_loaded, cx).await;

    let local_repo_status = match ensure_review_intelligence_checkout(
        initial.cache.clone(),
        &initial.detail,
        local_review_repository_status,
        cx,
    )
    .await
    {
        Ok(status) => {
            set_review_intelligence_checkout_success(
                &model,
                &initial.detail_key,
                status.clone(),
                cx,
            );
            status
        }
        Err(error) => {
            fail_checkout(
                &model,
                &initial.detail_key,
                scope,
                initial.provider,
                &request.request_key,
                &error,
                cx,
            )
            .await;
            finish_request(&model, &initial.detail_key, &request.request_key, cx).await;
            return;
        }
    };

    let generated_stack = if scope.includes_stack() {
        generate_or_load_stack(
            &model,
            initial.cache.as_ref(),
            &initial.detail_key,
            &initial.detail,
            initial.provider,
            &request.request_key,
            &request.stack_code_version_key,
            &local_repo_status,
            initial.open_pull_requests,
            force,
            cx,
        )
        .await
    } else {
        None
    };

    if scope.includes_partner() {
        if let Some(generated_stack) = generated_stack {
            generate_or_load_partner(
                &model,
                initial.cache.as_ref(),
                &initial.detail_key,
                &initial.detail,
                initial.provider,
                &request.partner_request_key,
                &local_repo_status,
                generated_stack.stack,
                generated_stack.semantic_review,
                force,
                initial.lsp_session_manager.clone(),
                cx,
            )
            .await;
        } else {
            set_partner_error(
                &model,
                &initial.detail_key,
                &request.partner_request_key,
                "Guided Review needs generated stack layers before Review Partner context can be built."
                    .to_string(),
                cx,
            )
            .await;
        }
    }

    if scope.includes_brief() {
        generate_or_load_brief(
            &model,
            initial.cache.as_ref(),
            &initial.detail_key,
            &initial.detail,
            initial.provider,
            &request.brief_request_key,
            &local_repo_status,
            force,
            automatic,
            cx,
        )
        .await;
    }

    finish_request(&model, &initial.detail_key, &request.request_key, cx).await;
}

fn read_review_intelligence_initial(
    model: &Entity<AppState>,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) -> Option<ReviewIntelligenceFlowInitial> {
    model
        .read_with(cx, |state, _| {
            if !state.review_ai_features_enabled() {
                return None;
            }
            if automatic && !state.review_ai_background_jobs_enabled() {
                return None;
            }
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let provider = state.selected_review_ai_provider();
            let open_pull_requests = state
                .active_detail_state()
                .and_then(|detail_state| detail_state.stack_open_pull_requests.clone())
                .unwrap_or_default();
            let existing_local_repository_status = state
                .active_detail_state()
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            Some(ReviewIntelligenceFlowInitial {
                cache: state.cache.clone(),
                detail_key,
                detail,
                provider,
                lsp_session_manager: state.lsp_session_manager.clone(),
                statuses_loaded: state.review_ai_provider_statuses_loaded,
                open_pull_requests,
                existing_local_repository_status,
            })
        })
        .ok()
        .flatten()
}

fn build_review_intelligence_request_plan(
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
    existing_local_repository_status: Option<local_repo::LocalRepositoryStatus>,
) -> ReviewIntelligenceRequestPlan {
    let request_key = review_intelligence_request_key(detail, provider);
    let code_version_key = review_code_version_key(detail);
    let stack_code_version_key = format!(
        "{}:{}:{}:{}",
        code_version_key,
        crate::stacks::model::STACK_GENERATOR_VERSION,
        structural_evidence::STRUCTURAL_EVIDENCE_VERSION,
        semantic_review::semantic_review_version_key()
    );
    let brief_request_key = build_review_brief_request_key(detail, provider);
    let partner_request_key = build_review_partner_request_key(detail, provider);
    let local_repository_status =
        local_review::reusable_local_repository_status(detail, existing_local_repository_status);
    let local_repository_already_ready = matches!(&local_repository_status, Ok(Some(_)));
    ReviewIntelligenceRequestPlan {
        request_key,
        stack_code_version_key,
        brief_request_key,
        partner_request_key,
        local_repository_status,
        local_repository_already_ready,
    }
}

fn start_review_intelligence_request(
    model: &Entity<AppState>,
    detail_key: &str,
    request: &ReviewIntelligenceRequestPlan,
    scope: ReviewIntelligenceScope,
    force: bool,
    statuses_loaded: bool,
    cx: &mut AsyncWindowContext,
) -> bool {
    model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(detail_key) else {
                return false;
            };
            if detail_state.review_intelligence_loading
                && detail_state.review_intelligence_request_key.as_deref()
                    == Some(request.request_key.as_str())
            {
                if force {
                    mark_existing_review_intelligence_generation(detail_state, scope, request);
                    cx.notify();
                }
                return false;
            }

            initialize_review_intelligence_states(detail_state, scope, force, request);

            if !statuses_loaded {
                state.review_ai_provider_loading = true;
                state.review_ai_provider_error = None;
            }

            cx.notify();
            true
        })
        .ok()
        .unwrap_or(false)
}

fn mark_existing_review_intelligence_generation(
    detail_state: &mut DetailState,
    scope: ReviewIntelligenceScope,
    request: &ReviewIntelligenceRequestPlan,
) {
    if scope.includes_brief() {
        let brief_state = &mut detail_state.review_brief_state;
        brief_state.request_key = Some(request.brief_request_key.clone());
        brief_state.loading = false;
        brief_state.generating = true;
        brief_state.progress_text = Some("Generation is already in progress.".to_string());
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

    if scope.includes_partner() {
        detail_state.review_partner_state.request_key = Some(request.partner_request_key.clone());
        detail_state.review_partner_state.loading = false;
        detail_state.review_partner_state.generating = true;
        detail_state.review_partner_state.error = None;
        detail_state.review_partner_state.message =
            Some("Generation is already in progress.".to_string());
        detail_state.review_partner_state.progress_text =
            Some("Generation is already in progress.".to_string());
        detail_state.review_partner_state.success = false;
    }
}

fn initialize_review_intelligence_states(
    detail_state: &mut DetailState,
    scope: ReviewIntelligenceScope,
    force: bool,
    request: &ReviewIntelligenceRequestPlan,
) {
    detail_state.review_intelligence_request_key = Some(request.request_key.clone());
    detail_state.review_intelligence_loading = true;
    detail_state.local_repository_loading = !request.local_repository_already_ready;
    detail_state.local_repository_error = None;
    if let Ok(Some(status)) = request.local_repository_status.as_ref() {
        detail_state.local_repository_status = Some(status.clone());
    }

    if scope.includes_stack() {
        initialize_stack_and_partner_generation(detail_state, force, request);
    }

    if scope.includes_brief() {
        initialize_brief_generation(detail_state, force, request);
    }
}

fn initialize_stack_and_partner_generation(
    detail_state: &mut DetailState,
    force: bool,
    request: &ReviewIntelligenceRequestPlan,
) {
    let stack_request_changed =
        detail_state.ai_stack_state.request_key.as_deref() != Some(request.request_key.as_str());
    detail_state.ai_stack_state.request_key = Some(request.request_key.clone());
    detail_state.ai_stack_state.loading = true;
    detail_state.ai_stack_state.generating = false;
    if force || stack_request_changed {
        detail_state.ai_stack_state.stack = None;
    }
    detail_state.ai_stack_state.error = None;
    detail_state.ai_stack_state.message =
        Some("Preparing local checkout for Guided Review.".to_string());
    detail_state.ai_stack_state.success = false;

    let partner_request_changed = detail_state.review_partner_state.request_key.as_deref()
        != Some(request.partner_request_key.as_str());
    detail_state.review_partner_state.request_key = Some(request.partner_request_key.clone());
    detail_state.review_partner_state.loading = true;
    detail_state.review_partner_state.generating = false;
    if force || partner_request_changed {
        detail_state.review_partner_state.document = None;
    }
    detail_state.review_partner_state.error = None;
    detail_state.review_partner_state.message = None;
    detail_state.review_partner_state.progress_text =
        Some("Preparing local checkout for Review Partner.".to_string());
    detail_state.review_partner_state.success = false;
}

fn initialize_brief_generation(
    detail_state: &mut DetailState,
    force: bool,
    request: &ReviewIntelligenceRequestPlan,
) {
    let brief_state = &mut detail_state.review_brief_state;
    let brief_request_changed =
        brief_state.request_key.as_deref() != Some(request.brief_request_key.as_str());
    brief_state.request_key = Some(request.brief_request_key.clone());
    if force || brief_request_changed {
        brief_state.document = None;
    }
    brief_state.loading = !force;
    brief_state.generating = force;
    brief_state.progress_text = Some("Preparing local checkout for the review brief.".to_string());
    brief_state.error = None;
    brief_state.message = None;
    brief_state.success = false;
}

async fn load_review_ai_provider_statuses_if_needed(
    model: &Entity<AppState>,
    statuses_loaded: bool,
    cx: &mut AsyncWindowContext,
) {
    if statuses_loaded {
        return;
    }

    let statuses_result = cx
        .background_executor()
        .spawn(async { review_ai::load_review_ai_provider_statuses() })
        .await;
    model
        .update(cx, |state, cx| {
            state.review_ai_provider_loading = false;
            state.review_ai_provider_statuses_loaded = true;
            match statuses_result {
                Ok(statuses) => {
                    state.review_ai_provider_statuses = statuses;
                    state.review_ai_provider_error = None;
                }
                Err(error) => {
                    state.review_ai_provider_error = Some(error);
                }
            }
            cx.notify();
        })
        .ok();
}

async fn ensure_review_intelligence_checkout(
    cache: Arc<CacheStore>,
    detail: &PullRequestDetail,
    local_repository_status: Option<local_repo::LocalRepositoryStatus>,
    cx: &mut AsyncWindowContext,
) -> Result<local_repo::LocalRepositoryStatus, String> {
    if let Some(status) = local_repository_status {
        return Ok(status);
    }

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
}

fn set_review_intelligence_checkout_success(
    model: &Entity<AppState>,
    detail_key: &str,
    status: local_repo::LocalRepositoryStatus,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_status = Some(status.clone());
                detail_state.local_repository_error = None;
            }
            cx.notify();
        })
        .ok();
}

async fn generate_or_load_stack(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
    request_key: &str,
    code_version_key: &str,
    local_repo_status: &local_repo::LocalRepositoryStatus,
    open_pull_requests: Vec<crate::stacks::model::StackPullRequestRef>,
    force: bool,
    cx: &mut AsyncWindowContext,
) -> Option<GeneratedReviewStack> {
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
            let deterministic_stack = stack.clone();
            let semantic_review = if let Some(working_directory) = local_repo_status.path.as_ref() {
                cx.background_executor()
                    .spawn({
                        let cache = CacheStore::clone(cache);
                        let detail = detail.clone();
                        let semantic_stack = deterministic_stack.clone();
                        let working_directory = PathBuf::from(working_directory);
                        let head_oid = checkout_head_oid(local_repo_status);
                        async move {
                            run_foreground_blocking(|| {
                                semantic_review::build_and_cache_semantic_review(
                                    &cache,
                                    &detail,
                                    &semantic_stack.atoms,
                                    &detail.repository,
                                    working_directory.as_path(),
                                    head_oid.as_deref(),
                                    false,
                                )
                            })
                        }
                    })
                    .await
            } else {
                None
            };
            let display_stack = if let Some(working_directory) = local_repo_status.path.as_ref() {
                cx.background_executor()
                    .spawn({
                        let cache = CacheStore::clone(cache);
                        let detail = detail.clone();
                        let stack = deterministic_stack.clone();
                        let code_version_key = code_version_key.to_string();
                        let working_directory = PathBuf::from(working_directory);
                        async move {
                            run_foreground_blocking(|| {
                                title_polish::polish_stack_titles_best_effort(
                                    &cache,
                                    &detail,
                                    &stack,
                                    provider,
                                    &code_version_key,
                                    working_directory.as_path(),
                                    false,
                                )
                            })
                        }
                    })
                    .await
            } else {
                deterministic_stack.clone()
            };
            set_stack_success(
                model,
                detail_key,
                request_key,
                display_stack.clone(),
                Some("Loaded cached Guided Review stack.".to_string()),
                cx,
            )
            .await;
            return Some(GeneratedReviewStack {
                stack: display_stack,
                semantic_review,
            });
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
        return None;
    };

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.ai_stack_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = true;
                    detail_state.ai_stack_state.message =
                        Some("Building Guided Review stack.".to_string());
                }
            }
            cx.notify();
        })
        .ok();

    let (result_tx, result_rx) = mpsc::channel::<
        Result<
            (
                ReviewStack,
                ReviewStack,
                Option<semantic_review::RemissSemanticReview>,
            ),
            String,
        >,
    >();
    std::thread::spawn({
        let cache = CacheStore::clone(cache);
        let detail = detail.clone();
        let code_version_key = code_version_key.to_string();
        let working_directory = PathBuf::from(working_directory);
        let head_oid = checkout_head_oid(local_repo_status);
        move || {
            let result = run_foreground_blocking(|| {
                let atoms = extract_change_atoms(&detail);
                let semantic_review = semantic_review::build_and_cache_semantic_review(
                    &cache,
                    &detail,
                    &atoms,
                    &detail.repository,
                    working_directory.as_path(),
                    head_oid.as_deref(),
                    force,
                );
                let structural_evidence = head_oid
                    .as_deref()
                    .map(|head_oid| {
                        structural_evidence::build_structural_evidence_pack(
                            &cache,
                            &detail,
                            &atoms,
                            &detail.repository,
                            working_directory.as_path(),
                            head_oid,
                        )
                    })
                    .unwrap_or_else(|| {
                        let mut pack = structural_evidence::StructuralEvidencePack::empty();
                        pack.warnings.push(
                            "Structural evidence could not be built because checkout head was unavailable."
                                .to_string(),
                        );
                        pack
                    });
                let options = guided_review_stack_discovery_options();

                let repo_context = RepoContext {
                    open_pull_requests,
                    local_repo_path: Some(working_directory.clone()),
                    trunk_branch: None,
                    structural_evidence: Some(structural_evidence),
                    semantic_review: semantic_review.clone(),
                };

                let deterministic_stack = discover_review_stack(&detail, &repo_context, options)
                    .map_err(|error| error.message)?;
                let display_stack = title_polish::polish_stack_titles_best_effort(
                    &cache,
                    &detail,
                    &deterministic_stack,
                    provider,
                    &code_version_key,
                    working_directory.as_path(),
                    force,
                );
                Ok((deterministic_stack, display_stack, semantic_review))
            });
            let _ = result_tx.send(result);
        }
    });

    let stack_result = loop {
        match result_rx.try_recv() {
            Ok(result) => break result,
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err(
                    "Guided Review stack generation stopped before returning a result.".to_string(),
                );
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        cx.background_executor()
            .spawn(async move {
                std::thread::sleep(Duration::from_millis(120));
            })
            .await;
    };

    match stack_result {
        Ok((deterministic_stack, display_stack, semantic_review)) => {
            let _ = save_ai_review_stack(cache, &deterministic_stack, provider, code_version_key);
            let generated_stack = display_stack.clone();
            set_stack_success(
                model,
                detail_key,
                request_key,
                display_stack,
                Some("Built Guided Review stack.".to_string()),
                cx,
            )
            .await;
            Some(GeneratedReviewStack {
                stack: generated_stack,
                semantic_review,
            })
        }
        Err(error) => {
            capture_review_intelligence_failure(
                detail,
                provider,
                "guided_review_stack",
                "stack_generation",
                request_key,
                Some(code_version_key),
                true,
                &error,
            );
            set_stack_error(model, detail_key, request_key, detail, error, cx).await;
            None
        }
    }
}

fn guided_review_stack_discovery_options() -> StackDiscoveryOptions {
    StackDiscoveryOptions {
        enable_github_native: true,
        enable_branch_topology: true,
        enable_local_metadata: true,
        enable_ai_virtual: false,
        enable_sem_virtual: true,
        enable_virtual_commits: false,
        enable_virtual_semantic: true,
        ai_provider: None,
        ..StackDiscoveryOptions::default()
    }
}

fn capture_review_intelligence_failure(
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
    feature: &str,
    phase: &str,
    request_key: &str,
    code_version_key: Option<&str>,
    fallback: bool,
    error: &str,
) {
    sentry_diagnostics::capture_ai_failure(feature, Some(provider.slug()), error, |scope| {
        scope.set_tag("ai.phase", phase);
        scope.set_tag("ai.fallback", fallback);
        scope.set_tag("pr.local", local_review::is_local_review_detail(detail));
        scope.set_tag("pr.repository", &detail.repository);
        scope.set_extra("repository", json!(&detail.repository));
        scope.set_extra("pullRequestNumber", json!(detail.number));
        scope.set_extra("requestKey", json!(request_key));
        scope.set_extra("codeVersionKey", json!(code_version_key));
        scope.set_extra("baseRefName", json!(&detail.base_ref_name));
        scope.set_extra("headRefName", json!(&detail.head_ref_name));
        scope.set_extra("headRefOid", json!(&detail.head_ref_oid));
        scope.set_extra("changedFiles", json!(detail.changed_files));
        scope.set_extra("additions", json!(detail.additions));
        scope.set_extra("deletions", json!(detail.deletions));
    });
}

async fn generate_or_load_partner(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
    partner_request_key: &str,
    local_repo_status: &local_repo::LocalRepositoryStatus,
    stack: ReviewStack,
    semantic_review: Option<semantic_review::RemissSemanticReview>,
    force: bool,
    lsp_session_manager: std::sync::Arc<crate::lsp::LspSessionManager>,
    cx: &mut AsyncWindowContext,
) {
    if !force {
        let cached = cx
            .background_executor()
            .spawn({
                let cache = CacheStore::clone(cache);
                let detail = detail.clone();
                async move { review_partner::load_review_partner_context(&cache, &detail, provider) }
            })
            .await;

        if let Ok(Some(partner)) = cached {
            set_partner_success(
                model,
                detail_key,
                partner_request_key,
                partner,
                Some("Loaded cached Review Partner context.".to_string()),
                cx,
            )
            .await;
            if let Some(working_directory) = local_repo_status.path.as_ref() {
                spawn_review_memory_candidate_extraction(
                    CacheStore::clone(cache),
                    detail.clone(),
                    provider,
                    working_directory.clone(),
                    false,
                );
            }
            return;
        }
    }

    let Some(working_directory) = local_repo_status.path.as_ref() else {
        set_partner_error(
            model,
            detail_key,
            partner_request_key,
            local_repo_status.message.clone(),
            cx,
        )
        .await;
        return;
    };

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.review_partner_state.request_key.as_deref()
                    == Some(partner_request_key)
                {
                    detail_state.review_partner_state.loading = false;
                    detail_state.review_partner_state.generating = true;
                    detail_state.review_partner_state.progress_text =
                        Some("Checking usages and codebase context.".to_string());
                    detail_state.review_partner_state.error = None;
                    detail_state.review_partner_state.message = None;
                    detail_state.review_partner_state.success = false;
                }
            }
            cx.notify();
        })
        .ok();

    let (progress_tx, progress_rx) = mpsc::channel::<ReviewAiProgressUpdate>();
    let (result_tx, result_rx) =
        mpsc::channel::<Result<review_partner::GeneratedReviewPartnerContext, String>>();
    std::thread::spawn({
        let cache = CacheStore::clone(cache);
        let detail = detail.clone();
        let stack = stack.clone();
        let semantic_review = semantic_review.clone();
        let working_directory = PathBuf::from(working_directory);
        let head_oid = checkout_head_oid(local_repo_status);
        let lsp_session_manager = lsp_session_manager.clone();
        let partner_request_key = partner_request_key.to_string();
        move || {
            let result = run_foreground_blocking(|| {
                let semantic_review = semantic_review.or_else(|| {
                    semantic_review::build_and_cache_semantic_review(
                        &cache,
                        &detail,
                        &stack.atoms,
                        &detail.repository,
                        working_directory.as_path(),
                        head_oid.as_deref(),
                        force,
                    )
                });
                let structural_evidence = head_oid
                    .as_deref()
                    .map(|head_oid| {
                        structural_evidence::build_structural_evidence_pack(
                            &cache,
                            &detail,
                            &stack.atoms,
                            &detail.repository,
                            working_directory.as_path(),
                            head_oid,
                        )
                    })
                    .unwrap_or_else(|| {
                        let mut pack = structural_evidence::StructuralEvidencePack::empty();
                        pack.warnings.push(
                            "Structural evidence could not be built because checkout head was unavailable."
                                .to_string(),
                        );
                        pack
                    });
                let memory_targets = stack
                    .atoms
                    .iter()
                    .flat_map(|atom| {
                        atom.symbol_name
                            .iter()
                            .chain(atom.defined_symbols.iter())
                            .map(move |symbol| review_memory::ReviewMemoryTarget {
                                path: atom.path.clone(),
                                symbol_name: Some(symbol.clone()),
                                symbol_kind: atom.semantic_kind.clone(),
                            })
                    })
                    .collect::<Vec<_>>();
                let review_memory = review_memory::review_memory_prompt_context_for_detail(
                    &cache,
                    &detail,
                    &memory_targets,
                    3,
                )
                .unwrap_or_default();
                let mut input = review_partner::build_review_partner_generation_input(
                    &detail,
                    provider,
                    working_directory.to_string_lossy().as_ref(),
                    stack,
                    structural_evidence,
                    semantic_review,
                    Some(lsp_session_manager),
                );
                input.review_memory = review_memory;

                let partner = match review_partner::generate_review_partner_context_with_progress(
                    &cache,
                    input.clone(),
                    &mut |progress| {
                        let _ = progress_tx.send(progress);
                    },
                ) {
                    Ok(partner) => partner,
                    Err(error) => {
                        capture_review_intelligence_failure(
                            &detail,
                            provider,
                            "review_partner",
                            "context_generation",
                            &partner_request_key,
                            Some(&input.code_version_key),
                            true,
                            &error,
                        );
                        review_partner::fallback_review_partner_context(
                            &input,
                            Some(format!("AI Review Partner context unavailable: {error}")),
                        )
                    }
                };
                Ok(partner)
            });
            let _ = result_tx.send(result);
        }
    });

    let partner_result = loop {
        while let Ok(progress) = progress_rx.try_recv() {
            model
                .update(cx, |state, cx| {
                    if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                        if detail_state.review_partner_state.request_key.as_deref()
                            == Some(partner_request_key)
                        {
                            detail_state.review_partner_state.progress_text =
                                Some(progress_status_text(&progress));
                        }
                    }
                    cx.notify();
                })
                .ok();
        }

        match result_rx.try_recv() {
            Ok(Ok(partner)) => break partner,
            Ok(Err(error)) => {
                let code_version_key = review_code_version_key(detail);
                capture_review_intelligence_failure(
                    detail,
                    provider,
                    "review_partner",
                    "context_result",
                    partner_request_key,
                    Some(&code_version_key),
                    false,
                    &error,
                );
                set_partner_error(model, detail_key, partner_request_key, error, cx).await;
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                let error = "Review Partner generation stopped before returning a result.";
                let code_version_key = review_code_version_key(detail);
                capture_review_intelligence_failure(
                    detail,
                    provider,
                    "review_partner",
                    "context_result",
                    partner_request_key,
                    Some(&code_version_key),
                    false,
                    error,
                );
                set_partner_error(
                    model,
                    detail_key,
                    partner_request_key,
                    error.to_string(),
                    cx,
                )
                .await;
                return;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        cx.background_executor()
            .spawn(async move {
                std::thread::sleep(Duration::from_millis(120));
            })
            .await;
    };

    let partner_message = if partner_result.fallback_reason.is_some() {
        Some("Using deterministic Review Partner fallback.".to_string())
    } else {
        Some("Generated Review Partner context.".to_string())
    };

    set_partner_success(
        model,
        detail_key,
        partner_request_key,
        partner_result,
        partner_message,
        cx,
    )
    .await;

    spawn_review_memory_candidate_extraction(
        CacheStore::clone(cache),
        detail.clone(),
        provider,
        working_directory.clone(),
        force,
    );
}

fn spawn_review_memory_candidate_extraction(
    cache: CacheStore,
    detail: PullRequestDetail,
    provider: ReviewAiProvider,
    working_directory: String,
    force: bool,
) {
    std::thread::spawn(move || {
        let _ = run_background_blocking(|| {
            review_memory::generate_llm_review_memory_candidates(
                &cache,
                &detail,
                provider,
                &working_directory,
                force,
            )
        });
    });
}

async fn generate_or_load_brief(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: &PullRequestDetail,
    provider: ReviewAiProvider,
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
                .review_ai_provider_statuses
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

    let (progress_tx, progress_rx) = mpsc::channel::<ReviewAiProgressUpdate>();
    let (result_tx, result_rx) = mpsc::channel::<Result<review_brief::ReviewBrief, String>>();
    std::thread::spawn({
        let cache = CacheStore::clone(cache);
        let detail = detail.clone();
        let working_directory = working_directory.clone();
        move || {
            let result = run_foreground_blocking(|| {
                let mut input = review_brief::build_review_brief_generation_input(
                    &detail,
                    provider,
                    &working_directory,
                );
                input.review_memory =
                    review_memory::review_memory_prompt_context_for_detail(&cache, &detail, &[], 3)
                        .unwrap_or_default();
                review_brief::generate_review_brief_with_progress(&cache, input, &mut |progress| {
                    let _ = progress_tx.send(progress);
                })
            });
            let _ = result_tx.send(result);
        }
    });

    let generation_result = loop {
        while let Ok(progress) = progress_rx.try_recv() {
            model
                .update(cx, |state, cx| {
                    if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                        set_review_brief_progress(
                            detail_state,
                            request_key,
                            false,
                            true,
                            &review_brief_loading_status_text(provider, &progress),
                        );
                    }
                    cx.notify();
                })
                .ok();
        }

        match result_rx.try_recv() {
            Ok(result) => break result,
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err("Review brief generation stopped before returning a result.".to_string());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        cx.background_executor()
            .spawn(async move {
                std::thread::sleep(Duration::from_millis(120));
            })
            .await;
    };

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
            let code_version_key = review_code_version_key(detail);
            capture_review_intelligence_failure(
                detail,
                provider,
                "review_brief",
                "brief_generation",
                request_key,
                Some(&code_version_key),
                false,
                &error,
            );
            set_brief_error(model, detail_key, request_key, error, cx).await;
        }
    }
}

async fn fail_checkout(
    model: &Entity<AppState>,
    detail_key: &str,
    scope: ReviewIntelligenceScope,
    _provider: ReviewAiProvider,
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

                if scope.includes_partner() {
                    detail_state.review_partner_state.loading = false;
                    detail_state.review_partner_state.generating = false;
                    detail_state.review_partner_state.error = Some(error.to_string());
                    detail_state.review_partner_state.progress_text = None;
                    detail_state.review_partner_state.message = None;
                    detail_state.review_partner_state.success = false;
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

async fn set_partner_success(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    partner: review_partner::GeneratedReviewPartnerContext,
    message: Option<String>,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.review_partner_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.stack =
                        Some(std::sync::Arc::new(partner.stack.clone()));
                    detail_state.review_partner_state.document = Some(std::sync::Arc::new(partner));
                    detail_state.review_partner_state.loading = false;
                    detail_state.review_partner_state.generating = false;
                    detail_state.review_partner_state.progress_text = None;
                    detail_state.review_partner_state.error = None;
                    detail_state.review_partner_state.message = message;
                    detail_state.review_partner_state.success = true;
                    detail_state.review_partner_state.loading_focus_keys.clear();
                    detail_state.review_partner_state.focus_errors.clear();
                    state.review_stack_cache.borrow_mut().clear();
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_partner_error(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.review_partner_state.request_key.as_deref() == Some(request_key) {
                    detail_state.review_partner_state.loading = false;
                    detail_state.review_partner_state.generating = false;
                    detail_state.review_partner_state.progress_text = None;
                    detail_state.review_partner_state.error = Some(error.clone());
                    detail_state.review_partner_state.message = None;
                    detail_state.review_partner_state.success = false;
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
    let stack = guided_review_stack_for_error(detail, &error);
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

fn progress_status_text(progress: &ReviewAiProgressUpdate) -> String {
    progress
        .detail
        .as_deref()
        .or(progress.log.as_deref())
        .unwrap_or(progress.summary.as_str())
        .to_string()
}

fn progress_detail_text(progress: &ReviewAiProgressUpdate) -> String {
    progress
        .detail
        .as_deref()
        .or(progress.log.as_deref())
        .unwrap_or(progress.summary.as_str())
        .to_string()
}

fn review_brief_loading_status_text(
    provider: ReviewAiProvider,
    progress: &ReviewAiProgressUpdate,
) -> String {
    match progress.stage.as_str() {
        "command" | "tool" => "Reading the local checkout for the review brief.".to_string(),
        "command_failed" => "Retrying checkout inspection for the review brief.".to_string(),
        "finalizing" => "Finalizing the review brief.".to_string(),
        _ => format!("{} is generating the review brief.", provider.label()),
    }
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
    provider: ReviewAiProvider,
) -> String {
    format!(
        "{}:{}#{}:{}",
        provider.slug(),
        detail.repository,
        detail.number,
        review_code_version_key(detail)
    )
}

fn detail_brief_request_matches(
    state: &AppState,
    detail_key: &str,
    provider: ReviewAiProvider,
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

fn detail_partner_request_matches(
    state: &AppState,
    detail_key: &str,
    provider: ReviewAiProvider,
    request_key: &str,
) -> bool {
    state
        .detail_states
        .get(detail_key)
        .and_then(|detail_state| detail_state.snapshot.as_ref())
        .and_then(|snapshot| snapshot.detail.as_ref())
        .map(|detail| build_review_partner_request_key(detail, provider) == request_key)
        .unwrap_or(false)
}

fn guided_review_stack_for_error(detail: &PullRequestDetail, message: &str) -> ReviewStack {
    ReviewStack {
        id: format!("stack-error:{}#{}", detail.repository, detail.number),
        repository: detail.repository.clone(),
        selected_pr_number: detail.number,
        source: crate::stacks::model::StackSource::VirtualSemantic,
        kind: crate::stacks::model::StackKind::Virtual,
        confidence: Confidence::Low,
        trunk_branch: Some(detail.base_ref_name.clone()),
        base_oid: detail.base_ref_oid.clone(),
        head_oid: detail.head_ref_oid.clone(),
        layers: Vec::new(),
        atoms: Vec::new(),
        warnings: vec![crate::stacks::model::StackWarning::new(
            "guided-review-stack-unavailable",
            format!("Guided Review stack generation failed. {message}"),
        )],
        provider: None,
        generated_at_ms: crate::stacks::model::stack_now_ms(),
        generator_version: crate::stacks::model::STACK_GENERATOR_VERSION.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        guided_review_stack_discovery_options, review_brief_loading_status_text,
        review_intelligence_request_key, set_review_brief_error, set_review_brief_progress,
        ReviewIntelligenceScope,
    };
    use crate::{
        github::PullRequestDetail,
        review_ai::{ReviewAiProgressUpdate, ReviewAiProvider},
        state::DetailState,
    };

    #[test]
    fn review_intelligence_request_key_ignores_metadata_updates_when_head_matches() {
        let first = detail("2026-04-17T10:00:00Z", Some("head123"), "diff-one");
        let second = detail("2026-04-17T11:00:00Z", Some("head123"), "diff-two");

        assert_eq!(
            review_intelligence_request_key(&first, ReviewAiProvider::Codex),
            review_intelligence_request_key(&second, ReviewAiProvider::Codex)
        );
    }

    #[test]
    fn review_intelligence_request_key_varies_by_provider() {
        let detail = detail("2026-04-17T10:00:00Z", Some("head123"), "diff-one");

        assert_ne!(
            review_intelligence_request_key(&detail, ReviewAiProvider::Codex),
            review_intelligence_request_key(&detail, ReviewAiProvider::Copilot)
        );
    }

    #[test]
    fn guided_review_stack_generation_prefers_real_and_sem_without_ai_planning() {
        let options = guided_review_stack_discovery_options();

        assert!(options.enable_github_native);
        assert!(options.enable_branch_topology);
        assert!(options.enable_local_metadata);
        assert!(!options.enable_ai_virtual);
        assert!(options.enable_sem_virtual);
        assert!(!options.enable_virtual_commits);
        assert!(options.enable_virtual_semantic);
        assert_eq!(options.ai_provider, None);
    }

    #[test]
    fn review_intelligence_scopes_only_cover_active_surfaces() {
        let scopes = [
            ReviewIntelligenceScope::All,
            ReviewIntelligenceScope::BriefOnly,
            ReviewIntelligenceScope::StackOnly,
        ];

        assert_eq!(scopes.len(), 3);
        assert!(ReviewIntelligenceScope::All.includes_stack());
        assert!(ReviewIntelligenceScope::All.includes_partner());
        assert!(ReviewIntelligenceScope::All.includes_brief());
        assert!(!ReviewIntelligenceScope::BriefOnly.includes_stack());
        assert!(!ReviewIntelligenceScope::BriefOnly.includes_partner());
        assert!(ReviewIntelligenceScope::StackOnly.includes_partner());
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

    #[test]
    fn review_brief_loading_status_hides_streamed_response_text() {
        let progress = ReviewAiProgressUpdate {
            stage: "drafting".to_string(),
            summary: "GitHub Copilot is drafting the response".to_string(),
            detail: Some("{\"briefParagraph\":\"Partial generated brief".to_string()),
            log: Some("Drafting Copilot response".to_string()),
            log_file_path: None,
        };

        assert_eq!(
            review_brief_loading_status_text(ReviewAiProvider::Copilot, &progress),
            "Copilot is generating the review brief."
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
            commits: Vec::new(),
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
