use std::sync::Arc;

use gpui::{App, AsyncWindowContext, Entity, Window};

use crate::github;
use crate::notifications;
use crate::review_memory;
use crate::review_session::load_review_session;
use crate::state::{pr_key, AppState, PullRequestSurface, SectionId};

use super::super::diff_view::{ensure_structural_diff_warmup_started, warm_structural_diffs_flow};

const DETAIL_AUTO_REFRESH_TTL_MS: i64 = 5 * 60 * 1000;

pub fn open_pull_request(
    state: &Entity<AppState>,
    summary: github::PullRequestSummary,
    window: &mut Window,
    cx: &mut App,
) {
    let key = pr_key(&summary.repository, summary.number);
    let repository = summary.repository.clone();
    let number = summary.number;
    let cached_review_session = {
        let cache = state.read(cx).cache.clone();
        load_review_session(cache.as_ref(), &key).ok().flatten()
    };
    let initial_surface = PullRequestSurface::Files;
    let load_plan = {
        let s = state.read(cx);
        plan_pull_request_open(&s, &key)
    };

    state.update(cx, |s, cx| {
        if !s
            .open_tabs
            .iter()
            .any(|t| pr_key(&t.repository, t.number) == key)
        {
            s.open_tabs.insert(0, summary);
        }
        s.set_active_section(SectionId::Pulls);
        s.active_surface = initial_surface;
        s.active_pr_key = Some(key.clone());
        s.palette_open = false;
        s.palette_selected_index = 0;
        s.review_body.clear();
        s.review_editor_active = false;
        s.review_message = None;
        s.review_success = false;
        s.pr_header_compact = false;

        s.detail_states.entry(key.clone()).or_default();
        s.apply_review_session_document(&key, cached_review_session.clone());
        s.ensure_active_selected_file_is_valid();
        let detail_state = s.detail_states.entry(key.clone()).or_default();
        detail_state.loading = load_plan.show_loading;
        if load_plan.load_cached_snapshot || load_plan.sync_live {
            detail_state.error = None;
        }
        cx.notify();
    });

    ensure_structural_diff_warmup_started(state, window, cx);

    if !load_plan.load_cached_snapshot && !load_plan.sync_live {
        return;
    }

    // Load PR detail in background
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |s, _| s.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let detail_key = pr_key(&repository, number);
            let mut should_sync = load_plan.sync_live;

            if load_plan.load_cached_snapshot {
                let cached_result = cx
                    .background_executor()
                    .spawn({
                        let cache = cache.clone();
                        let repository = repository.clone();
                        async move { github::load_pull_request_detail(&cache, &repository, number) }
                    })
                    .await;

                should_sync = match &cached_result {
                    Ok(snapshot) => detail_snapshot_needs_background_refresh(snapshot),
                    Err(_) => true,
                };
                let cached_memory_snapshot = cached_result.as_ref().ok().cloned();

                model
                    .update(cx, |s, cx| {
                        let ds = s.detail_states.entry(detail_key.clone()).or_default();
                        match &cached_result {
                            Ok(snapshot) => {
                                ds.snapshot = Some(snapshot.clone());
                                ds.loading = snapshot.detail.is_none() && should_sync;
                                ds.error = None;
                            }
                            Err(error) => {
                                ds.loading = should_sync;
                                ds.error = Some(error.clone());
                            }
                        }
                        s.ensure_active_selected_file_is_valid();
                        cx.notify();
                    })
                    .ok();

                if let Some(snapshot) = cached_memory_snapshot {
                    record_review_memory_snapshot(cache.clone(), snapshot, cx).await;
                }

                warm_structural_diffs_flow(model.clone(), cx).await;
                refresh_brief_if_active_overview(model.clone(), &detail_key, cx).await;
            }

            if !should_sync {
                return;
            }

            model
                .update(cx, |s, cx| {
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = ds
                        .snapshot
                        .as_ref()
                        .and_then(|sn| sn.detail.as_ref())
                        .is_none();
                    ds.syncing = true;
                    ds.error = None;
                    cx.notify();
                })
                .ok();

            let sync_result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let repository = repository.clone();
                    async move {
                        notifications::sync_pull_request_detail_with_read_state(
                            &cache,
                            &repository,
                            number,
                        )
                    }
                })
                .await;
            let sync_memory_snapshot = sync_result
                .as_ref()
                .ok()
                .map(|(snapshot, _)| snapshot.clone());

            model
                .update(cx, |s, cx| {
                    let mut next_unread_ids = None;
                    let ds = s.detail_states.entry(detail_key.clone()).or_default();
                    ds.loading = false;
                    ds.syncing = false;
                    match sync_result {
                        Ok((snapshot, unread_ids)) => {
                            ds.snapshot = Some(snapshot);
                            ds.error = None;
                            next_unread_ids = Some(unread_ids);
                        }
                        Err(e) => {
                            ds.error = Some(e);
                        }
                    }
                    s.ensure_active_selected_file_is_valid();
                    if let Some(unread_ids) = next_unread_ids {
                        s.unread_review_comment_ids = unread_ids;
                    }
                    cx.notify();
                })
                .ok();

            if let Some(snapshot) = sync_memory_snapshot {
                record_review_memory_snapshot(cache.clone(), snapshot, cx).await;
            }

            warm_structural_diffs_flow(model.clone(), cx).await;
            refresh_brief_if_active_overview(model.clone(), &detail_key, cx).await;
        })
        .detach();
}

async fn record_review_memory_snapshot(
    cache: Arc<crate::cache::CacheStore>,
    snapshot: github::PullRequestDetailSnapshot,
    cx: &mut AsyncWindowContext,
) {
    let Some(detail) = snapshot.detail else {
        return;
    };

    let _ = cx
        .background_executor()
        .spawn(async move { review_memory::record_pull_request_memory(cache.as_ref(), &detail) })
        .await;
}

async fn refresh_brief_if_active_overview(
    model: Entity<AppState>,
    detail_key: &str,
    cx: &mut AsyncWindowContext,
) {
    let should_refresh_brief = model
        .read_with(cx, |state, _| {
            state.active_surface == PullRequestSurface::Overview
                && state.active_pr_key.as_deref() == Some(detail_key)
        })
        .ok()
        .unwrap_or(false);

    if should_refresh_brief {
        crate::review_intelligence::refresh_active_review_brief_flow(model.clone(), true, cx).await;
        crate::review_intelligence::refresh_active_review_partner_flow(model, true, cx).await;
    }
}

#[derive(Clone, Copy)]
struct PullRequestOpenPlan {
    load_cached_snapshot: bool,
    sync_live: bool,
    show_loading: bool,
}

fn plan_pull_request_open(state: &AppState, key: &str) -> PullRequestOpenPlan {
    let Some(detail_state) = state.detail_states.get(key) else {
        return PullRequestOpenPlan {
            load_cached_snapshot: true,
            sync_live: false,
            show_loading: true,
        };
    };

    let has_detail = detail_state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.detail.as_ref())
        .is_some();

    if detail_state.loading || detail_state.syncing {
        return PullRequestOpenPlan {
            load_cached_snapshot: false,
            sync_live: false,
            show_loading: !has_detail,
        };
    }

    if !has_detail {
        return PullRequestOpenPlan {
            load_cached_snapshot: true,
            sync_live: false,
            show_loading: true,
        };
    }

    PullRequestOpenPlan {
        load_cached_snapshot: false,
        sync_live: detail_state
            .snapshot
            .as_ref()
            .map(detail_snapshot_needs_background_refresh)
            .unwrap_or(true),
        show_loading: false,
    }
}

fn detail_snapshot_needs_background_refresh(snapshot: &github::PullRequestDetailSnapshot) -> bool {
    if snapshot.detail.is_none() {
        return true;
    }

    let Some(fetched_at_ms) = snapshot.fetched_at_ms else {
        return true;
    };

    current_time_ms().saturating_sub(fetched_at_ms) > DETAIL_AUTO_REFRESH_TTL_MS
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
