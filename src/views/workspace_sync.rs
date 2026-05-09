use std::time::Duration;

use gpui::*;

use crate::{
    code_tour, code_tour_background, notifications,
    state::{pr_key, AppState},
};

use super::diff_view::warm_structural_diffs_flow;

pub const WORKSPACE_SYNC_POLL_INTERVAL: Duration = Duration::from_secs(90);

pub fn trigger_sync_workspace(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.workspace_syncing {
            return;
        }

        state.workspace_syncing = true;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            sync_workspace_flow(model, cx).await;
        })
        .detach();
}

pub async fn sync_workspace_flow(model: Entity<AppState>, cx: &mut AsyncWindowContext) {
    let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
    let Some(cache) = cache else { return };

    let result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            async move { notifications::sync_workspace_with_notifications(&cache) }
        })
        .await;

    match result {
        Ok(outcome) => {
            let notifications = outcome.notifications.clone();
            let workspace = outcome.workspace.clone();
            let review_detail_snapshots = outcome.review_detail_snapshots.clone();
            model
                .update(cx, |state, cx| {
                    state.workspace_syncing = false;
                    state.gh_available = outcome.workspace.auth.is_authenticated;
                    state.workspace = Some(outcome.workspace);
                    state.unread_review_comment_ids = outcome.unread_review_comment_ids;
                    for snapshot in review_detail_snapshots {
                        if let Some(detail) = snapshot.detail.as_ref() {
                            let key = pr_key(&detail.repository, detail.number);
                            let detail_state = state.detail_states.entry(key).or_default();
                            detail_state.snapshot = Some(snapshot);
                            detail_state.loading = false;
                            detail_state.error = None;
                        }
                    }
                    state.workspace_error = None;
                    cx.notify();
                })
                .ok();
            notifications::deliver_system_notifications(&notifications);

            warm_structural_diffs_flow(model.clone(), cx).await;

            let should_sync_background_tours = model
                .read_with(cx, |state, _| !state.code_tour_settings.background_syncing)
                .ok()
                .unwrap_or(false);

            if should_sync_background_tours {
                model
                    .update(cx, |state, cx| {
                        state.code_tour_settings.background_syncing = true;
                        state.code_tour_settings.background_error = None;
                        state.code_tour_settings.background_message =
                            Some("Refreshing automatic background guides...".to_string());
                        cx.notify();
                    })
                    .ok();

                let settings_result = cx
                    .background_executor()
                    .spawn({
                        let cache = cache.clone();
                        async move { code_tour::load_code_tour_settings(&cache) }
                    })
                    .await;

                match settings_result {
                    Ok(settings) => {
                        model
                            .update(cx, |state, cx| {
                                state.code_tour_settings.settings = settings.clone();
                                state.code_tour_settings.loaded = true;
                                state.code_tour_settings.loading = false;
                                state.code_tour_settings.error = None;
                                cx.notify();
                            })
                            .ok();

                        let sync_result = cx
                            .background_executor()
                            .spawn({
                                let cache = cache.clone();
                                let workspace = workspace.clone();
                                let settings = settings.clone();
                                async move {
                                    code_tour_background::sync_workspace_code_tours(
                                        &cache, &workspace, &settings,
                                    )
                                }
                            })
                            .await;

                        model
                            .update(cx, |state, cx| {
                                state.code_tour_settings.background_syncing = false;
                                match sync_result {
                                    Ok(outcome) => {
                                        state.code_tour_settings.background_message =
                                            Some(outcome.summary());
                                        state.code_tour_settings.background_error = None;
                                    }
                                    Err(error) => {
                                        state.code_tour_settings.background_message = None;
                                        state.code_tour_settings.background_error = Some(error);
                                    }
                                }
                                cx.notify();
                            })
                            .ok();
                    }
                    Err(error) => {
                        model
                            .update(cx, |state, cx| {
                                state.code_tour_settings.background_syncing = false;
                                state.code_tour_settings.background_message = None;
                                state.code_tour_settings.background_error = Some(error.clone());
                                state.code_tour_settings.error = Some(error);
                                cx.notify();
                            })
                            .ok();
                    }
                }
            }
        }
        Err(error) => {
            model
                .update(cx, |state, cx| {
                    state.workspace_syncing = false;
                    state.workspace_error = Some(error);
                    cx.notify();
                })
                .ok();
        }
    }
}

pub async fn wait_for_workspace_poll_interval(cx: &mut AsyncWindowContext) {
    cx.background_executor()
        .spawn(async move {
            std::thread::sleep(WORKSPACE_SYNC_POLL_INTERVAL);
        })
        .await;
}
