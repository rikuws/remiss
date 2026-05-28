use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{px, size, Pixels, Size};

use crate::{
    demo_data,
    env_flags::permissive_truthy_value,
    github::{PullRequestDetail, RepositoryFileContent},
    review_session::ReviewCenterMode,
    state::{
        pr_key, summary_key, AppState, FileContentState, PreparedFileContent, PreparedFileLine,
        PullRequestSurface, SectionId,
    },
    syntax,
    theme::{self, CodeFontSizePreference, DiffColorThemePreference, ThemePreference},
};

pub const SCREENSHOT_MODE_ENV: &str = "REMISS_SCREENSHOT_MODE";
pub const SCREENSHOT_SCENARIO_ENV: &str = "REMISS_SCREENSHOT_SCENARIO";
pub const SCREENSHOT_OUTPUT_FILE_ENV: &str = "REMISS_SCREENSHOT_OUTPUT_FILE";

const DEFAULT_SCENARIO: ScreenshotScenario = ScreenshotScenario::ReviewWorkspace;
const SCREENSHOT_WINDOW_WIDTH: f32 = 1440.0;
const SCREENSHOT_WINDOW_HEIGHT: f32 = 1000.0;
const REVIEW_WORKSPACE_REPOSITORY: &str = "remiss/review-core";
const REVIEW_WORKSPACE_NUMBER: i64 = 201;
const REVIEW_WORKSPACE_FILE: &str = "src/review_routes.rs";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotConfig {
    pub scenario: ScreenshotScenario,
    pub output_file: PathBuf,
    pub ready_file: PathBuf,
    pub cache_path: PathBuf,
}

impl ScreenshotConfig {
    pub fn from_env() -> Result<Option<Self>, String> {
        config_from_vars(|name| std::env::var(name).ok())
    }

    pub fn window_size(&self) -> Size<Pixels> {
        size(px(SCREENSHOT_WINDOW_WIDTH), px(SCREENSHOT_WINDOW_HEIGHT))
    }

    pub fn clear_ready_file(&self) -> Result<(), String> {
        match fs::remove_file(&self.ready_file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Failed to clear screenshot ready file '{}': {error}",
                self.ready_file.display()
            )),
        }
    }

    pub fn write_ready_file(&self) -> Result<(), String> {
        if let Some(parent) = self.ready_file.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Failed to create screenshot ready file directory '{}': {error}",
                    parent.display()
                )
            })?;
        }

        fs::write(
            &self.ready_file,
            format!(
                "scenario={}\noutput={}\n",
                self.scenario.slug(),
                self.output_file.display()
            ),
        )
        .map_err(|error| {
            format!(
                "Failed to write screenshot ready file '{}': {error}",
                self.ready_file.display()
            )
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotScenario {
    ReviewWorkspace,
}

impl ScreenshotScenario {
    pub fn parse(value: &str) -> Result<Self, String> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "review-workspace" | "review_workspace" | "reviewworkspace" => {
                Ok(Self::ReviewWorkspace)
            }
            _ => Err(format!(
                "Unsupported screenshot scenario '{value}'. Supported scenarios: review-workspace"
            )),
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::ReviewWorkspace => "review-workspace",
        }
    }
}

pub fn screenshot_mode_enabled() -> bool {
    screenshot_mode_enabled_from_vars(|name| std::env::var(name).ok())
}

pub(crate) fn screenshot_mode_enabled_from_vars<F>(mut var: F) -> bool
where
    F: FnMut(&str) -> Option<String>,
{
    var(SCREENSHOT_MODE_ENV)
        .map(|value| permissive_truthy_value(&value))
        .unwrap_or(false)
}

pub fn stage_initial_state(state: &mut AppState, config: &ScreenshotConfig) {
    force_screenshot_theme(state);
    match config.scenario {
        ScreenshotScenario::ReviewWorkspace => stage_review_workspace(state),
    }
}

pub fn is_ready_for_capture(state: &AppState, config: &ScreenshotConfig) -> bool {
    match config.scenario {
        ScreenshotScenario::ReviewWorkspace => review_workspace_ready(state),
    }
}

fn config_from_vars<F>(mut var: F) -> Result<Option<ScreenshotConfig>, String>
where
    F: FnMut(&str) -> Option<String>,
{
    if !screenshot_mode_enabled_from_vars(&mut var) {
        return Ok(None);
    }

    let scenario = var(SCREENSHOT_SCENARIO_ENV)
        .filter(|value| !value.trim().is_empty())
        .map(|value| ScreenshotScenario::parse(&value))
        .transpose()?
        .unwrap_or(DEFAULT_SCENARIO);
    let output_file = var(SCREENSHOT_OUTPUT_FILE_ENV)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            format!("{SCREENSHOT_OUTPUT_FILE_ENV} is required when {SCREENSHOT_MODE_ENV}=1")
        })?;
    let ready_file = ready_file_for_output(&output_file);
    let cache_path = temp_cache_path(scenario);

    Ok(Some(ScreenshotConfig {
        scenario,
        output_file,
        ready_file,
        cache_path,
    }))
}

fn ready_file_for_output(output_file: &Path) -> PathBuf {
    let mut file_name = output_file
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("screenshot"));
    file_name.push(".ready");
    output_file.with_file_name(file_name)
}

fn temp_cache_path(scenario: ScreenshotScenario) -> PathBuf {
    std::env::temp_dir()
        .join(format!(
            "remiss-screenshot-{}-{}",
            std::process::id(),
            scenario.slug()
        ))
        .join("cache.sqlite3")
}

fn force_screenshot_theme(state: &mut AppState) {
    state.set_theme_preference(ThemePreference::Dark);
    state.set_code_font_size_preference(CodeFontSizePreference::default_size());
    state.set_diff_color_theme_preference(DiffColorThemePreference::Graphite);
    theme::set_active_diff_color_theme(DiffColorThemePreference::Graphite);
}

fn stage_review_workspace(state: &mut AppState) {
    let detail_key = pr_key(REVIEW_WORKSPACE_REPOSITORY, REVIEW_WORKSPACE_NUMBER);
    let Some(summary) =
        demo_data::pull_request_summary(REVIEW_WORKSPACE_REPOSITORY, REVIEW_WORKSPACE_NUMBER)
    else {
        return;
    };
    let Some(snapshot) = demo_data::pull_request_detail_snapshot(
        REVIEW_WORKSPACE_REPOSITORY,
        REVIEW_WORKSPACE_NUMBER,
    ) else {
        return;
    };
    let staged_file_content = snapshot
        .detail
        .as_ref()
        .and_then(|detail| stage_file_content_for_detail(detail, REVIEW_WORKSPACE_FILE));

    state.workspace = Some(demo_data::workspace_snapshot());
    state.workspace_loading = false;
    state.workspace_syncing = false;
    state.workspace_error = None;
    state.bootstrap_loading = false;
    state.gh_available = true;
    state.gh_version = Some("demo".to_string());
    state.active_queue_id = "reviewRequested".to_string();
    state.active_section = SectionId::Pulls;
    state.active_surface = PullRequestSurface::Files;
    state.active_pr_key = Some(detail_key.clone());
    state.selected_file_path = Some(REVIEW_WORKSPACE_FILE.to_string());
    state.selected_diff_anchor = None;
    state.active_onboarding_wizard = None;
    state.onboarding_route_before_tutorial = None;
    state.pr_header_compact = false;
    state.review_body.clear();
    state.review_editor_active = false;
    state.review_finish_modal_open = false;
    state.review_loading = false;
    state.review_message = None;
    state.review_success = false;
    state.inline_comment_loading = false;
    state.inline_comment_error = None;
    state.active_review_line_action = None;
    state.active_review_line_action_position = None;
    state.notification_drawer_open = false;
    state.palette_open = false;
    state.file_chooser_open = false;
    state.waypoint_spotlight_open = false;
    state.review_ai_settings.background_syncing = false;
    state.review_ai_settings.background_message = None;
    state.review_ai_settings.background_error = None;

    state.open_tabs.retain(|tab| summary_key(tab) != detail_key);
    state.open_tabs.insert(0, summary);

    let detail_state = state.detail_states.entry(detail_key).or_default();
    detail_state.snapshot = Some(snapshot);
    detail_state.loading = false;
    detail_state.syncing = false;
    detail_state.error = None;
    detail_state.local_repository_loading = false;
    detail_state.local_repository_error = None;
    detail_state.review_intelligence_loading = false;
    detail_state.review_route_loading = false;
    detail_state.review_route_error = None;
    detail_state.review_session.loaded = true;
    detail_state.review_session.error = None;
    detail_state.review_session.center_mode = ReviewCenterMode::SemanticDiff;
    detail_state.review_session.code_lens_mode = ReviewCenterMode::SemanticDiff;
    detail_state.review_session.show_file_tree = true;
    detail_state.review_session.source_target = None;
    if let Some(file_content) = staged_file_content {
        detail_state
            .file_content_states
            .insert(REVIEW_WORKSPACE_FILE.to_string(), file_content);
    }

    state.reset_review_focus_scroll();
}

fn stage_file_content_for_detail(
    detail: &PullRequestDetail,
    path: &str,
) -> Option<FileContentState> {
    let file = detail.files.iter().find(|file| file.path == path)?;
    let reference = if file.change_type == "DELETED" {
        detail
            .base_ref_oid
            .clone()
            .unwrap_or_else(|| detail.base_ref_name.clone())
    } else {
        detail
            .head_ref_oid
            .clone()
            .unwrap_or_else(|| detail.head_ref_name.clone())
    };
    let reference = reference.trim().to_string();
    if reference.is_empty() {
        return None;
    }

    let document = demo_data::pull_request_file_content(&detail.repository, &reference, path)?;
    let prepared = prepare_screenshot_file_content(path, &reference, &document);

    Some(FileContentState {
        request_key: Some(format!(
            "{}:{reference}:{path}:{}",
            detail.updated_at, detail.repository
        )),
        document: Some(document),
        prepared: Some(prepared),
        loading: false,
        error: None,
    })
}

fn prepare_screenshot_file_content(
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
            .collect::<Vec<Vec<_>>>()
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

fn review_workspace_ready(state: &AppState) -> bool {
    let detail_key = pr_key(REVIEW_WORKSPACE_REPOSITORY, REVIEW_WORKSPACE_NUMBER);
    if state.bootstrap_loading
        || state.workspace_loading
        || state.workspace_syncing
        || state.review_loading
        || state.inline_comment_loading
        || state.active_section != SectionId::Pulls
        || state.active_surface != PullRequestSurface::Files
        || state.active_pr_key.as_deref() != Some(detail_key.as_str())
        || state.selected_file_path.as_deref() != Some(REVIEW_WORKSPACE_FILE)
    {
        return false;
    }

    let Some(detail_state) = state.detail_states.get(&detail_key) else {
        return false;
    };
    if detail_state.loading
        || detail_state.syncing
        || detail_state.error.is_some()
        || detail_state.local_repository_loading
        || detail_state.review_intelligence_loading
        || detail_state.review_route_loading
        || detail_state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.detail.as_ref())
            .is_none()
    {
        return false;
    }

    let Some(file_state) = detail_state.file_content_states.get(REVIEW_WORKSPACE_FILE) else {
        return false;
    };
    !file_state.loading && file_state.error.is_none() && file_state.prepared.is_some()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{cache::CacheStore, onboarding::StartupWizardOptions};

    use super::*;

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn vars(values: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<HashMap<_, _>>();
        move |name| values.get(name).cloned()
    }

    fn temp_cache() -> CacheStore {
        CacheStore::new(unique_test_path("cache.sqlite3")).expect("failed to create cache")
    }

    fn unique_test_path(file_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "remiss-screenshot-mode-test-{nanos}-{test_id}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir.join(file_name)
    }

    #[test]
    fn disabled_mode_ignores_missing_output() {
        let config = config_from_vars(vars(&[])).expect("parse config");

        assert!(config.is_none());
    }

    #[test]
    fn enabled_mode_requires_output_file() {
        let error = config_from_vars(vars(&[(SCREENSHOT_MODE_ENV, "1")]))
            .expect_err("missing output should fail");

        assert!(error.contains(SCREENSHOT_OUTPUT_FILE_ENV));
    }

    #[test]
    fn parses_review_workspace_config_and_paths() {
        let output = PathBuf::from("/tmp/remiss/screenshots/review-workspace.png");
        let config = config_from_vars(vars(&[
            (SCREENSHOT_MODE_ENV, "1"),
            (SCREENSHOT_SCENARIO_ENV, "review-workspace"),
            (
                SCREENSHOT_OUTPUT_FILE_ENV,
                output.to_str().expect("unicode path"),
            ),
        ]))
        .expect("parse config")
        .expect("config");

        assert_eq!(config.scenario, ScreenshotScenario::ReviewWorkspace);
        assert_eq!(config.output_file, output);
        assert_eq!(
            config.ready_file,
            PathBuf::from("/tmp/remiss/screenshots/review-workspace.png.ready")
        );
        assert!(config.cache_path.starts_with(std::env::temp_dir()));
        assert!(config.cache_path.ends_with("cache.sqlite3"));
    }

    #[test]
    fn rejects_unknown_scenario() {
        let error = config_from_vars(vars(&[
            (SCREENSHOT_MODE_ENV, "1"),
            (SCREENSHOT_SCENARIO_ENV, "welcome"),
            (SCREENSHOT_OUTPUT_FILE_ENV, "/tmp/remiss.png"),
        ]))
        .expect_err("unknown scenario should fail");

        assert!(error.contains("Unsupported screenshot scenario"));
    }

    #[test]
    fn truthy_screenshot_mode_values_are_detected() {
        assert!(screenshot_mode_enabled_from_vars(vars(&[(
            SCREENSHOT_MODE_ENV,
            "true",
        )])));
        assert!(!screenshot_mode_enabled_from_vars(vars(&[(
            SCREENSHOT_MODE_ENV,
            "off",
        )])));
    }

    #[test]
    fn review_workspace_staging_sets_deterministic_state() {
        let config = ScreenshotConfig {
            scenario: ScreenshotScenario::ReviewWorkspace,
            output_file: PathBuf::from("/tmp/review-workspace.png"),
            ready_file: PathBuf::from("/tmp/review-workspace.png.ready"),
            cache_path: PathBuf::from("/tmp/remiss-screenshot-cache.sqlite3"),
        };
        let mut state = AppState::new(temp_cache(), StartupWizardOptions::default());

        stage_initial_state(&mut state, &config);

        assert_eq!(state.active_section, SectionId::Pulls);
        assert_eq!(state.active_surface, PullRequestSurface::Files);
        assert_eq!(
            state.active_pr_key.as_deref(),
            Some("remiss/review-core#201")
        );
        assert_eq!(
            state.selected_file_path.as_deref(),
            Some(REVIEW_WORKSPACE_FILE)
        );
        assert_eq!(state.theme_preference, ThemePreference::Dark);
        assert_eq!(
            state.diff_color_theme_preference,
            DiffColorThemePreference::Graphite
        );
        assert_eq!(
            state.code_font_size_preference,
            CodeFontSizePreference::default_size()
        );
        assert!(!state.workspace_loading);
        assert!(!state.workspace_syncing);
        assert!(!state.bootstrap_loading);

        let detail_state = state
            .detail_states
            .get("remiss/review-core#201")
            .expect("detail state");
        assert!(detail_state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.detail.as_ref())
            .is_some());
        assert!(!detail_state.loading);
        assert!(!detail_state.syncing);
        assert!(!detail_state.local_repository_loading);
        let file_state = detail_state
            .file_content_states
            .get(REVIEW_WORKSPACE_FILE)
            .expect("staged file content");
        assert!(!file_state.loading);
        assert!(file_state.error.is_none());
        assert!(file_state.document.is_some());
        assert!(file_state.prepared.is_some());
        assert_eq!(
            detail_state.review_session.center_mode,
            ReviewCenterMode::SemanticDiff
        );
    }
}
