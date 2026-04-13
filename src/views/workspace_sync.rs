use std::time::Duration;

use gpui::*;

use crate::{notifications, state::AppState};

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
            model
                .update(cx, |state, cx| {
                    state.workspace_syncing = false;
                    state.gh_available = outcome.workspace.auth.is_authenticated;
                    state.workspace = Some(outcome.workspace);
                    state.workspace_error = None;
                    cx.notify();
                })
                .ok();
            notifications::deliver_system_notifications(&notifications);
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
