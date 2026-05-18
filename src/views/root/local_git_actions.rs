use std::path::PathBuf;

use gpui::*;

use crate::local_git::{self, LocalGitOperationKind, LocalGitOperationState};
use crate::local_review;
use crate::state::AppState;

#[derive(Clone, Debug)]
enum LocalGitCommand {
    StageFile(String),
    UnstageFile(String),
    StageAll,
    UnstageAll,
    Commit(String),
    Push,
}

impl LocalGitCommand {
    fn kind(&self) -> LocalGitOperationKind {
        match self {
            Self::StageFile(_) | Self::StageAll => LocalGitOperationKind::Stage,
            Self::UnstageFile(_) | Self::UnstageAll => LocalGitOperationKind::Unstage,
            Self::Commit(_) => LocalGitOperationKind::Commit,
            Self::Push => LocalGitOperationKind::Push,
        }
    }

    fn run(self, root: &PathBuf) -> Result<(), String> {
        match self {
            Self::StageFile(path) => local_git::stage_file(root, &path),
            Self::UnstageFile(path) => local_git::unstage_file(root, &path),
            Self::StageAll => local_git::stage_all(root),
            Self::UnstageAll => local_git::unstage_all(root),
            Self::Commit(message) => local_git::commit_staged(root, &message),
            Self::Push => local_git::push_current_branch(root),
        }
    }

    fn clears_commit_message(&self) -> bool {
        matches!(self, Self::Commit(_))
    }
}

pub(crate) fn trigger_stage_local_file(
    state: &Entity<AppState>,
    path: String,
    window: &mut Window,
    cx: &mut App,
) {
    trigger_local_git_operation(state, LocalGitCommand::StageFile(path), window, cx);
}

pub(crate) fn trigger_unstage_local_file(
    state: &Entity<AppState>,
    path: String,
    window: &mut Window,
    cx: &mut App,
) {
    trigger_local_git_operation(state, LocalGitCommand::UnstageFile(path), window, cx);
}

pub(crate) fn trigger_stage_all_local_changes(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    trigger_local_git_operation(state, LocalGitCommand::StageAll, window, cx);
}

pub(crate) fn trigger_unstage_all_local_changes(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    trigger_local_git_operation(state, LocalGitCommand::UnstageAll, window, cx);
}

pub(crate) fn trigger_commit_local_changes(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let message = state.read(cx).local_commit_message.trim().to_string();
    trigger_local_git_operation(state, LocalGitCommand::Commit(message), window, cx);
}

pub(crate) fn trigger_push_local_branch(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    trigger_local_git_operation(state, LocalGitCommand::Push, window, cx);
}

fn trigger_local_git_operation(
    state: &Entity<AppState>,
    command: LocalGitCommand,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((detail_key, root)) = active_local_review_root(state, cx) else {
        return;
    };

    if matches!(command, LocalGitCommand::Commit(ref message) if message.trim().is_empty()) {
        set_local_git_operation_error(
            state,
            detail_key,
            "Enter a commit message before committing.",
            cx,
        );
        return;
    }

    let operation_kind = command.kind();
    state.update(cx, |state, cx| {
        if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
            detail_state.local_git_operation = LocalGitOperationState::running(operation_kind);
        }
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let command_for_run = command.clone();
            let result = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    async move { command_for_run.run(&root) }
                })
                .await;

            match result {
                Ok(()) => {
                    if command.clears_commit_message() {
                        model
                            .update(cx, |state, _| {
                                state.local_commit_message.clear();
                            })
                            .ok();
                    }
                    super::inspect_and_open_local_review(model, root, false, cx).await;
                }
                Err(error) => {
                    model
                        .update(cx, |state, cx| {
                            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                                detail_state.local_git_operation = LocalGitOperationState {
                                    running: None,
                                    message: None,
                                    error: Some(error.clone()),
                                };
                            } else {
                                state.local_review_error = Some(error.clone());
                            }
                            cx.notify();
                        })
                        .ok();
                }
            }
        })
        .detach();
}

fn active_local_review_root(state: &Entity<AppState>, cx: &App) -> Option<(String, PathBuf)> {
    let s = state.read(cx);
    let detail_key = s.active_pr_key.clone()?;
    let detail = s.active_detail()?;
    if !local_review::is_local_review_detail(detail) {
        return None;
    }
    let root = s
        .active_detail_state()
        .and_then(|detail_state| detail_state.local_repository_status.as_ref())
        .and_then(|status| status.path.as_deref())
        .map(PathBuf::from)
        .or_else(|| {
            s.local_review_repositories
                .iter()
                .find(|repository| repository.repository == detail.repository)
                .map(|repository| PathBuf::from(repository.path.clone()))
        })?;
    Some((detail_key, root))
}

fn set_local_git_operation_error(
    state: &Entity<AppState>,
    detail_key: String,
    error: &str,
    cx: &mut App,
) {
    state.update(cx, |state, cx| {
        if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
            detail_state.local_git_operation = LocalGitOperationState {
                running: None,
                message: None,
                error: Some(error.to_string()),
            };
        }
        cx.notify();
    });
}
