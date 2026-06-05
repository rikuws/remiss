use crate::commit_timeline::{
    filtered_detail_for_commit, selected_file_for_commit_detail, timeline_items,
    CommitDiffCacheEntry, CommitDiffDataset, CommitDiffFilter, CommitDiffKey,
    CommitTimelineContext,
};
use crate::github::PullRequestDetail;
use crate::review_session::ReviewCenterMode;

use super::{AppState, DetailState};

#[derive(Clone, Debug)]
pub enum ActiveCommitDiffStatus {
    All,
    Disabled,
    Loading,
    Loaded(PullRequestDetail),
    Error(String),
}

impl AppState {
    fn active_detail_state_mut(&mut self) -> Option<&mut DetailState> {
        let key = self.active_pr_key.clone()?;
        self.detail_states.get_mut(&key)
    }

    pub fn active_commit_timeline_filter(&self) -> CommitDiffFilter {
        self.active_detail_state()
            .and_then(|detail_state| {
                detail_state
                    .commit_diff_state
                    .preview_filter
                    .clone()
                    .or_else(|| Some(detail_state.commit_diff_state.selected_filter.clone()))
            })
            .unwrap_or_default()
    }

    pub fn active_commit_filter(&self) -> CommitDiffFilter {
        self.active_detail_state()
            .map(|detail_state| detail_state.commit_diff_state.selected_filter.clone())
            .unwrap_or_default()
    }

    pub fn active_commit_timeline_context(
        &self,
        detail: &PullRequestDetail,
    ) -> CommitTimelineContext {
        if !crate::local_review::is_local_review_detail(detail) {
            return CommitTimelineContext::default();
        }

        CommitTimelineContext {
            local_path: self
                .active_detail_state()
                .and_then(|detail_state| detail_state.local_repository_status.as_ref())
                .and_then(|status| status.path.clone()),
            has_uncommitted: crate::local_review::detail_has_uncommitted_changes(detail),
        }
    }

    pub fn active_commit_filter_applies(&self) -> bool {
        if self.effective_review_center_mode() != ReviewCenterMode::SemanticDiff {
            return false;
        }
        if self.active_detail().is_none() {
            return false;
        }
        !self.active_commit_filter().is_all()
    }

    pub fn active_commit_filter_read_only(&self) -> bool {
        self.active_commit_filter_applies()
    }

    pub fn active_commit_diff_status(&self) -> ActiveCommitDiffStatus {
        let Some(detail) = self.active_detail() else {
            return ActiveCommitDiffStatus::Disabled;
        };
        if self.effective_review_center_mode() != ReviewCenterMode::SemanticDiff {
            return ActiveCommitDiffStatus::Disabled;
        }

        let Some(detail_state) = self.active_detail_state() else {
            return ActiveCommitDiffStatus::Disabled;
        };
        if detail_state.commit_diff_state.selected_filter.is_all() {
            return ActiveCommitDiffStatus::All;
        }

        let context = self.active_commit_timeline_context(detail);
        match detail_state
            .commit_diff_state
            .selected_entry(detail, &context)
        {
            Some(CommitDiffCacheEntry::Loaded(dataset)) => {
                ActiveCommitDiffStatus::Loaded(filtered_detail_for_commit(detail, dataset))
            }
            Some(CommitDiffCacheEntry::Error(error)) => {
                ActiveCommitDiffStatus::Error(error.clone())
            }
            Some(CommitDiffCacheEntry::Loading) | None => ActiveCommitDiffStatus::Loading,
        }
    }

    pub fn active_effective_semantic_detail(&self) -> Option<PullRequestDetail> {
        let detail = self.active_detail()?.clone();
        match self.active_commit_diff_status() {
            ActiveCommitDiffStatus::Loaded(filtered) => Some(filtered),
            _ => Some(detail),
        }
    }

    pub fn select_active_commit_filter(&mut self, filter: CommitDiffFilter) {
        let Some(detail) = self.active_detail().cloned() else {
            return;
        };
        let context = self.active_commit_timeline_context(&detail);

        if let Some(detail_state) = self.active_detail_state_mut() {
            detail_state
                .commit_diff_state
                .select_filter(&detail, &context, filter.clone());
        }
        self.close_commit_filter_review_entry_points();
        self.apply_active_commit_filter_selected_file();
    }

    pub fn set_active_commit_preview_filter(&mut self, filter: Option<CommitDiffFilter>) {
        let Some(detail) = self.active_detail().cloned() else {
            return;
        };
        let context = self.active_commit_timeline_context(&detail);
        if let Some(filter) = filter.as_ref() {
            if !timeline_items(&detail, &context)
                .iter()
                .any(|item| &item.filter == filter)
            {
                return;
            }
        }
        if let Some(detail_state) = self.active_detail_state_mut() {
            detail_state.commit_diff_state.preview_filter = filter;
        }
    }

    pub fn move_active_commit_filter(&mut self, delta: isize) {
        let Some(detail) = self.active_detail().cloned() else {
            return;
        };
        if self.effective_review_center_mode() != ReviewCenterMode::SemanticDiff {
            return;
        }

        let context = self.active_commit_timeline_context(&detail);
        let items = timeline_items(&detail, &context);
        if items.is_empty() {
            return;
        }
        let selected = self.active_commit_filter();
        let current_index = items
            .iter()
            .position(|item| item.filter == selected)
            .unwrap_or(0);
        let next_index = (current_index as isize + delta)
            .clamp(0, items.len().saturating_sub(1) as isize) as usize;
        if next_index != current_index {
            self.select_active_commit_filter(items[next_index].filter.clone());
        }
    }

    pub fn reset_active_commit_filter(&mut self) {
        self.select_active_commit_filter(CommitDiffFilter::All);
    }

    pub fn next_active_commit_diff_fetch_key(&mut self) -> Option<CommitDiffKey> {
        let detail = self.active_detail()?.clone();
        let context = self.active_commit_timeline_context(&detail);
        self.active_detail_state_mut()?
            .commit_diff_state
            .next_fetch_key(&detail, &context)
    }

    pub fn complete_active_commit_diff_fetch(
        &mut self,
        key: CommitDiffKey,
        result: Result<CommitDiffDataset, String>,
    ) {
        let detail_key = key.detail_key.clone();
        if let Some(detail_state) = self.detail_states.get_mut(&detail_key) {
            detail_state.commit_diff_state.complete_fetch(key, result);
        }
        if self.active_pr_key.as_deref() == Some(detail_key.as_str()) {
            self.apply_active_commit_filter_selected_file();
        }
    }

    fn apply_active_commit_filter_selected_file(&mut self) {
        let Some(detail) = self.active_effective_semantic_detail() else {
            return;
        };
        let next_path = if self.active_commit_filter().is_all() {
            Self::select_changed_file_path_for_detail(&detail, self.selected_file_path.clone())
        } else {
            selected_file_for_commit_detail(&detail, self.selected_file_path.clone())
        };
        if self.selected_file_path != next_path {
            self.selected_file_path = next_path;
            self.selected_diff_anchor = None;
        }
    }

    fn close_commit_filter_review_entry_points(&mut self) {
        self.selected_diff_anchor = None;
        self.review_finish_modal_open = false;
        self.review_editor_active = false;
        self.review_message = None;
        self.review_success = false;
        self.active_review_line_action = None;
        self.active_review_line_action_position = None;
        self.active_review_line_drag_origin = None;
        self.active_review_line_drag_current = None;
        self.inline_comment_draft.clear();
        self.inline_comment_error = None;
    }
}
