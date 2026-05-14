use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::cache::CacheStore;
use crate::code_tour::{
    self, build_tour_request_key, review_thread_anchor, CodeTourProvider, CodeTourProviderStatus,
    CodeTourSettings, DiffAnchor, GeneratedCodeTour,
};
use crate::diff::{build_diff_render_rows, find_parsed_diff_file, DiffRenderRow, ParsedDiffFile};
use crate::difftastic::AdaptedDifftasticDiffFile;
use crate::github::{
    PullRequestDetail, PullRequestDetailSnapshot, PullRequestQueue, PullRequestReviewThread,
    PullRequestSummary, RepositoryFileContent, ReviewAction, WorkspaceSnapshot,
};
use crate::local_repo::LocalRepositoryStatus;
use crate::local_review::{self, RememberedLocalRepository};
use crate::lsp::{LspServerStatus, LspSessionManager, LspSymbolDetails};
use crate::managed_lsp::{ManagedServerInstallStatus, ManagedServerKind};
use crate::notifications;
use crate::review_brief::ReviewBrief;
use crate::review_queue::{default_review_file, ReviewQueue};
use crate::review_session::{
    add_waymark, load_review_session, location_label, push_history_location, push_route_location,
    remove_waymark, sanitize_code_lens_mode, save_review_session, DiffLayout, ReviewCenterMode,
    ReviewLocation, ReviewSessionDocument, ReviewSessionState, ReviewSourceTarget, ReviewTaskRoute,
    ReviewWaymark,
};
use crate::semantic_diff::SemanticDiffFile;
use crate::shader_surface::{
    load_project_shader_settings, save_project_shader_settings, OverviewShaderVariant,
    ProjectShaderSettings,
};
use crate::stacks::model::{ReviewStack, StackDiffMode, StackPullRequestRef};
use crate::syntax::{self, SyntaxSpan};
use crate::theme::{self, CodeFontSizePreference, DiffColorThemePreference, ThemePreference};
use gpui::{
    px, AnyWindowHandle, ListAlignment, ListState, Pixels, Point, ScrollHandle, WindowAppearance,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SectionId {
    Overview,
    Pulls,
    Issues,
    Reviews,
    Settings,
}

impl SectionId {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Pulls => "Pull Requests",
            Self::Issues => "Issues",
            Self::Reviews => "Reviews",
            Self::Settings => "Settings",
        }
    }

    pub fn all() -> &'static [SectionId] {
        &[
            SectionId::Overview,
            SectionId::Pulls,
            SectionId::Issues,
            SectionId::Reviews,
            SectionId::Settings,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PullRequestSurface {
    Overview,
    Files,
}

impl PullRequestSurface {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overview => "Briefing",
            Self::Files => "Review",
        }
    }

    pub fn all() -> &'static [PullRequestSurface] {
        &[PullRequestSurface::Overview, PullRequestSurface::Files]
    }
}

pub fn pr_key(repository: &str, number: i64) -> String {
    format!("{repository}#{number}")
}

pub fn summary_key(summary: &PullRequestSummary) -> String {
    summary
        .local_key
        .clone()
        .unwrap_or_else(|| pr_key(&summary.repository, summary.number))
}

#[derive(Clone, Debug)]
pub struct DetailState {
    pub snapshot: Option<PullRequestDetailSnapshot>,
    pub loading: bool,
    pub syncing: bool,
    pub error: Option<String>,
    pub local_repository_status: Option<LocalRepositoryStatus>,
    pub local_repository_loading: bool,
    pub local_repository_error: Option<String>,
    pub source_file_tree: SourceFileTreeState,
    pub review_intelligence_request_key: Option<String>,
    pub review_intelligence_loading: bool,
    pub review_brief_state: ReviewBriefState,
    pub ai_stack_state: AiStackState,
    pub tour_states: std::collections::HashMap<CodeTourProvider, CodeTourState>,
    pub file_content_states: std::collections::HashMap<String, FileContentState>,
    pub structural_diff_states: std::collections::HashMap<String, StructuralDiffFileState>,
    pub structural_diff_warmup: StructuralDiffWarmupState,
    pub lsp_statuses: std::collections::HashMap<String, LspServerStatus>,
    pub lsp_loading_paths: std::collections::HashSet<String>,
    pub lsp_symbol_states: std::collections::HashMap<String, LspSymbolState>,
    pub review_route_loading: bool,
    pub review_route_message: Option<String>,
    pub review_route_error: Option<String>,
    pub review_session: ReviewSessionState,
    pub stack_open_pull_requests: Option<Vec<StackPullRequestRef>>,
    pub stack_open_pull_requests_loading: bool,
    pub stack_open_pull_requests_error: Option<String>,
}

impl Default for DetailState {
    fn default() -> Self {
        Self {
            snapshot: None,
            loading: false,
            syncing: false,
            error: None,
            local_repository_status: None,
            local_repository_loading: false,
            local_repository_error: None,
            source_file_tree: SourceFileTreeState::default(),
            review_intelligence_request_key: None,
            review_intelligence_loading: false,
            review_brief_state: ReviewBriefState::default(),
            ai_stack_state: AiStackState::default(),
            tour_states: std::collections::HashMap::new(),
            file_content_states: std::collections::HashMap::new(),
            structural_diff_states: std::collections::HashMap::new(),
            structural_diff_warmup: StructuralDiffWarmupState::default(),
            lsp_statuses: std::collections::HashMap::new(),
            lsp_loading_paths: std::collections::HashSet::new(),
            lsp_symbol_states: std::collections::HashMap::new(),
            review_route_loading: false,
            review_route_message: None,
            review_route_error: None,
            review_session: ReviewSessionState::default(),
            stack_open_pull_requests: None,
            stack_open_pull_requests_loading: false,
            stack_open_pull_requests_error: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceFileTreeState {
    pub request_key: Option<String>,
    pub rows: Option<Arc<Vec<ReviewFileTreeRow>>>,
    pub file_count: usize,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ReviewBriefState {
    pub request_key: Option<String>,
    pub document: Option<ReviewBrief>,
    pub loading: bool,
    pub generating: bool,
    pub progress_text: Option<String>,
    pub error: Option<String>,
    pub message: Option<String>,
    pub success: bool,
}

impl Default for ReviewBriefState {
    fn default() -> Self {
        Self {
            request_key: None,
            document: None,
            loading: false,
            generating: false,
            progress_text: None,
            error: None,
            message: None,
            success: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AiStackState {
    pub request_key: Option<String>,
    pub stack: Option<Arc<ReviewStack>>,
    pub loading: bool,
    pub generating: bool,
    pub error: Option<String>,
    pub message: Option<String>,
    pub success: bool,
}

impl Default for AiStackState {
    fn default() -> Self {
        Self {
            request_key: None,
            stack: None,
            loading: false,
            generating: false,
            error: None,
            message: None,
            success: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CodeTourState {
    pub request_key: Option<String>,
    pub document: Option<GeneratedCodeTour>,
    pub loading: bool,
    pub generating: bool,
    pub progress_summary: Option<String>,
    pub progress_detail: Option<String>,
    pub progress_log: Vec<String>,
    pub progress_log_file_path: Option<String>,
    pub error: Option<String>,
    pub message: Option<String>,
    pub success: bool,
}

impl Default for CodeTourState {
    fn default() -> Self {
        Self {
            request_key: None,
            document: None,
            loading: false,
            generating: false,
            progress_summary: None,
            progress_detail: None,
            progress_log: Vec::new(),
            progress_log_file_path: None,
            error: None,
            message: None,
            success: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileContentState {
    pub request_key: Option<String>,
    pub document: Option<RepositoryFileContent>,
    pub prepared: Option<PreparedFileContent>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Default for FileContentState {
    fn default() -> Self {
        Self {
            request_key: None,
            document: None,
            prepared: None,
            loading: false,
            error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempSourceSide {
    Base,
    Head,
}

impl TempSourceSide {
    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Head => "head",
        }
    }

    pub fn diff_side(self) -> &'static str {
        match self {
            Self::Base => "LEFT",
            Self::Head => "RIGHT",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempSourceTarget {
    pub path: String,
    pub side: TempSourceSide,
    pub line: usize,
    pub reference: String,
}

impl TempSourceTarget {
    pub fn content_key(&self) -> String {
        format!("{}:{}:{}", self.side.label(), self.reference, self.path)
    }

    pub fn focus_key(&self) -> String {
        format!("{}:{}", self.content_key(), self.line)
    }
}

#[derive(Clone)]
pub struct TempSourceWindowState {
    pub target: Option<TempSourceTarget>,
    pub request_key: Option<String>,
    pub document: Option<RepositoryFileContent>,
    pub prepared: Option<PreparedFileContent>,
    pub loading: bool,
    pub error: Option<String>,
    pub window: Option<AnyWindowHandle>,
}

impl Default for TempSourceWindowState {
    fn default() -> Self {
        Self {
            target: None,
            request_key: None,
            document: None,
            prepared: None,
            loading: false,
            error: None,
            window: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StructuralDiffFileState {
    pub request_key: Option<String>,
    pub diff: Option<Arc<AdaptedDifftasticDiffFile>>,
    pub loading: bool,
    pub error: Option<String>,
    pub terminal_error: bool,
}

#[derive(Clone, Debug, Default)]
pub struct StructuralDiffWarmupState {
    pub request_key: Option<String>,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub loading: bool,
}

impl StructuralDiffWarmupState {
    pub fn status_text(&self) -> Option<String> {
        if self.total == 0 {
            return self
                .loading
                .then(|| "Preparing structural diffs".to_string());
        }

        if !self.loading && self.completed == 0 && self.failed == 0 {
            return None;
        }

        let processed = self.completed + self.failed;
        if !self.loading && self.failed == 0 && processed >= self.total {
            return None;
        }

        let mut text = if self.loading {
            format!(
                "Preparing structural diffs {}/{}",
                processed.min(self.total),
                self.total
            )
        } else {
            format!(
                "Structural diffs {}/{} prepared",
                self.completed.min(self.total),
                self.total
            )
        };
        if self.failed > 0 {
            text.push_str(&format!(", {} unavailable", self.failed));
        }
        if self.loading {
            if processed < self.total {
                text.push_str(&format!(", {} queued", self.total - processed));
            }
        }

        Some(text)
    }
}

#[derive(Clone, Debug, Default)]
pub struct LspSymbolState {
    pub loading: bool,
    pub details: Option<LspSymbolDetails>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ManagedLspSettingsState {
    pub statuses: std::collections::HashMap<ManagedServerKind, ManagedServerInstallStatus>,
    pub loading: bool,
    pub loaded: bool,
    pub installing: std::collections::HashSet<ManagedServerKind>,
    pub install_errors: std::collections::HashMap<ManagedServerKind, String>,
    pub install_messages: std::collections::HashMap<ManagedServerKind, String>,
}

#[derive(Clone, Debug)]
pub struct CodeTourSettingsState {
    pub settings: CodeTourSettings,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub background_syncing: bool,
    pub background_message: Option<String>,
    pub background_error: Option<String>,
}

impl Default for CodeTourSettingsState {
    fn default() -> Self {
        Self {
            settings: CodeTourSettings::default(),
            loading: false,
            loaded: false,
            error: None,
            background_syncing: false,
            background_message: None,
            background_error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedFileContent {
    pub path: String,
    pub reference: String,
    pub is_binary: bool,
    pub size_bytes: usize,
    pub text: Arc<str>,
    pub lines: Arc<Vec<PreparedFileLine>>,
}

impl PreparedFileContent {
    pub fn rehighlighted(&self) -> Self {
        let text_lines = if self.text.is_empty() {
            Vec::new()
        } else {
            self.text
                .lines()
                .map(str::to_string)
                .collect::<Vec<String>>()
        };
        let spans = if self.is_binary || self.size_bytes > syntax::MAX_HIGHLIGHT_BYTES {
            text_lines
                .iter()
                .map(|_| Vec::new())
                .collect::<Vec<Vec<SyntaxSpan>>>()
        } else {
            syntax::highlight_lines(
                self.path.as_str(),
                text_lines.iter().map(|line| line.as_str()),
            )
        };

        let lines = text_lines
            .into_iter()
            .zip(spans)
            .enumerate()
            .map(|(index, (text, spans))| PreparedFileLine {
                line_number: index + 1,
                text,
                spans,
            })
            .collect::<Vec<_>>();

        Self {
            lines: Arc::new(lines),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedFileLine {
    pub line_number: usize,
    pub text: String,
    pub spans: Vec<SyntaxSpan>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffInlineRange {
    pub column_start: usize,
    pub column_end: usize,
}

// Keep review diffs line-oriented. Syntax highlighting remains active, but
// changed-token background patches are disabled for GitHub-style scanability.
pub const DIFF_INLINE_EMPHASIS_ENABLED: bool = false;

#[derive(Clone, Debug, Default)]
pub struct DiffLineHighlight {
    pub syntax_spans: Vec<SyntaxSpan>,
    pub emphasis_ranges: Vec<DiffInlineRange>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReviewLineActionMode {
    #[default]
    Menu,
    Comment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewLineActionTarget {
    pub anchor: DiffAnchor,
    pub start_line: Option<i64>,
    pub start_side: Option<String>,
    pub label: String,
}

impl ReviewLineActionTarget {
    pub fn stable_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.anchor.file_path,
            self.anchor.side.as_deref().unwrap_or(""),
            self.anchor.line.unwrap_or_default(),
            self.start_line.unwrap_or_default()
        )
    }

    pub fn review_location(&self) -> ReviewLocation {
        ReviewLocation::from_diff(self.anchor.file_path.clone(), Some(self.anchor.clone()))
    }
}

#[derive(Clone)]
pub struct DiffFileViewState {
    pub rows: Arc<Vec<DiffRenderRow>>,
    pub revision: String,
    pub parsed_file_index: Option<usize>,
    pub highlighted_hunks: Option<Arc<Vec<Vec<DiffLineHighlight>>>>,
    pub list_state: ListState,
    pub side_by_side_left_scroll: ScrollHandle,
    pub side_by_side_right_scroll: ScrollHandle,
    pub last_focus_key: Rc<RefCell<Option<String>>>,
}

impl DiffFileViewState {
    pub fn new(
        rows: Arc<Vec<DiffRenderRow>>,
        revision: String,
        parsed_file_index: Option<usize>,
        highlighted_hunks: Option<Arc<Vec<Vec<DiffLineHighlight>>>>,
    ) -> Self {
        Self {
            rows,
            revision,
            parsed_file_index,
            highlighted_hunks,
            list_state: ListState::new(0, ListAlignment::Top, px(400.0)).measure_all(),
            side_by_side_left_scroll: ScrollHandle::new(),
            side_by_side_right_scroll: ScrollHandle::new(),
            last_focus_key: Rc::new(RefCell::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct SourceBrowserViewState {
    pub list_state: ListState,
    pub last_focus_key: Rc<RefCell<Option<String>>>,
}

impl SourceBrowserViewState {
    pub fn new() -> Self {
        Self {
            list_state: ListState::new(0, ListAlignment::Top, px(400.0)),
            last_focus_key: Rc::new(RefCell::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct CombinedDiffViewState {
    pub list_state: ListState,
    pub side_by_side_left_scroll: ScrollHandle,
    pub side_by_side_right_scroll: ScrollHandle,
    pub last_focus_key: Rc<RefCell<Option<String>>>,
}

impl CombinedDiffViewState {
    pub fn new() -> Self {
        Self {
            list_state: ListState::new(0, ListAlignment::Top, px(400.0)).measure_all(),
            side_by_side_left_scroll: ScrollHandle::new(),
            side_by_side_right_scroll: ScrollHandle::new(),
            last_focus_key: Rc::new(RefCell::new(None)),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ReviewModeFocus {
    mode: ReviewCenterMode,
    file_path: String,
    line: Option<usize>,
    side: Option<String>,
    anchor: Option<DiffAnchor>,
}

#[derive(Clone)]
struct ReviewCommentNavigationItem {
    thread_index: usize,
    file_index: usize,
    row_index: usize,
    location: ReviewLocation,
}

fn diff_anchor_for_line(
    parsed: &ParsedDiffFile,
    line_number: i64,
    preferred_side: Option<&str>,
) -> Option<DiffAnchor> {
    let preferred_side = preferred_side.filter(|side| *side == "LEFT" || *side == "RIGHT");
    let sides = match preferred_side {
        Some("LEFT") => ["LEFT", "RIGHT"],
        _ => ["RIGHT", "LEFT"],
    };

    for side in sides {
        for hunk in &parsed.hunks {
            if hunk.lines.iter().any(|line| match side {
                "LEFT" => line.left_line_number == Some(line_number),
                "RIGHT" => line.right_line_number == Some(line_number),
                _ => false,
            }) {
                return Some(DiffAnchor {
                    file_path: parsed.path.clone(),
                    hunk_header: Some(hunk.header.clone()),
                    line: Some(line_number),
                    side: Some(side.to_string()),
                    thread_id: None,
                });
            }
        }
    }

    None
}

fn review_comment_navigation_items(
    detail: &PullRequestDetail,
    mode: ReviewCenterMode,
) -> Vec<ReviewCommentNavigationItem> {
    let mut seen_threads = HashSet::<usize>::new();
    let mut items = Vec::new();

    for (file_index, file) in detail.files.iter().enumerate() {
        for (row_index, row) in build_diff_render_rows(detail, &file.path)
            .into_iter()
            .enumerate()
        {
            let thread_index = match row {
                DiffRenderRow::FileCommentThread { thread_index }
                | DiffRenderRow::InlineThread { thread_index }
                | DiffRenderRow::OutdatedThread { thread_index } => thread_index,
                _ => continue,
            };
            if seen_threads.insert(thread_index) {
                push_review_comment_navigation_item(
                    &mut items,
                    detail,
                    thread_index,
                    mode,
                    file_index,
                    row_index,
                );
            }
        }
    }

    for thread_index in 0..detail.review_threads.len() {
        if seen_threads.insert(thread_index) {
            let file_index = detail
                .review_threads
                .get(thread_index)
                .and_then(|thread| review_thread_file_index(detail, thread))
                .unwrap_or(detail.files.len());
            push_review_comment_navigation_item(
                &mut items,
                detail,
                thread_index,
                mode,
                file_index,
                usize::MAX,
            );
        }
    }

    items
}

fn push_review_comment_navigation_item(
    items: &mut Vec<ReviewCommentNavigationItem>,
    detail: &PullRequestDetail,
    thread_index: usize,
    mode: ReviewCenterMode,
    file_index: usize,
    row_index: usize,
) {
    let Some(thread) = detail.review_threads.get(thread_index) else {
        return;
    };
    if thread.comments.is_empty() {
        return;
    }
    let Some(anchor) = review_thread_anchor(thread) else {
        return;
    };

    let file_path = if anchor.file_path.is_empty() {
        thread.path.clone()
    } else {
        anchor.file_path.clone()
    };
    let location = review_thread_location(mode, file_path.clone(), anchor);

    items.push(ReviewCommentNavigationItem {
        thread_index,
        file_index,
        row_index,
        location,
    });
}

fn review_thread_location(
    mode: ReviewCenterMode,
    file_path: String,
    anchor: DiffAnchor,
) -> ReviewLocation {
    match mode {
        ReviewCenterMode::StructuralDiff => {
            ReviewLocation::from_structural_diff(file_path, Some(anchor))
        }
        _ => ReviewLocation::from_diff(file_path, Some(anchor)),
    }
}

fn review_thread_file_index(
    detail: &PullRequestDetail,
    thread: &PullRequestReviewThread,
) -> Option<usize> {
    detail
        .files
        .iter()
        .position(|file| file.path == thread.path)
}

fn first_review_comment_after_focus_index(
    detail: &PullRequestDetail,
    items: &[ReviewCommentNavigationItem],
    focus: &ReviewModeFocus,
) -> Option<usize> {
    let (focus_file_index, focus_row_index) = review_focus_position(detail, focus)?;
    items.iter().position(|item| {
        item.file_index > focus_file_index
            || (item.file_index == focus_file_index && item.row_index >= focus_row_index)
    })
}

fn review_focus_position(
    detail: &PullRequestDetail,
    focus: &ReviewModeFocus,
) -> Option<(usize, usize)> {
    let file_index = detail
        .files
        .iter()
        .position(|file| file.path == focus.file_path)?;
    let rows = build_diff_render_rows(detail, &focus.file_path);
    let row_index = focus
        .anchor
        .as_ref()
        .and_then(|anchor| review_focus_row_index(detail, &focus.file_path, &rows, anchor))
        .or_else(|| {
            review_focus_line_row_index(
                detail,
                &focus.file_path,
                &rows,
                focus.line,
                focus.side.as_deref(),
            )
        })
        .unwrap_or(0);

    Some((file_index, row_index))
}

fn review_focus_row_index(
    detail: &PullRequestDetail,
    file_path: &str,
    rows: &[DiffRenderRow],
    anchor: &DiffAnchor,
) -> Option<usize> {
    if let Some(thread_id) = anchor.thread_id.as_deref() {
        if let Some(row_index) = rows.iter().position(|row| match row {
            DiffRenderRow::FileCommentThread { thread_index }
            | DiffRenderRow::InlineThread { thread_index }
            | DiffRenderRow::OutdatedThread { thread_index } => detail
                .review_threads
                .get(*thread_index)
                .map(|thread| thread.id == thread_id)
                .unwrap_or(false),
            _ => false,
        }) {
            return Some(row_index);
        }
    }

    review_focus_line_row_index(
        detail,
        file_path,
        rows,
        anchor
            .line
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0),
        anchor.side.as_deref(),
    )
}

fn review_focus_line_row_index(
    detail: &PullRequestDetail,
    file_path: &str,
    rows: &[DiffRenderRow],
    line: Option<usize>,
    preferred_side: Option<&str>,
) -> Option<usize> {
    let line = i64::try_from(line?).ok()?;
    let parsed = find_parsed_diff_file(&detail.parsed_diff, file_path)?;
    let anchor = diff_anchor_for_line(parsed, line, preferred_side)?;

    rows.iter().position(|row| match row {
        DiffRenderRow::Line {
            hunk_index,
            line_index,
        } => parsed
            .hunks
            .get(*hunk_index)
            .and_then(|hunk| hunk.lines.get(*line_index))
            .map(|line| code_tour::line_matches_diff_anchor(line, Some(&anchor)))
            .unwrap_or(false),
        _ => false,
    })
}

#[derive(Clone)]
pub struct CachedReviewQueue {
    pub revision: String,
    pub queue: Arc<ReviewQueue>,
}

#[derive(Clone)]
pub struct CachedSemanticDiffFile {
    pub revision: String,
    pub semantic: Arc<SemanticDiffFile>,
}

#[derive(Clone, Debug)]
pub enum ReviewFileTreeRow {
    Directory {
        name: String,
        depth: usize,
    },
    File {
        path: String,
        name: String,
        depth: usize,
        additions: i64,
        deletions: i64,
    },
}

#[derive(Clone)]
pub struct CachedReviewFileTree {
    pub revision: String,
    pub rows: Arc<Vec<ReviewFileTreeRow>>,
}

#[derive(Clone)]
pub struct CachedReviewStack {
    pub revision: String,
    pub open_pr_revision: usize,
    pub stack: Arc<ReviewStack>,
}

#[derive(Clone, Debug)]
pub struct ProjectShaderPickerState {
    pub project: String,
    pub label: String,
}

pub struct AppState {
    pub cache: Arc<CacheStore>,
    pub lsp_session_manager: Arc<LspSessionManager>,

    // Navigation
    pub active_section: SectionId,
    pub active_surface: PullRequestSurface,
    pub active_queue_id: String,
    pub active_pr_key: Option<String>,
    pub open_tabs: Vec<PullRequestSummary>,
    pub local_review_repositories: Vec<RememberedLocalRepository>,
    pub local_review_loading: bool,
    pub local_review_error: Option<String>,
    pub muted_repos: std::collections::HashSet<String>,
    pub project_shader_settings: ProjectShaderSettings,
    pub project_shader_settings_error: Option<String>,
    pub project_shader_picker: Option<ProjectShaderPickerState>,

    // Workspace data
    pub workspace: Option<WorkspaceSnapshot>,
    pub workspace_loading: bool,
    pub workspace_syncing: bool,
    pub workspace_error: Option<String>,

    // PR detail data (keyed by pr_key)
    pub detail_states: std::collections::HashMap<String, DetailState>,
    pub unread_review_comment_ids: std::collections::BTreeSet<String>,
    pub expanded_automation_activity_keys: std::collections::BTreeSet<String>,

    // Bootstrap
    pub gh_available: bool,
    pub gh_version: Option<String>,
    pub cache_path: String,
    pub bootstrap_loading: bool,
    pub theme_preference: ThemePreference,
    pub code_font_size_preference: CodeFontSizePreference,
    pub diff_color_theme_preference: DiffColorThemePreference,
    pub window_appearance: WindowAppearance,
    pub app_sidebar_collapsed: bool,
    pub notification_drawer_open: bool,
    pub software_update_message: Option<String>,
    pub software_update_error: Option<String>,

    // Selected file in diff view
    pub selected_file_path: Option<String>,
    pub selected_diff_anchor: Option<DiffAnchor>,
    review_scroll_focus: Option<ReviewModeFocus>,
    pub diff_view_states: RefCell<std::collections::HashMap<String, DiffFileViewState>>,
    pub review_queue_cache: RefCell<std::collections::HashMap<String, CachedReviewQueue>>,
    pub semantic_diff_cache: RefCell<std::collections::HashMap<String, CachedSemanticDiffFile>>,
    pub review_file_tree_cache: RefCell<std::collections::HashMap<String, CachedReviewFileTree>>,
    pub review_stack_cache: RefCell<std::collections::HashMap<String, CachedReviewStack>>,
    pub review_file_tree_list_states: RefCell<std::collections::HashMap<String, ListState>>,
    pub review_nav_list_states: RefCell<std::collections::HashMap<String, ListState>>,
    pub combined_diff_view_states:
        RefCell<std::collections::HashMap<String, CombinedDiffViewState>>,
    pub source_browser_list_states:
        RefCell<std::collections::HashMap<String, SourceBrowserViewState>>,
    pub temp_source_window: TempSourceWindowState,
    pub hovered_temp_source_target: Option<TempSourceTarget>,
    // Review form
    pub review_action: ReviewAction,
    pub review_body: String,
    pub review_editor_active: bool,
    pub review_editor_preview: bool,
    pub review_finish_modal_open: bool,
    pub review_loading: bool,
    pub review_message: Option<String>,
    pub review_success: bool,
    pub waymark_draft: String,
    pub active_review_line_action: Option<ReviewLineActionTarget>,
    pub active_review_line_action_position: Option<Point<Pixels>>,
    pub review_line_action_mode: ReviewLineActionMode,
    pub active_review_line_drag_origin: Option<ReviewLineActionTarget>,
    pub active_review_line_drag_current: Option<ReviewLineActionTarget>,
    pub inline_comment_draft: String,
    pub inline_comment_preview: bool,
    pub inline_comment_loading: bool,
    pub inline_comment_error: Option<String>,
    pub active_review_thread_reply_id: Option<String>,
    pub editing_review_comment_id: Option<String>,
    pub review_thread_action_loading_id: Option<String>,
    pub review_comment_action_loading_id: Option<String>,
    pub review_thread_action_error: Option<String>,
    pub pr_header_compact: bool,

    // Command palette
    pub palette_open: bool,
    pub palette_closing: bool,
    pub palette_close_generation: u64,
    pub palette_query: String,
    pub palette_selected_index: usize,
    pub palette_scroll_handle: ScrollHandle,
    pub palette_scroll_animation_generation: u64,
    pub palette_scroll_animation_active: bool,
    pub palette_last_scroll_navigation_at: Option<Instant>,
    pub palette_code_theme_expanded: bool,
    pub palette_code_theme_preview_original: Option<DiffColorThemePreference>,
    pub palette_code_theme_preview: Option<DiffColorThemePreference>,
    pub waypoint_spotlight_open: bool,
    pub waypoint_spotlight_query: String,
    pub waypoint_spotlight_selected_index: usize,

    // Code tours
    pub code_tour_provider_statuses: Vec<CodeTourProviderStatus>,
    pub code_tour_provider_statuses_loaded: bool,
    pub code_tour_provider_loading: bool,
    pub code_tour_provider_error: Option<String>,
    pub automatic_tour_request_keys: std::collections::HashSet<String>,
    pub automatic_brief_request_keys: std::collections::HashSet<String>,
    pub settings_scroll_handle: ScrollHandle,
    pub ai_tour_section_list_state: ListState,
    pub code_tour_settings: CodeTourSettingsState,
    pub managed_lsp_settings: ManagedLspSettingsState,
}

impl AppState {
    pub fn new(cache: CacheStore) -> Self {
        let theme_settings = theme::load_theme_settings(&cache).unwrap_or_default();
        let theme_preference = theme_settings.preference;
        let code_font_size_preference = theme_settings.code_font_size;
        let diff_color_theme_preference = theme_settings.diff_color_theme;
        theme::set_active_theme(theme::resolve_theme(
            theme_preference,
            WindowAppearance::Light,
        ));
        theme::set_active_code_font_size(code_font_size_preference);
        theme::set_active_diff_color_theme(diff_color_theme_preference);
        let cache_path = cache.path().display().to_string();
        let unread_review_comment_ids =
            notifications::load_unread_review_comment_ids(&cache).unwrap_or_default();
        let initial_code_tour_settings = match code_tour::load_code_tour_settings(&cache) {
            Ok(settings) => CodeTourSettingsState {
                settings,
                loaded: true,
                ..CodeTourSettingsState::default()
            },
            Err(error) => CodeTourSettingsState {
                error: Some(error),
                ..CodeTourSettingsState::default()
            },
        };
        let local_review_repositories =
            local_review::load_remembered_repositories(&cache).unwrap_or_default();
        let (project_shader_settings, project_shader_settings_error) =
            match load_project_shader_settings(&cache) {
                Ok(settings) => (settings, None),
                Err(error) => (ProjectShaderSettings::default(), Some(error)),
            };
        let mut state = Self {
            cache: Arc::new(cache),
            lsp_session_manager: Arc::new(LspSessionManager::new()),
            active_section: SectionId::Overview,
            active_surface: PullRequestSurface::Overview,
            active_queue_id: "reviewRequested".to_string(),
            active_pr_key: None,
            open_tabs: Vec::new(),
            local_review_repositories,
            local_review_loading: false,
            local_review_error: None,
            muted_repos: std::collections::HashSet::new(),
            project_shader_settings,
            project_shader_settings_error,
            project_shader_picker: None,
            workspace: None,
            workspace_loading: true,
            workspace_syncing: false,
            workspace_error: None,
            detail_states: std::collections::HashMap::new(),
            unread_review_comment_ids,
            expanded_automation_activity_keys: std::collections::BTreeSet::new(),
            gh_available: false,
            gh_version: None,
            cache_path,
            bootstrap_loading: true,
            theme_preference,
            code_font_size_preference,
            diff_color_theme_preference,
            window_appearance: WindowAppearance::Light,
            app_sidebar_collapsed: false,
            notification_drawer_open: false,
            software_update_message: None,
            software_update_error: None,
            selected_file_path: None,
            selected_diff_anchor: None,
            review_scroll_focus: None,
            diff_view_states: RefCell::new(std::collections::HashMap::new()),
            review_queue_cache: RefCell::new(std::collections::HashMap::new()),
            semantic_diff_cache: RefCell::new(std::collections::HashMap::new()),
            review_file_tree_cache: RefCell::new(std::collections::HashMap::new()),
            review_stack_cache: RefCell::new(std::collections::HashMap::new()),
            review_file_tree_list_states: RefCell::new(std::collections::HashMap::new()),
            review_nav_list_states: RefCell::new(std::collections::HashMap::new()),
            combined_diff_view_states: RefCell::new(std::collections::HashMap::new()),
            source_browser_list_states: RefCell::new(std::collections::HashMap::new()),
            temp_source_window: TempSourceWindowState::default(),
            hovered_temp_source_target: None,
            review_action: ReviewAction::Comment,
            review_body: String::new(),
            review_editor_active: false,
            review_editor_preview: false,
            review_finish_modal_open: false,
            review_loading: false,
            review_message: None,
            review_success: false,
            waymark_draft: String::new(),
            active_review_line_action: None,
            active_review_line_action_position: None,
            review_line_action_mode: ReviewLineActionMode::Menu,
            active_review_line_drag_origin: None,
            active_review_line_drag_current: None,
            inline_comment_draft: String::new(),
            inline_comment_preview: false,
            inline_comment_loading: false,
            inline_comment_error: None,
            active_review_thread_reply_id: None,
            editing_review_comment_id: None,
            review_thread_action_loading_id: None,
            review_comment_action_loading_id: None,
            review_thread_action_error: None,
            pr_header_compact: false,
            palette_open: false,
            palette_closing: false,
            palette_close_generation: 0,
            palette_query: String::new(),
            palette_selected_index: 0,
            palette_scroll_handle: ScrollHandle::new(),
            palette_scroll_animation_generation: 0,
            palette_scroll_animation_active: false,
            palette_last_scroll_navigation_at: None,
            palette_code_theme_expanded: false,
            palette_code_theme_preview_original: None,
            palette_code_theme_preview: None,
            waypoint_spotlight_open: false,
            waypoint_spotlight_query: String::new(),
            waypoint_spotlight_selected_index: 0,
            code_tour_provider_statuses: Vec::new(),
            code_tour_provider_statuses_loaded: false,
            code_tour_provider_loading: false,
            code_tour_provider_error: None,
            automatic_tour_request_keys: std::collections::HashSet::new(),
            automatic_brief_request_keys: std::collections::HashSet::new(),
            settings_scroll_handle: ScrollHandle::new(),
            ai_tour_section_list_state: ListState::new(0, ListAlignment::Top, px(720.0)),
            code_tour_settings: initial_code_tour_settings,
            managed_lsp_settings: ManagedLspSettingsState::default(),
        };

        state.restore_debug_pull_request_from_cache();
        state
    }

    pub fn resolved_theme(&self) -> theme::ActiveTheme {
        theme::resolve_theme(self.theme_preference, self.window_appearance)
    }

    pub fn set_active_section(&mut self, section: SectionId) {
        self.active_section = section;
    }

    pub fn shader_for_project(&self, project: &str) -> OverviewShaderVariant {
        self.project_shader_settings.shader_for_project(project)
    }

    pub fn open_project_shader_picker(&mut self, project: &str, label: &str) {
        self.project_shader_picker = Some(ProjectShaderPickerState {
            project: project.to_string(),
            label: label.to_string(),
        });
    }

    pub fn close_project_shader_picker(&mut self) {
        self.project_shader_picker = None;
    }

    pub fn set_project_shader(&mut self, project: &str, variant: OverviewShaderVariant) {
        self.project_shader_settings
            .set_project_shader(project, variant);
        match save_project_shader_settings(self.cache.as_ref(), &self.project_shader_settings) {
            Ok(()) => {
                self.project_shader_settings_error = None;
                self.project_shader_picker = None;
            }
            Err(error) => {
                self.project_shader_settings_error = Some(error);
            }
        }
    }

    pub fn set_theme_preference(&mut self, preference: ThemePreference) {
        let previous = self.resolved_theme();
        self.theme_preference = preference;
        self.apply_theme_change(previous);
    }

    pub fn set_code_font_size_preference(&mut self, preference: CodeFontSizePreference) {
        self.code_font_size_preference = preference;
        theme::set_active_code_font_size(preference);
    }

    pub fn set_diff_color_theme_preference(&mut self, preference: DiffColorThemePreference) {
        self.diff_color_theme_preference = preference;
        theme::set_active_diff_color_theme(preference);
    }

    pub fn set_window_appearance(&mut self, appearance: WindowAppearance) {
        let previous = self.resolved_theme();
        self.window_appearance = appearance;
        self.apply_theme_change(previous);
    }

    fn apply_theme_change(&mut self, previous: theme::ActiveTheme) {
        let next = self.resolved_theme();
        theme::set_active_theme(next);
        if next != previous {
            self.refresh_theme_dependent_state();
        }
    }

    fn refresh_theme_dependent_state(&mut self) {
        for detail_state in self.detail_states.values_mut() {
            for file_state in detail_state.file_content_states.values_mut() {
                if let Some(prepared) = file_state.prepared.as_ref() {
                    file_state.prepared = Some(prepared.rehighlighted());
                }
            }
        }

        for diff_view_state in self.diff_view_states.borrow_mut().values_mut() {
            diff_view_state.highlighted_hunks = None;
        }
    }

    pub fn active_queue(&self) -> Option<&PullRequestQueue> {
        self.workspace
            .as_ref()?
            .queues
            .iter()
            .find(|q| q.id == self.active_queue_id)
            .or_else(|| self.workspace.as_ref()?.queues.first())
    }

    pub fn active_pr(&self) -> Option<&PullRequestSummary> {
        let key = self.active_pr_key.as_ref()?;
        self.open_tabs.iter().find(|tab| summary_key(tab) == *key)
    }

    pub fn active_detail(&self) -> Option<&PullRequestDetail> {
        let key = self.active_pr_key.as_ref()?;
        self.detail_states
            .get(key)?
            .snapshot
            .as_ref()?
            .detail
            .as_ref()
    }

    pub fn active_is_local_review(&self) -> bool {
        self.active_pr_key
            .as_deref()
            .map(local_review::is_local_review_key)
            .unwrap_or(false)
    }

    pub fn default_changed_file_path(detail: &PullRequestDetail) -> Option<String> {
        default_review_file(detail)
            .filter(|path| Self::detail_has_changed_file(detail, path))
            .or_else(|| detail.files.first().map(|file| file.path.clone()))
    }

    pub fn select_changed_file_path_for_detail(
        detail: &PullRequestDetail,
        candidate: Option<String>,
    ) -> Option<String> {
        candidate
            .filter(|path| Self::detail_has_changed_file(detail, path))
            .or_else(|| Self::default_changed_file_path(detail))
    }

    pub fn ensure_active_selected_file_is_valid(&mut self) {
        let Some(detail) = self.active_detail().cloned() else {
            return;
        };

        let selected_file =
            Self::select_changed_file_path_for_detail(&detail, self.selected_file_path.clone());
        if self.selected_file_path != selected_file {
            self.selected_file_path = selected_file;
            self.selected_diff_anchor = None;
        }
    }

    fn detail_has_changed_file(detail: &PullRequestDetail, path: &str) -> bool {
        detail.files.iter().any(|file| file.path == path)
    }

    pub fn is_review_comment_unread(&self, comment_id: &str) -> bool {
        self.unread_review_comment_ids.contains(comment_id)
    }

    pub fn unread_review_comment_ids_for_detail(&self, detail: &PullRequestDetail) -> Vec<String> {
        let viewer_login = self.viewer_login().unwrap_or_default();
        detail
            .review_threads
            .iter()
            .flat_map(|thread| &thread.comments)
            .filter(|comment| viewer_login.is_empty() || comment.author_login != viewer_login)
            .filter(|comment| self.is_review_comment_unread(&comment.id))
            .map(|comment| comment.id.clone())
            .collect()
    }

    pub fn mark_review_comments_read<I>(&mut self, comment_ids: I)
    where
        I: IntoIterator<Item = String>,
    {
        let comment_ids = comment_ids.into_iter().collect::<Vec<_>>();
        if comment_ids.is_empty() {
            return;
        }

        match notifications::mark_review_comments_read(self.cache.as_ref(), comment_ids.clone()) {
            Ok(unread_ids) => {
                self.unread_review_comment_ids = unread_ids;
            }
            Err(error) => {
                eprintln!("Failed to persist review comment read state: {error}");
                for comment_id in comment_ids {
                    self.unread_review_comment_ids.remove(&comment_id);
                }
            }
        }
    }

    pub fn active_detail_state(&self) -> Option<&DetailState> {
        let key = self.active_pr_key.as_ref()?;
        self.detail_states.get(key)
    }

    pub fn active_tour_state(&self) -> Option<&CodeTourState> {
        let detail_state = self.active_detail_state()?;
        detail_state
            .tour_states
            .get(&self.code_tour_settings.settings.provider)
    }

    pub fn active_review_brief_state(&self) -> Option<&ReviewBriefState> {
        Some(&self.active_detail_state()?.review_brief_state)
    }

    pub fn active_review_session(&self) -> Option<&ReviewSessionState> {
        self.active_detail_state()
            .map(|detail_state| &detail_state.review_session)
    }

    pub fn active_review_session_mut(&mut self) -> Option<&mut ReviewSessionState> {
        let key = self.active_pr_key.clone()?;
        self.detail_states
            .get_mut(&key)
            .map(|detail_state| &mut detail_state.review_session)
    }

    pub fn active_local_repository_status(&self) -> Option<&LocalRepositoryStatus> {
        self.active_detail_state()?.local_repository_status.as_ref()
    }

    pub fn selected_tour_provider_status(&self) -> Option<&CodeTourProviderStatus> {
        self.code_tour_provider_statuses
            .iter()
            .find(|status| status.provider == self.code_tour_settings.settings.provider)
    }

    pub fn active_tour_request_key(&self) -> Option<String> {
        let detail = self.active_detail()?;
        Some(build_tour_request_key(
            detail,
            self.code_tour_settings.settings.provider,
        ))
    }

    pub fn selected_tour_provider(&self) -> CodeTourProvider {
        self.code_tour_settings.settings.provider
    }

    pub fn section_count(&self, section: SectionId) -> i64 {
        match section {
            SectionId::Overview => 0,
            SectionId::Pulls => self
                .workspace
                .as_ref()
                .map(|w| w.queues.iter().map(|q| q.total_count).sum())
                .unwrap_or(0),
            SectionId::Issues => 0,
            SectionId::Reviews => self
                .workspace
                .as_ref()
                .and_then(|w| w.queues.iter().find(|q| q.id == "reviewRequested"))
                .map(|q| q.total_count)
                .unwrap_or(0),
            SectionId::Settings => 0,
        }
    }

    pub fn viewer_name(&self) -> &str {
        self.workspace
            .as_ref()
            .and_then(|w| w.viewer.as_ref())
            .and_then(|v| v.name.as_deref().or(Some(v.login.as_str())))
            .unwrap_or("developer")
    }

    pub fn viewer_login(&self) -> Option<&str> {
        let workspace = self.workspace.as_ref()?;
        workspace
            .viewer
            .as_ref()
            .map(|viewer| viewer.login.as_str())
            .or(workspace.auth.active_login.as_deref())
    }

    pub fn is_authenticated(&self) -> bool {
        self.workspace
            .as_ref()
            .map(|w| w.auth.is_authenticated)
            .unwrap_or(false)
    }

    pub fn review_queue(&self) -> Option<&PullRequestQueue> {
        self.workspace
            .as_ref()?
            .queues
            .iter()
            .find(|q| q.id == "reviewRequested")
    }

    pub fn authored_queue(&self) -> Option<&PullRequestQueue> {
        self.workspace
            .as_ref()?
            .queues
            .iter()
            .find(|q| q.id == "authored")
    }

    pub fn current_review_location(&self) -> Option<ReviewLocation> {
        let session = self.active_review_session();
        if let Some(source_target) = session.and_then(|session| {
            (session.center_mode == ReviewCenterMode::SourceBrowser)
                .then(|| session.source_target.clone())
                .flatten()
        }) {
            return Some(ReviewLocation::from_source(
                source_target.path,
                source_target.line,
                source_target.reason,
            ));
        }

        self.selected_file_path.clone().map(|file_path| {
            match session.map(|session| session.center_mode) {
                Some(ReviewCenterMode::AiTour) => {
                    ReviewLocation::from_ai_tour(file_path, self.selected_diff_anchor.clone())
                }
                Some(ReviewCenterMode::StructuralDiff) => ReviewLocation::from_structural_diff(
                    file_path,
                    self.selected_diff_anchor.clone(),
                ),
                _ => ReviewLocation::from_diff(file_path, self.selected_diff_anchor.clone()),
            }
        })
    }

    pub fn selected_diff_line_target(&self) -> Option<ReviewLineActionTarget> {
        let file_path = self.selected_file_path.clone()?;
        let anchor = self.selected_diff_anchor.as_ref()?;
        let side = anchor.side.as_deref()?;
        let line = anchor.line?;
        let line_number = usize::try_from(line).ok().filter(|line| *line > 0)?;

        Some(ReviewLineActionTarget {
            anchor: DiffAnchor {
                file_path: file_path.clone(),
                hunk_header: anchor.hunk_header.clone(),
                line: Some(line),
                side: Some(side.to_string()),
                thread_id: None,
            },
            start_line: None,
            start_side: None,
            label: location_label(&file_path, Some(line_number)),
        })
    }

    pub fn active_review_task_route(&self) -> Option<&ReviewTaskRoute> {
        self.active_review_session()
            .and_then(|session| session.task_route.as_ref())
    }

    pub fn apply_review_session_document(
        &mut self,
        detail_key: &str,
        document: Option<ReviewSessionDocument>,
    ) {
        if let Some(document) = document {
            self.selected_file_path = document.selected_file_path.clone();
            self.selected_diff_anchor = document.selected_diff_anchor.clone();
            if let Some(detail_state) = self.detail_states.get_mut(detail_key) {
                detail_state.review_session = ReviewSessionState::from_document(document);
            }
        } else {
            self.selected_file_path = None;
            self.selected_diff_anchor = None;
            if let Some(detail_state) = self.detail_states.get_mut(detail_key) {
                detail_state.review_session.loaded = true;
                detail_state.review_session.error = None;
            }
        }
        self.ensure_active_selected_file_is_valid();
    }

    pub fn navigate_to_review_location(&mut self, location: ReviewLocation, push_history: bool) {
        let previous = if push_history {
            self.current_review_location()
        } else {
            None
        };
        let Some(detail_key) = self.active_pr_key.clone() else {
            return;
        };

        let path_is_changed = self
            .detail_states
            .get(&detail_key)
            .and_then(|detail_state| detail_state.snapshot.as_ref())
            .and_then(|snapshot| snapshot.detail.as_ref())
            .map(|detail| {
                detail
                    .files
                    .iter()
                    .any(|file| file.path == location.file_path)
            })
            .unwrap_or(false);

        match location.mode {
            ReviewCenterMode::SemanticDiff
            | ReviewCenterMode::StructuralDiff
            | ReviewCenterMode::AiTour
            | ReviewCenterMode::Stack => {
                self.selected_file_path = Some(location.file_path.clone());
                self.selected_diff_anchor = location.anchor.clone();
            }
            ReviewCenterMode::SourceBrowser => {
                if path_is_changed {
                    self.selected_file_path = Some(location.file_path.clone());
                }
            }
        }

        let Some(session) = self.active_review_session_mut() else {
            return;
        };

        if push_history {
            if let Some(previous) = previous.filter(|previous| previous != &location) {
                push_history_location(&mut session.history_back, previous);
                session.history_forward.clear();
            }
        }

        session.center_mode = location.mode;
        if matches!(
            location.mode,
            ReviewCenterMode::SemanticDiff
                | ReviewCenterMode::StructuralDiff
                | ReviewCenterMode::SourceBrowser
        ) {
            session.code_lens_mode = location.mode;
        }
        session.source_target = location.as_source_target();
        session.last_read = Some(location.clone());
        push_route_location(&mut session.route, location);
    }

    pub fn current_waymark(&self) -> Option<&ReviewWaymark> {
        self.active_review_session().and_then(|session| {
            self.selected_diff_line_target()
                .map(|target| target.review_location())
                .or_else(|| self.current_review_location())
                .and_then(|location| session.waymark_for_location(&location))
        })
    }

    pub fn add_waymark_for_current_review_location(
        &mut self,
        name: impl Into<String>,
    ) -> Option<ReviewWaymark> {
        let location = self
            .selected_diff_line_target()
            .map(|target| target.review_location())
            .or_else(|| self.current_review_location())?;
        let session = self.active_review_session_mut()?;
        Some(add_waymark(&mut session.waymarks, location, name))
    }

    pub fn remove_review_waymark(&mut self, waymark_id: &str) -> bool {
        let Some(session) = self.active_review_session_mut() else {
            return false;
        };

        remove_waymark(&mut session.waymarks, waymark_id)
    }

    pub fn navigate_review_back(&mut self) -> bool {
        let current = self.current_review_location();
        let target = {
            let Some(session) = self.active_review_session_mut() else {
                return false;
            };
            session.history_back.pop()
        };

        let Some(target) = target else {
            return false;
        };

        if let Some(current) = current {
            if let Some(session) = self.active_review_session_mut() {
                push_history_location(&mut session.history_forward, current);
            }
        }

        self.navigate_to_review_location(target, false);
        true
    }

    pub fn navigate_review_forward(&mut self) -> bool {
        let current = self.current_review_location();
        let target = {
            let Some(session) = self.active_review_session_mut() else {
                return false;
            };
            session.history_forward.pop()
        };

        let Some(target) = target else {
            return false;
        };

        if let Some(current) = current {
            if let Some(session) = self.active_review_session_mut() {
                push_history_location(&mut session.history_back, current);
            }
        }

        self.navigate_to_review_location(target, false);
        true
    }

    pub fn next_review_comment_location(&self) -> Option<ReviewLocation> {
        let mode = self.active_review_comment_navigation_mode()?;
        let detail = self.active_detail()?;
        let items = review_comment_navigation_items(detail, mode);
        if items.is_empty() {
            return None;
        }

        let selected_thread_id = self
            .selected_diff_anchor
            .as_ref()
            .and_then(|anchor| anchor.thread_id.as_deref());
        let next_index = selected_thread_id
            .and_then(|thread_id| {
                items
                    .iter()
                    .position(|item| {
                        detail
                            .review_threads
                            .get(item.thread_index)
                            .map(|thread| thread.id == thread_id)
                            .unwrap_or(false)
                    })
                    .map(|index| (index + 1) % items.len())
            })
            .or_else(|| {
                self.current_review_mode_focus()
                    .as_ref()
                    .and_then(|focus| first_review_comment_after_focus_index(detail, &items, focus))
            })
            .unwrap_or(0);

        items.get(next_index).map(|item| item.location.clone())
    }

    fn active_review_comment_navigation_mode(&self) -> Option<ReviewCenterMode> {
        self.active_review_session()
            .map(|session| session.center_mode)
            .filter(|mode| {
                matches!(
                    mode,
                    ReviewCenterMode::SemanticDiff | ReviewCenterMode::StructuralDiff
                )
            })
    }

    pub fn toggle_review_section_collapse(&mut self, section_id: &str) {
        let Some(session) = self.active_review_session_mut() else {
            return;
        };

        if !session.collapsed_sections.insert(section_id.to_string()) {
            session.collapsed_sections.remove(section_id);
        }
    }

    pub fn is_review_section_collapsed(&self, section_id: &str) -> bool {
        self.active_review_session()
            .map(|session| session.collapsed_sections.contains(section_id))
            .unwrap_or(false)
    }

    pub fn set_review_file_collapsed(&mut self, file_path: &str, collapsed: bool) {
        let Some(session) = self.active_review_session_mut() else {
            return;
        };

        if collapsed {
            session.collapsed_file_paths.insert(file_path.to_string());
        } else {
            session.collapsed_file_paths.remove(file_path);
        }
    }

    pub fn is_review_file_collapsed(&self, file_path: &str) -> bool {
        self.active_review_session()
            .map(|session| session.collapsed_file_paths.contains(file_path))
            .unwrap_or(false)
    }

    pub fn set_review_file_reviewed(
        &mut self,
        review_stack: &ReviewStack,
        file_path: &str,
        reviewed: bool,
    ) {
        let Some(session) = self.active_review_session_mut() else {
            return;
        };

        if reviewed {
            session.reviewed_file_paths.insert(file_path.to_string());
        } else {
            session.reviewed_file_paths.remove(file_path);
        }

        let affected_atom_ids = review_stack
            .atoms
            .iter()
            .filter(|atom| {
                atom.path == file_path || atom.previous_path.as_deref() == Some(file_path)
            })
            .map(|atom| atom.id.clone())
            .collect::<HashSet<_>>();

        for atom_id in &affected_atom_ids {
            if reviewed {
                session.reviewed_stack_atom_ids.insert(atom_id.clone());
            } else {
                session.reviewed_stack_atom_ids.remove(atom_id);
            }
        }

        for layer in &review_stack.layers {
            if !layer.atom_ids.is_empty()
                && layer
                    .atom_ids
                    .iter()
                    .all(|atom_id| session.reviewed_stack_atom_ids.contains(atom_id))
            {
                session.reviewed_stack_layer_ids.insert(layer.id.clone());
            } else if layer
                .atom_ids
                .iter()
                .any(|atom_id| affected_atom_ids.contains(atom_id))
            {
                session.reviewed_stack_layer_ids.remove(&layer.id);
            }
        }
    }

    pub fn is_review_file_reviewed(&self, file_path: &str) -> bool {
        self.active_review_session()
            .map(|session| session.reviewed_file_paths.contains(file_path))
            .unwrap_or(false)
    }

    fn current_review_mode_focus(&self) -> Option<ReviewModeFocus> {
        let session = self.active_review_session();
        if let Some(scroll_focus) = self.review_scroll_focus.as_ref().filter(|focus| {
            session
                .map(|session| session.center_mode == focus.mode)
                .unwrap_or(false)
        }) {
            return Some(scroll_focus.clone());
        }

        if let Some(source_target) = session.and_then(|session| {
            (session.center_mode == ReviewCenterMode::SourceBrowser)
                .then(|| session.source_target.clone())
                .flatten()
        }) {
            return Some(ReviewModeFocus {
                mode: ReviewCenterMode::SourceBrowser,
                file_path: source_target.path,
                line: source_target.line,
                side: Some("RIGHT".to_string()),
                anchor: None,
            });
        }

        let file_path = self
            .selected_diff_anchor
            .as_ref()
            .map(|anchor| anchor.file_path.clone())
            .or_else(|| self.selected_file_path.clone())?;
        let anchor = self.selected_diff_anchor.clone();
        let line = anchor
            .as_ref()
            .and_then(|anchor| anchor.line)
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0);
        let side = anchor.as_ref().and_then(|anchor| anchor.side.clone());

        Some(ReviewModeFocus {
            mode: session
                .map(|session| session.center_mode)
                .unwrap_or(ReviewCenterMode::SemanticDiff),
            file_path,
            line,
            side,
            anchor,
        })
    }

    fn changed_file_path_for_focus(&self, file_path: &str) -> Option<String> {
        self.active_detail().and_then(|detail| {
            detail
                .files
                .iter()
                .find(|file| file.path == file_path)
                .map(|file| file.path.clone())
                .or_else(|| {
                    find_parsed_diff_file(&detail.parsed_diff, file_path)
                        .map(|parsed| parsed.path.clone())
                })
        })
    }

    fn anchor_for_focus(&self, focus: &ReviewModeFocus) -> Option<DiffAnchor> {
        let line_number = i64::try_from(focus.line?).ok()?;
        let detail = self.active_detail()?;
        let parsed = find_parsed_diff_file(&detail.parsed_diff, &focus.file_path)?;
        diff_anchor_for_line(parsed, line_number, focus.side.as_deref())
    }

    pub fn set_review_center_mode_preserving_focus(&mut self, mode: ReviewCenterMode) {
        let focus = self.current_review_mode_focus();
        let target_anchor = match mode {
            ReviewCenterMode::SemanticDiff | ReviewCenterMode::StructuralDiff => {
                focus.as_ref().and_then(|focus| {
                    focus
                        .anchor
                        .clone()
                        .or_else(|| self.anchor_for_focus(focus))
                })
            }
            _ => None,
        };
        let target_file_path = focus
            .as_ref()
            .and_then(|focus| self.changed_file_path_for_focus(&focus.file_path))
            .or_else(|| self.selected_file_path.clone());
        let target_source = (mode == ReviewCenterMode::SourceBrowser)
            .then(|| {
                focus.as_ref().map(|focus| ReviewSourceTarget {
                    path: focus.file_path.clone(),
                    line: focus.line,
                    reason: Some("Current review focus".to_string()),
                })
            })
            .flatten();

        self.set_review_center_mode(mode);

        match mode {
            ReviewCenterMode::SemanticDiff | ReviewCenterMode::StructuralDiff => {
                if let Some(path) = target_file_path {
                    self.selected_file_path = Some(path);
                }
                if target_anchor.is_some()
                    || focus
                        .as_ref()
                        .map(|focus| focus.anchor.is_none())
                        .unwrap_or(false)
                {
                    self.selected_diff_anchor = target_anchor;
                }
            }
            ReviewCenterMode::SourceBrowser => {
                if let Some(target) = target_source {
                    self.selected_file_path = self
                        .changed_file_path_for_focus(&target.path)
                        .or_else(|| self.selected_file_path.clone());
                    if let Some(session) = self.active_review_session_mut() {
                        session.source_target = Some(target);
                    }
                }
            }
            ReviewCenterMode::AiTour | ReviewCenterMode::Stack => {}
        }

        self.reset_review_focus_scroll();
    }

    pub fn set_review_center_mode(&mut self, mode: ReviewCenterMode) {
        if let Some(session) = self.active_review_session_mut() {
            session.center_mode = mode;
            if matches!(
                mode,
                ReviewCenterMode::SemanticDiff
                    | ReviewCenterMode::StructuralDiff
                    | ReviewCenterMode::SourceBrowser
            ) {
                session.code_lens_mode = mode;
            }
            if mode != ReviewCenterMode::SourceBrowser {
                session.source_target = None;
            }
        }
    }

    pub fn reset_review_focus_scroll(&mut self) {
        self.review_scroll_focus = None;
        for view_state in self.diff_view_states.borrow().values() {
            *view_state.last_focus_key.borrow_mut() = None;
        }
        for view_state in self.source_browser_list_states.borrow().values() {
            *view_state.last_focus_key.borrow_mut() = None;
        }
        for view_state in self.combined_diff_view_states.borrow().values() {
            *view_state.last_focus_key.borrow_mut() = None;
        }
    }

    pub fn set_review_scroll_focus(
        &mut self,
        mode: ReviewCenterMode,
        file_path: impl Into<String>,
        line: Option<usize>,
        side: Option<String>,
        anchor: Option<DiffAnchor>,
    ) -> bool {
        let next = ReviewModeFocus {
            mode,
            file_path: file_path.into(),
            line,
            side,
            anchor,
        };
        if self.review_scroll_focus.as_ref() == Some(&next) {
            return false;
        }

        self.review_scroll_focus = Some(next);
        true
    }

    pub fn set_review_file_tree_visible(&mut self, visible: bool) {
        if let Some(session) = self.active_review_session_mut() {
            session.show_file_tree = visible;
        }
    }

    pub fn set_normal_diff_layout(&mut self, layout: DiffLayout) {
        if let Some(session) = self.active_review_session_mut() {
            session.normal_diff_layout = layout;
        }
    }

    pub fn set_structural_diff_layout(&mut self, layout: DiffLayout) {
        if let Some(session) = self.active_review_session_mut() {
            session.structural_diff_layout = layout;
        }
    }

    pub fn set_diff_line_wrap(&mut self, wrap: bool) {
        if let Some(session) = self.active_review_session_mut() {
            session.wrap_diff_lines = wrap;
        }
    }

    pub fn set_review_source_target(&mut self, target: ReviewSourceTarget) {
        if let Some(session) = self.active_review_session_mut() {
            session.center_mode = ReviewCenterMode::SourceBrowser;
            session.code_lens_mode = ReviewCenterMode::SourceBrowser;
            session.source_target = Some(target);
        }
    }

    pub fn active_code_lens_mode(&self) -> ReviewCenterMode {
        self.active_review_session()
            .map(|session| session.active_code_lens_mode())
            .unwrap_or(ReviewCenterMode::SemanticDiff)
    }

    pub fn enter_code_review_mode(&mut self) {
        let mode = sanitize_code_lens_mode(self.active_code_lens_mode());
        self.set_review_center_mode_preserving_focus(mode);
    }

    pub fn set_selected_stack_layer(&mut self, layer_id: Option<String>) {
        if let Some(session) = self.active_review_session_mut() {
            session.selected_stack_layer_id = layer_id;
        }
    }

    pub fn set_stack_diff_mode(&mut self, mode: StackDiffMode) {
        if let Some(session) = self.active_review_session_mut() {
            session.stack_diff_mode = mode;
        }
    }

    pub fn set_stack_rail_expanded(&mut self, expanded: bool) {
        if let Some(session) = self.active_review_session_mut() {
            session.stack_rail_expanded = expanded;
        }
    }

    pub fn set_stack_layer_reviewed(
        &mut self,
        stack: &ReviewStack,
        layer_id: &str,
        reviewed: bool,
    ) {
        let Some(session) = self.active_review_session_mut() else {
            return;
        };
        let Some(layer) = stack.layers.iter().find(|layer| layer.id == layer_id) else {
            return;
        };

        if reviewed {
            session.reviewed_stack_layer_ids.insert(layer.id.clone());
            for atom_id in &layer.atom_ids {
                session.reviewed_stack_atom_ids.insert(atom_id.clone());
            }
        } else {
            session.reviewed_stack_layer_ids.remove(&layer.id);
            for atom_id in &layer.atom_ids {
                session.reviewed_stack_atom_ids.remove(atom_id);
            }
        }
    }

    pub fn set_active_review_task_route(&mut self, route: Option<ReviewTaskRoute>) {
        if let Some(session) = self.active_review_session_mut() {
            session.task_route = route;
        }
    }

    pub fn persist_active_review_session(&self) {
        let Some(detail_key) = self.active_pr_key.as_deref() else {
            return;
        };
        let Some(session) = self.active_review_session() else {
            return;
        };

        let document = session.to_document(
            self.selected_file_path.as_deref(),
            self.selected_diff_anchor.as_ref(),
        );
        let _ = save_review_session(self.cache.as_ref(), detail_key, &document);
    }

    fn restore_debug_pull_request_from_cache(&mut self) {
        let Some((repository, number)) = std::env::var("REVIEWBUDDY_DEBUG_OPEN_PR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .and_then(|value| parse_debug_pull_request_target(&value))
        else {
            return;
        };

        let Ok(snapshot) =
            crate::github::load_pull_request_detail(self.cache.as_ref(), &repository, number)
        else {
            return;
        };
        let Some(detail) = snapshot.detail.clone() else {
            return;
        };

        let summary = PullRequestSummary {
            local_key: None,
            repository: detail.repository.clone(),
            number: detail.number,
            title: detail.title.clone(),
            author_login: detail.author_login.clone(),
            author_avatar_url: detail.author_avatar_url.clone(),
            is_draft: detail.is_draft,
            comments_count: detail.comments_count,
            additions: detail.additions,
            deletions: detail.deletions,
            changed_files: detail.changed_files,
            state: detail.state.clone(),
            review_decision: detail.review_decision.clone(),
            updated_at: detail.updated_at.clone(),
            url: detail.url.clone(),
        };
        let detail_key = pr_key(&repository, number);

        self.open_tabs.insert(0, summary);
        self.set_active_section(SectionId::Pulls);
        self.active_surface = PullRequestSurface::Overview;
        self.active_pr_key = Some(detail_key.clone());
        self.detail_states
            .entry(detail_key.clone())
            .or_default()
            .snapshot = Some(snapshot);

        if let Ok(document) = load_review_session(self.cache.as_ref(), &detail_key) {
            self.apply_review_session_document(&detail_key, document);
        }
    }
}

fn parse_debug_pull_request_target(target: &str) -> Option<(String, i64)> {
    let (repository, number) = target.trim().rsplit_once('#')?;
    let number = number.parse::<i64>().ok()?;
    Some((repository.to_string(), number))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine};
    use crate::github::{
        PullRequestComment, PullRequestDataCompleteness, PullRequestDetail, PullRequestFile,
        PullRequestReview, PullRequestReviewComment, PullRequestReviewThread,
    };
    use crate::review_session::ReviewCenterMode;

    use super::{
        diff_anchor_for_line, first_review_comment_after_focus_index,
        review_comment_navigation_items, ReviewModeFocus, StructuralDiffWarmupState,
    };

    #[test]
    fn diff_anchor_for_line_preserves_requested_side() {
        let parsed = ParsedDiffFile {
            path: "src/lib.rs".to_string(),
            previous_path: Some("src/lib.rs".to_string()),
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,2 +1,2 @@".to_string(),
                lines: vec![
                    ParsedDiffLine {
                        kind: DiffLineKind::Context,
                        prefix: " ".to_string(),
                        left_line_number: Some(1),
                        right_line_number: Some(1),
                        content: "fn main() {".to_string(),
                    },
                    ParsedDiffLine {
                        kind: DiffLineKind::Deletion,
                        prefix: "-".to_string(),
                        left_line_number: Some(2),
                        right_line_number: None,
                        content: "    old();".to_string(),
                    },
                    ParsedDiffLine {
                        kind: DiffLineKind::Addition,
                        prefix: "+".to_string(),
                        left_line_number: None,
                        right_line_number: Some(2),
                        content: "    new();".to_string(),
                    },
                ],
            }],
        };

        let right = diff_anchor_for_line(&parsed, 2, Some("RIGHT"))
            .expect("right line anchor should resolve");
        let left = diff_anchor_for_line(&parsed, 2, Some("LEFT"))
            .expect("left line anchor should resolve");

        assert_eq!(right.side.as_deref(), Some("RIGHT"));
        assert_eq!(right.hunk_header.as_deref(), Some("@@ -1,2 +1,2 @@"));
        assert_eq!(left.side.as_deref(), Some("LEFT"));
    }

    #[test]
    fn structural_diff_warmup_hides_complete_ready_status() {
        let state = StructuralDiffWarmupState {
            request_key: Some("pr:head".to_string()),
            total: 4,
            completed: 4,
            failed: 0,
            loading: false,
        };

        assert_eq!(state.status_text(), None);
    }

    #[test]
    fn structural_diff_warmup_status_avoids_ready_copy() {
        let state = StructuralDiffWarmupState {
            request_key: Some("pr:head".to_string()),
            total: 4,
            completed: 2,
            failed: 1,
            loading: true,
        };

        let text = state.status_text().expect("loading status is visible");
        assert!(text.starts_with("Preparing structural diffs 3/4"));
        assert!(!text.contains("ready"));
    }

    #[test]
    fn review_comment_navigation_follows_rendered_diff_order() {
        let detail = detail_with_threads(vec![
            review_thread("file", "src/a.rs", None, "RIGHT", "FILE", false),
            review_thread("inline", "src/a.rs", Some(3), "RIGHT", "LINE", false),
            review_thread("outdated", "src/a.rs", Some(1), "RIGHT", "LINE", true),
            review_thread("second-file", "src/b.rs", Some(5), "RIGHT", "LINE", false),
        ]);

        let items = review_comment_navigation_items(&detail, ReviewCenterMode::SemanticDiff);
        let ids = items
            .iter()
            .map(|item| detail.review_threads[item.thread_index].id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["file", "inline", "outdated", "second-file"]);
    }

    #[test]
    fn next_review_comment_after_focus_uses_rendered_row_position() {
        let detail = detail_with_threads(vec![
            review_thread("file", "src/a.rs", None, "RIGHT", "FILE", false),
            review_thread("inline", "src/a.rs", Some(3), "RIGHT", "LINE", false),
            review_thread("outdated", "src/a.rs", Some(1), "RIGHT", "LINE", true),
            review_thread("second-file", "src/b.rs", Some(5), "RIGHT", "LINE", false),
        ]);
        let items = review_comment_navigation_items(&detail, ReviewCenterMode::StructuralDiff);
        let focus = ReviewModeFocus {
            mode: ReviewCenterMode::StructuralDiff,
            file_path: "src/a.rs".to_string(),
            line: Some(4),
            side: Some("RIGHT".to_string()),
            anchor: None,
        };

        let index = first_review_comment_after_focus_index(&detail, &items, &focus)
            .expect("outdated thread should be after the focused row");

        assert_eq!(
            detail.review_threads[items[index].thread_index].id,
            "outdated"
        );
        assert_eq!(items[index].location.mode, ReviewCenterMode::StructuralDiff);
    }

    fn detail_with_threads(review_threads: Vec<PullRequestReviewThread>) -> PullRequestDetail {
        PullRequestDetail {
            id: "pr".to_string(),
            repository: "org/repo".to_string(),
            number: 1,
            title: "Review comments".to_string(),
            body: String::new(),
            url: "https://example.com/pr".to_string(),
            author_login: "alice".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: None,
            head_ref_oid: None,
            additions: 0,
            deletions: 0,
            changed_files: 2,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-05-13T00:00:00Z".to_string(),
            updated_at: "2026-05-13T00:00:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: BTreeMap::new(),
            comments: Vec::<PullRequestComment>::new(),
            latest_reviews: Vec::<PullRequestReview>::new(),
            review_threads,
            viewer_pending_review: None,
            files: vec![file("src/a.rs"), file("src/b.rs")],
            raw_diff: String::new(),
            parsed_diff: vec![parsed_file("src/a.rs", 1, 5), parsed_file("src/b.rs", 1, 6)],
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }

    fn file(path: &str) -> PullRequestFile {
        PullRequestFile {
            path: path.to_string(),
            additions: 0,
            deletions: 0,
            change_type: "MODIFIED".to_string(),
        }
    }

    fn parsed_file(path: &str, start: i64, end: i64) -> ParsedDiffFile {
        ParsedDiffFile {
            path: path.to_string(),
            previous_path: Some(path.to_string()),
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: format!("@@ -{start},{end} +{start},{end} @@"),
                lines: (start..=end)
                    .map(|line| ParsedDiffLine {
                        kind: DiffLineKind::Context,
                        prefix: " ".to_string(),
                        left_line_number: Some(line),
                        right_line_number: Some(line),
                        content: format!("line {line}"),
                    })
                    .collect(),
            }],
        }
    }

    fn review_thread(
        id: &str,
        path: &str,
        line: Option<i64>,
        diff_side: &str,
        subject_type: &str,
        is_outdated: bool,
    ) -> PullRequestReviewThread {
        PullRequestReviewThread {
            id: id.to_string(),
            path: path.to_string(),
            line,
            original_line: line,
            start_line: None,
            original_start_line: None,
            diff_side: diff_side.to_string(),
            start_diff_side: None,
            is_collapsed: false,
            is_outdated,
            is_resolved: false,
            subject_type: subject_type.to_string(),
            resolved_by_login: None,
            viewer_can_reply: true,
            viewer_can_resolve: true,
            viewer_can_unresolve: true,
            comments: vec![review_comment(id, path, line)],
        }
    }

    fn review_comment(id: &str, path: &str, line: Option<i64>) -> PullRequestReviewComment {
        PullRequestReviewComment {
            id: format!("{id}-comment"),
            author_login: "bob".to_string(),
            author_avatar_url: None,
            body: "Please check this.".to_string(),
            path: path.to_string(),
            line,
            original_line: line,
            start_line: None,
            original_start_line: None,
            state: "SUBMITTED".to_string(),
            created_at: "2026-05-13T00:00:00Z".to_string(),
            updated_at: "2026-05-13T00:00:00Z".to_string(),
            published_at: Some("2026-05-13T00:00:00Z".to_string()),
            reply_to_id: None,
            viewer_can_update: false,
            viewer_can_delete: false,
            url: "https://example.com/comment".to_string(),
        }
    }
}
