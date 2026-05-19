use std::{collections::BTreeSet, sync::mpsc, time::Duration};

use gpui::*;

use crate::guided_review::{
    build_guided_review_generation_input, build_guided_review_request_key, GeneratedGuidedReview,
};
use crate::local_repo;
use crate::local_review;
use crate::review_ai::{self, ReviewAiProgressUpdate, ReviewAiProvider};
use crate::review_intelligence::{self, ReviewIntelligenceScope};
use crate::review_memory;
use crate::state::{AppState, GuidedReviewState};
use crate::{github, guided_review};

use super::diff_view::{load_local_source_file_content_flow, load_pull_request_file_content_flow};

pub fn refresh_active_guided_review(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    allow_automatic_generation: bool,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            refresh_active_guided_review_flow(model, allow_automatic_generation, cx).await;
        })
        .detach();
}

pub async fn refresh_active_guided_review_flow(
    model: Entity<AppState>,
    allow_automatic_generation: bool,
    cx: &mut AsyncWindowContext,
) {
    let initial = model
        .read_with(cx, |state, _| {
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let existing_local_repository_status = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.local_repository_status.clone());
            Some((
                state.cache.clone(),
                detail_key,
                detail,
                state.review_ai_settings.loaded,
                state.review_ai_settings.settings.clone(),
                state.review_ai_provider_statuses_loaded,
                state.review_ai_provider_statuses.clone(),
                existing_local_repository_status,
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
        existing_local_repository_status,
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

    let provider_statuses = provider_statuses_result.clone().unwrap_or_default();
    let settings = settings_result
        .clone()
        .unwrap_or_else(|_| existing_settings.clone());
    let provider = settings.provider;
    let automatic_generation_enabled = settings.automatically_generates_for(&detail.repository);
    let local_review_repository_status =
        local_review::reusable_local_repository_status(&detail, existing_local_repository_status);
    let local_repository_already_ready = matches!(&local_review_repository_status, Ok(Some(_)));

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
                detail_state.local_repository_loading = !local_repository_already_ready;
                detail_state.local_repository_error = None;
                if let Ok(Some(status)) = local_review_repository_status.as_ref() {
                    detail_state.local_repository_status = Some(status.clone());
                }

                let request_key = build_guided_review_request_key(&detail, provider);
                let guided_review_state = detail_state
                    .guided_review_states
                    .entry(provider)
                    .or_default();
                clear_guided_review_progress(guided_review_state);
                guided_review_state.loading = true;
                guided_review_state.generating = false;
                guided_review_state.request_key = Some(request_key);
                guided_review_state.error = None;
                guided_review_state.message = None;
                guided_review_state.success = false;
            }

            cx.notify();
        })
        .ok();

    let request_key = build_guided_review_request_key(&detail, provider);

    let local_repo_result = match local_review_repository_status {
        Ok(Some(status)) => Ok(status),
        Ok(None) => {
            cx.background_executor()
                .spawn({
                    let cache = cache.clone();
                    let repository = detail.repository.clone();
                    let head_ref_oid = detail.head_ref_oid.clone();
                    async move {
                        local_repo::load_local_repository_status_for_pull_request(
                            &cache,
                            &repository,
                            head_ref_oid.as_deref(),
                        )
                    }
                })
                .await
        }
        Err(error) => Err(error),
    };

    let cached_tour_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let detail = detail.clone();
            async move { guided_review::load_guided_review(&cache, &detail, provider) }
        })
        .await;

    let provider_ready = provider_statuses
        .iter()
        .find(|status| status.provider == provider)
        .map(|status| status.available && status.authenticated)
        .unwrap_or(false);

    let missing_cached_tour = cached_tour_result
        .as_ref()
        .ok()
        .map(|tour| tour.is_none())
        .unwrap_or(false);
    let cached_tour_error = cached_tour_result.as_ref().err().cloned();
    let should_auto_generate = allow_automatic_generation
        && automatic_generation_enabled
        && provider_ready
        && matches!(local_repo_result.as_ref(), Ok(status) if status.path.is_some())
        && missing_cached_tour
        && cached_tour_error.is_none()
        && model
            .read_with(cx, |state, _| {
                !state
                    .automatic_guided_review_request_keys
                    .contains(&request_key)
                    && detail_request_matches(state, &detail_key, provider, &request_key)
            })
            .ok()
            .unwrap_or(false);

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = false;
                match &local_repo_result {
                    Ok(status) => {
                        detail_state.local_repository_status = Some(status.clone());
                        detail_state.local_repository_error = None;
                    }
                    Err(error) => {
                        detail_state.local_repository_error = Some(error.clone());
                    }
                }

                let guided_review_state = detail_state
                    .guided_review_states
                    .entry(provider)
                    .or_default();
                guided_review_state.loading = false;
                clear_guided_review_progress(guided_review_state);
                match &cached_tour_result {
                    Ok(document) => {
                        guided_review_state.document = document.clone();
                        guided_review_state.error = None;
                    }
                    Err(error) => {
                        guided_review_state.document = None;
                        guided_review_state.error = Some(error.clone());
                    }
                }
            }

            cx.notify();
        })
        .ok();

    let changed_file_paths = cached_tour_result
        .as_ref()
        .ok()
        .and_then(|document| document.as_ref())
        .map(guided_review_changed_file_paths)
        .unwrap_or_default();
    let callsite_paths = cached_tour_result
        .as_ref()
        .ok()
        .and_then(|document| document.as_ref())
        .map(guided_review_callsite_paths)
        .unwrap_or_default();

    preload_guided_review_source_files(model.clone(), changed_file_paths, callsite_paths, cx).await;

    if should_auto_generate {
        model
            .update(cx, |state, _| {
                state
                    .automatic_guided_review_request_keys
                    .insert(request_key.clone());
            })
            .ok();
        let scope = if local_review::is_local_review_detail(&detail) {
            ReviewIntelligenceScope::TourOnly
        } else {
            ReviewIntelligenceScope::All
        };
        review_intelligence::run_review_intelligence_flow(model, scope, false, true, cx).await;
    }
}

pub fn trigger_generate_guided_review(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    _automatic: bool,
) {
    let scope = {
        let state = state.read(cx);
        if state
            .active_detail()
            .map(local_review::is_local_review_detail)
            .unwrap_or(false)
        {
            ReviewIntelligenceScope::TourOnly
        } else {
            ReviewIntelligenceScope::All
        }
    };

    review_intelligence::trigger_review_intelligence(state, window, cx, scope, true);
}

pub(crate) async fn generate_guided_review_flow(
    model: Entity<AppState>,
    context: Option<(String, github::PullRequestDetail, ReviewAiProvider, String)>,
    prepared_local_repo_status: Option<local_repo::LocalRepositoryStatus>,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) {
    let initial = if let Some(context) = context {
        let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
        cache.map(|cache| (cache, context))
    } else {
        model
            .read_with(cx, |state, _| {
                let detail = state.active_detail()?.clone();
                let detail_key = state.active_pr_key.clone()?;
                let provider = state.selected_review_ai_provider();
                Some((
                    state.cache.clone(),
                    (
                        detail_key,
                        detail.clone(),
                        provider,
                        build_guided_review_request_key(&detail, provider),
                    ),
                ))
            })
            .ok()
            .flatten()
    };

    let Some((cache, (detail_key, detail, provider, request_key))) = initial else {
        return;
    };

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

    let Some(provider_status) = provider_status else {
        if !automatic {
            set_guided_review_error(
                &model,
                &detail_key,
                provider,
                &request_key,
                "Still checking provider status.".to_string(),
                cx,
            );
        }
        return;
    };

    if !provider_status.available {
        if !automatic {
            set_guided_review_error(
                &model,
                &detail_key,
                provider,
                &request_key,
                format!("{} is not available in this workspace.", provider.label()),
                cx,
            );
        }
        return;
    }

    if !provider_status.authenticated {
        if !automatic {
            set_guided_review_error(
                &model,
                &detail_key,
                provider,
                &request_key,
                provider_status.message,
                cx,
            );
        }
        return;
    }

    let mut prepared_local_repo_status = prepared_local_repo_status;
    let local_review_status_error = if prepared_local_repo_status.is_none() {
        let existing_status = model
            .read_with(cx, |state, _| {
                state
                    .detail_states
                    .get(&detail_key)
                    .and_then(|detail_state| detail_state.local_repository_status.clone())
            })
            .ok()
            .flatten();
        match local_review::reusable_local_repository_status(&detail, existing_status) {
            Ok(status) => {
                prepared_local_repo_status = status;
                None
            }
            Err(error) => Some(error),
        }
    } else {
        None
    };
    let has_prepared_checkout = prepared_local_repo_status.is_some();
    let prepared_checkout_for_ui = prepared_local_repo_status.clone();

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = !has_prepared_checkout;
                detail_state.local_repository_error = None;
                if let Some(status) = prepared_checkout_for_ui.as_ref() {
                    detail_state.local_repository_status = Some(status.clone());
                }

                let guided_review_state = detail_state
                    .guided_review_states
                    .entry(provider)
                    .or_default();
                clear_guided_review_progress(guided_review_state);
                guided_review_state.request_key = Some(request_key.clone());
                guided_review_state.loading = false;
                guided_review_state.generating = true;
                guided_review_state.error = None;
                guided_review_state.message = None;
                guided_review_state.success = false;
                apply_guided_review_progress_message(
                    guided_review_state,
                    if has_prepared_checkout {
                        "Using prepared checkout".to_string()
                    } else {
                        "Preparing local checkout".to_string()
                    },
                    Some(if has_prepared_checkout {
                        format!(
                            "Reusing the checkout prepared for the AI stack before starting {}.",
                            provider.label()
                        )
                    } else {
                        format!(
                            "Checking the linked or managed repository before starting {}.",
                            provider.label()
                        )
                    }),
                    Some(if has_prepared_checkout {
                        "Using the prepared checkout".to_string()
                    } else {
                        "Preparing the local checkout".to_string()
                    }),
                    None,
                );
            }

            cx.notify();
        })
        .ok();

    if let Some(error) = local_review_status_error {
        set_local_repo_error(&model, &detail_key, provider, &request_key, error, cx);
        return;
    }

    let local_repo_result = if let Some(status) = prepared_local_repo_status {
        Ok(status)
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let pull_request_number = detail.number;
                let head_ref_oid = detail.head_ref_oid.clone();
                async move {
                    review_intelligence::run_foreground_blocking(|| {
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

    let Ok(local_repo_status) = local_repo_result else {
        let error = local_repo_result
            .err()
            .unwrap_or_else(|| "Failed to prepare the local repository.".to_string());
        set_local_repo_error(&model, &detail_key, provider, &request_key, error, cx);
        return;
    };

    let Some(working_directory) = local_repo_status.path.clone() else {
        set_local_repo_error(
            &model,
            &detail_key,
            provider,
            &request_key,
            local_repo_status.message.clone(),
            cx,
        );
        return;
    };

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_status = Some(local_repo_status.clone());
                detail_state.local_repository_error = None;
                let guided_review_state = detail_state
                    .guided_review_states
                    .entry(provider)
                    .or_default();
                let checkout_label = match local_repo_status.source.as_str() {
                    "linked" => "linked checkout",
                    _ => "app-managed checkout",
                };
                apply_guided_review_progress_message(
                    guided_review_state,
                    format!("Starting {}", provider.label()),
                    Some(format!(
                        "Launching {} in the {} and sending the pull request context.",
                        provider.label(),
                        checkout_label,
                    )),
                    Some(format!("Starting {}", provider.label())),
                    None,
                );
                cx.notify();
            }
        })
        .ok();

    let mut generation_input =
        build_guided_review_generation_input(&detail, provider, &working_directory);
    generation_input.review_memory =
        review_memory::review_memory_prompt_context_for_detail(&cache, &detail, &[], 3)
            .unwrap_or_default();
    let (progress_tx, progress_rx) = mpsc::channel::<ReviewAiProgressUpdate>();
    let (result_tx, result_rx) = mpsc::channel::<Result<GeneratedGuidedReview, String>>();
    std::thread::spawn({
        let cache = cache.clone();
        move || {
            let result = review_intelligence::run_foreground_blocking(|| {
                guided_review::generate_guided_review_with_progress(
                    &cache,
                    generation_input,
                    |progress| {
                        let _ = progress_tx.send(progress);
                    },
                )
            });
            let _ = result_tx.send(result);
        }
    });
    let generation_result = loop {
        while let Ok(progress) = progress_rx.try_recv() {
            model
                .update(cx, |state, cx| {
                    if !detail_request_matches(state, &detail_key, provider, &request_key) {
                        return;
                    }

                    if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                        let guided_review_state = detail_state
                            .guided_review_states
                            .entry(provider)
                            .or_default();
                        apply_guided_review_progress_update(guided_review_state, progress);
                    }

                    cx.notify();
                })
                .ok();
        }

        match result_rx.try_recv() {
            Ok(result) => {
                while let Ok(progress) = progress_rx.try_recv() {
                    model
                        .update(cx, |state, cx| {
                            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                                return;
                            }

                            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                                let guided_review_state = detail_state
                                    .guided_review_states
                                    .entry(provider)
                                    .or_default();
                                apply_guided_review_progress_update(guided_review_state, progress);
                            }

                            cx.notify();
                        })
                        .ok();
                }
                break result;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err(
                    "The Guided Review walkthrough generator stopped before returning a result."
                        .to_string(),
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

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let guided_review_state = detail_state
                    .guided_review_states
                    .entry(provider)
                    .or_default();
                guided_review_state.generating = false;
                match generation_result {
                    Ok(ref document) => {
                        clear_guided_review_progress(guided_review_state);
                        guided_review_state.document = Some(document.clone());
                        guided_review_state.error = None;
                        guided_review_state.message = Some(if automatic {
                            format!("Cached a {} guide in the background.", provider.label())
                        } else {
                            format!("Generated a {} guide.", provider.label())
                        });
                        guided_review_state.success = true;
                    }
                    Err(ref error) => {
                        guided_review_state.error = Some(error.clone());
                        guided_review_state.message = None;
                        guided_review_state.success = false;
                    }
                }
            }

            cx.notify();
        })
        .ok();

    if let Ok(document) = &generation_result {
        preload_guided_review_source_files(
            model.clone(),
            guided_review_changed_file_paths(document),
            guided_review_callsite_paths(document),
            cx,
        )
        .await;
    }
}

async fn preload_guided_review_source_files(
    model: Entity<AppState>,
    changed_file_paths: BTreeSet<String>,
    callsite_paths: BTreeSet<String>,
    cx: &mut AsyncWindowContext,
) {
    for file_path in changed_file_paths {
        load_pull_request_file_content_flow(model.clone(), Some(file_path), cx).await;
    }

    for file_path in callsite_paths {
        load_local_source_file_content_flow(model.clone(), file_path, cx).await;
    }
}

fn guided_review_changed_file_paths(tour: &GeneratedGuidedReview) -> BTreeSet<String> {
    tour.steps
        .iter()
        .filter_map(|step| {
            step.file_path
                .clone()
                .or_else(|| step.anchor.as_ref().map(|anchor| anchor.file_path.clone()))
        })
        .filter(|path| !path.trim().is_empty())
        .collect()
}

fn guided_review_callsite_paths(tour: &GeneratedGuidedReview) -> BTreeSet<String> {
    tour.sections
        .iter()
        .flat_map(|section| {
            section
                .callsites
                .iter()
                .map(|callsite| callsite.path.clone())
        })
        .filter(|path| !path.trim().is_empty())
        .collect()
}

const MAX_GUIDED_REVIEW_PROGRESS_LOG_ITEMS: usize = 10;

fn clear_guided_review_progress(guided_review_state: &mut GuidedReviewState) {
    guided_review_state.progress_summary = None;
    guided_review_state.progress_detail = None;
    guided_review_state.progress_log.clear();
    guided_review_state.progress_log_file_path = None;
}

fn push_guided_review_progress_log(guided_review_state: &mut GuidedReviewState, entry: String) {
    let normalized = entry.trim();
    if normalized.is_empty() {
        return;
    }

    if guided_review_state
        .progress_log
        .last()
        .map(|existing| existing == normalized)
        .unwrap_or(false)
    {
        return;
    }

    guided_review_state
        .progress_log
        .push(normalized.to_string());
    if guided_review_state.progress_log.len() > MAX_GUIDED_REVIEW_PROGRESS_LOG_ITEMS {
        let overflow =
            guided_review_state.progress_log.len() - MAX_GUIDED_REVIEW_PROGRESS_LOG_ITEMS;
        guided_review_state.progress_log.drain(0..overflow);
    }
}

fn apply_guided_review_progress_message(
    guided_review_state: &mut GuidedReviewState,
    summary: String,
    detail: Option<String>,
    log_entry: Option<String>,
    log_file_path: Option<String>,
) {
    guided_review_state.progress_summary = Some(summary);
    guided_review_state.progress_detail = detail.clone();
    if let Some(path) = log_file_path {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            guided_review_state.progress_log_file_path = Some(trimmed.to_string());
        }
    }

    if let Some(log_entry) = log_entry.or_else(|| detail.clone()) {
        push_guided_review_progress_log(guided_review_state, log_entry);
    }
}

fn apply_guided_review_progress_update(
    guided_review_state: &mut GuidedReviewState,
    progress: ReviewAiProgressUpdate,
) {
    apply_guided_review_progress_message(
        guided_review_state,
        progress.summary,
        progress.detail,
        progress.log,
        progress.log_file_path,
    );
}

fn set_guided_review_error(
    model: &Entity<AppState>,
    detail_key: &str,
    provider: ReviewAiProvider,
    request_key: &str,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, detail_key, provider, request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                let guided_review_state = detail_state
                    .guided_review_states
                    .entry(provider)
                    .or_default();
                guided_review_state.generating = false;
                guided_review_state.loading = false;
                guided_review_state.error = Some(error);
                guided_review_state.message = None;
                guided_review_state.success = false;
            }

            cx.notify();
        })
        .ok();
}

fn set_local_repo_error(
    model: &Entity<AppState>,
    detail_key: &str,
    provider: ReviewAiProvider,
    request_key: &str,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, detail_key, provider, request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_error = Some(error.clone());

                let guided_review_state = detail_state
                    .guided_review_states
                    .entry(provider)
                    .or_default();
                guided_review_state.generating = false;
                guided_review_state.loading = false;
                guided_review_state.error = Some(error);
                guided_review_state.message = None;
                guided_review_state.success = false;
            }

            cx.notify();
        })
        .ok();
}

fn detail_request_matches(
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
        .map(|detail| build_guided_review_request_key(detail, provider) == request_key)
        .unwrap_or(false)
}
