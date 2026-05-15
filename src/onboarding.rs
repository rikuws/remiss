use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;
use crate::gh::CommandOutput;
use crate::github::AuthState;

const ONBOARDING_PROGRESS_CACHE_KEY: &str = "app-onboarding-progress-v1";
pub const WELCOME_WIZARD_ID: &str = "welcome";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardKind {
    Welcome,
    Feature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardTone {
    Welcome,
    Workspace,
    Review,
    Ai,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardStepTarget {
    GithubSetup,
    TutorialReview,
    GuidedReview,
    LocalReview,
    ReviewFeedback,
}

impl WizardStepTarget {
    pub fn label(self) -> &'static str {
        match self {
            Self::GithubSetup => "GitHub CLI",
            Self::TutorialReview => "Review surface",
            Self::GuidedReview => "Guided Review",
            Self::LocalReview => "Local Review",
            Self::ReviewFeedback => "Review feedback",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WizardStepDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub body: &'static str,
    pub bullets: &'static [&'static str],
    pub tone: WizardTone,
    pub target: WizardStepTarget,
}

#[derive(Clone, Copy, Debug)]
pub struct WizardDefinition {
    pub id: &'static str,
    pub version: u32,
    pub kind: WizardKind,
    pub enabled: bool,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub complete_label: &'static str,
    pub steps: &'static [WizardStepDefinition],
}

impl WizardDefinition {
    pub fn completion_key(&self) -> String {
        format!("{}:v{}", self.id, self.version)
    }
}

#[derive(Clone, Debug)]
pub struct WizardSession {
    pub definition: WizardDefinition,
    pub step_index: usize,
    pub forced: bool,
}

impl WizardSession {
    pub fn new(definition: WizardDefinition, forced: bool) -> Self {
        Self {
            definition,
            step_index: 0,
            forced,
        }
    }

    pub fn active_step(&self) -> WizardStepDefinition {
        self.definition.steps[self.step_index.min(self.step_count().saturating_sub(1))]
    }

    pub fn step_count(&self) -> usize {
        self.definition.steps.len()
    }

    pub fn is_first_step(&self) -> bool {
        self.step_index == 0
    }

    pub fn is_last_step(&self) -> bool {
        self.step_index + 1 >= self.step_count()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupWizardOptions {
    pub force_wizard_id: Option<String>,
}

impl StartupWizardOptions {
    pub fn from_env_and_args() -> Self {
        let env_force_wizard = std::env::var("REMISS_FORCE_WIZARD").ok();
        let env_force_welcome =
            env_truthy("REMISS_FORCE_WELCOME_WIZARD") || env_truthy("REMISS_WELCOME_WIZARD");
        parse_startup_wizard_options(
            std::env::args().skip(1),
            env_force_wizard.as_deref(),
            env_force_welcome,
        )
    }

    pub fn force_welcome() -> Self {
        Self {
            force_wizard_id: Some(WELCOME_WIZARD_ID.to_string()),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingProgress {
    #[serde(default)]
    pub completed_wizard_keys: BTreeSet<String>,
    #[serde(default)]
    pub last_completed_wizard_key: Option<String>,
    #[serde(default)]
    pub last_completed_at_ms: Option<i64>,
}

impl OnboardingProgress {
    pub fn is_completed(&self, definition: &WizardDefinition) -> bool {
        self.completed_wizard_keys
            .contains(&definition.completion_key())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhSetupState {
    Checking,
    Ready,
    Missing,
    NeedsAuth,
}

impl GhSetupState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking",
            Self::Ready => "Ready",
            Self::Missing => "Missing",
            Self::NeedsAuth => "Needs auth",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhSetupStatus {
    pub state: GhSetupState,
    pub version: Option<String>,
    pub login: Option<String>,
    pub hostname: Option<String>,
    pub message: String,
}

impl GhSetupStatus {
    pub fn checking() -> Self {
        Self {
            state: GhSetupState::Checking,
            version: None,
            login: None,
            hostname: None,
            message: "Checking GitHub CLI setup...".to_string(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state == GhSetupState::Ready
    }
}

pub fn derive_gh_setup_status(
    gh_version_result: Result<CommandOutput, String>,
    auth_result: Result<AuthState, String>,
) -> GhSetupStatus {
    let output = match gh_version_result {
        Ok(output) if output.exit_code == Some(0) => output,
        Ok(output) => {
            let message = if !output.stderr.trim().is_empty() {
                output.stderr
            } else if !output.stdout.trim().is_empty() {
                output.stdout
            } else {
                "GitHub CLI (`gh`) did not return a usable version.".to_string()
            };
            return GhSetupStatus {
                state: GhSetupState::Missing,
                version: None,
                login: None,
                hostname: None,
                message,
            };
        }
        Err(message) => {
            return GhSetupStatus {
                state: GhSetupState::Missing,
                version: None,
                login: None,
                hostname: None,
                message,
            };
        }
    };

    let version = output
        .stdout
        .lines()
        .next()
        .map(str::trim)
        .and_then(|line| {
            if line.is_empty() {
                None
            } else {
                Some(line.to_string())
            }
        });

    match auth_result {
        Ok(auth) if auth.is_authenticated => GhSetupStatus {
            state: GhSetupState::Ready,
            version,
            login: auth.active_login,
            hostname: auth.active_hostname,
            message: auth.message,
        },
        Ok(auth) => GhSetupStatus {
            state: GhSetupState::NeedsAuth,
            version,
            login: auth.active_login,
            hostname: auth.active_hostname,
            message: auth.message,
        },
        Err(message) => GhSetupStatus {
            state: GhSetupState::NeedsAuth,
            version,
            login: None,
            hostname: None,
            message,
        },
    }
}

const WELCOME_STEPS: &[WizardStepDefinition] = &[
    WizardStepDefinition {
        id: "github-cli",
        title: "Connect GitHub CLI",
        body: "Remiss uses GitHub CLI for live pull requests, comments, review submission, and repository prep. Setup is non-blocking, but real review work needs `gh` installed and authenticated.",
        bullets: &[
            "Install `gh` if it is missing.",
            "Authenticate with GitHub before syncing live review queues.",
            "Run `gh auth setup-git` so managed checkouts can fetch private repositories.",
        ],
        tone: WizardTone::Workspace,
        target: WizardStepTarget::GithubSetup,
    },
    WizardStepDefinition {
        id: "tutorial-review",
        title: "Review a pull request",
        body: "This local tutorial pull request shows the real review surface without calling GitHub or requiring an existing pull request.",
        bullets: &[
            "The synthetic PR opens only while onboarding is active.",
            "The Review surface shows changed files, diff context, existing comments, and pending feedback.",
            "The tutorial data is not added to review queues or written to GitHub cache data.",
        ],
        tone: WizardTone::Review,
        target: WizardStepTarget::TutorialReview,
    },
    WizardStepDefinition {
        id: "guided-review",
        title: "Use Guided Review",
        body: "Guided Review combines the generated review path and review stack into one mode for moving through larger changes deliberately.",
        bullets: &[
            "Use Guided Review when the diff is large or unfamiliar.",
            "Move by review layer, then open the exact files for that layer.",
            "Switch back to Code whenever you want the plain diff or source lens.",
        ],
        tone: WizardTone::Ai,
        target: WizardStepTarget::GuidedReview,
    },
    WizardStepDefinition {
        id: "local-review",
        title: "Local review catches work before it is pushed",
        body: "Add a working checkout to review unpushed local changes with the same review surface before you open a pull request.",
        bullets: &[
            "Local repositories are remembered for quick refreshes.",
            "The app inspects the working checkout and reports setup problems in place.",
            "You can review local changes without waiting for a pull request.",
        ],
        tone: WizardTone::Local,
        target: WizardStepTarget::LocalReview,
    },
    WizardStepDefinition {
        id: "review-feedback",
        title: "Waypoints and comments keep the pass organized",
        body: "Use line actions, waypoints, markdown preview, and review submission controls to keep feedback structured as you move through files.",
        bullets: &[
            "Add waypoints to save important review stops.",
            "Draft line comments stay visible in the review flow.",
            "Finish review submits the pending feedback when you are ready.",
        ],
        tone: WizardTone::Review,
        target: WizardStepTarget::ReviewFeedback,
    },
];

pub const WIZARD_DEFINITIONS: &[WizardDefinition] = &[WizardDefinition {
    id: WELCOME_WIZARD_ID,
    version: 1,
    kind: WizardKind::Welcome,
    enabled: true,
    title: "Welcome to Remiss",
    subtitle: "A quick pass through the review workspace before you start.",
    complete_label: "Start reviewing",
    steps: WELCOME_STEPS,
}];

pub fn load_onboarding_progress(cache: &CacheStore) -> Result<OnboardingProgress, String> {
    Ok(cache
        .get::<OnboardingProgress>(ONBOARDING_PROGRESS_CACHE_KEY)?
        .map(|document| document.value)
        .unwrap_or_default())
}

pub fn save_onboarding_progress(
    cache: &CacheStore,
    progress: &OnboardingProgress,
) -> Result<(), String> {
    cache.put(ONBOARDING_PROGRESS_CACHE_KEY, progress, now_ms())
}

pub fn initial_wizard_session(
    progress: &OnboardingProgress,
    options: &StartupWizardOptions,
) -> Option<WizardSession> {
    if let Some(forced_id) = options.force_wizard_id.as_deref() {
        return wizard_by_id(forced_id).map(|definition| WizardSession::new(definition, true));
    }

    next_pending_wizard(progress)
}

pub fn next_pending_wizard(progress: &OnboardingProgress) -> Option<WizardSession> {
    WIZARD_DEFINITIONS
        .iter()
        .copied()
        .find(|definition| definition.enabled && !progress.is_completed(definition))
        .map(|definition| WizardSession::new(definition, false))
}

pub fn wizard_by_id(id: &str) -> Option<WizardDefinition> {
    WIZARD_DEFINITIONS
        .iter()
        .copied()
        .find(|definition| definition.enabled && definition.id == id)
}

pub fn mark_wizard_completed(progress: &mut OnboardingProgress, definition: &WizardDefinition) {
    let completion_key = definition.completion_key();
    progress
        .completed_wizard_keys
        .insert(completion_key.clone());
    progress.last_completed_wizard_key = Some(completion_key);
    progress.last_completed_at_ms = Some(now_ms());
}

pub fn parse_startup_wizard_options<I, S>(
    args: I,
    env_force_wizard: Option<&str>,
    env_force_welcome: bool,
) -> StartupWizardOptions
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut force_wizard_id = env_force_wizard
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if env_force_welcome {
        force_wizard_id = Some(WELCOME_WIZARD_ID.to_string());
    }

    let mut args = args.into_iter().map(Into::into).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--welcome-wizard" | "--force-welcome-wizard" | "--debug-welcome-wizard" => {
                force_wizard_id = Some(WELCOME_WIZARD_ID.to_string());
            }
            "--force-wizard" | "--show-wizard" => {
                if let Some(id) = args.next().map(|value| value.trim().to_string()) {
                    if !id.is_empty() {
                        force_wizard_id = Some(id);
                    }
                }
            }
            _ => {
                if let Some(id) = arg.strip_prefix("--force-wizard=") {
                    let id = id.trim();
                    if !id.is_empty() {
                        force_wizard_id = Some(id.to_string());
                    }
                } else if let Some(id) = arg.strip_prefix("--show-wizard=") {
                    let id = id.trim();
                    if !id.is_empty() {
                        force_wizard_id = Some(id.to_string());
                    }
                }
            }
        }
    }

    StartupWizardOptions { force_wizard_id }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_output(exit_code: Option<i32>, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            stdout_bytes: stdout.as_bytes().to_vec(),
            stderr_bytes: stderr.as_bytes().to_vec(),
            timed_out: false,
            duration_ms: 1,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    fn auth_state(authenticated: bool) -> AuthState {
        AuthState {
            is_authenticated: authenticated,
            active_login: authenticated.then(|| "octo".to_string()),
            active_hostname: Some("github.com".to_string()),
            message: if authenticated {
                "Using gh auth on github.com.".to_string()
            } else {
                "gh is installed but not authenticated.".to_string()
            },
        }
    }

    #[test]
    fn initial_wizard_returns_welcome_until_completed() {
        let progress = OnboardingProgress::default();
        let session = initial_wizard_session(&progress, &StartupWizardOptions::default())
            .expect("welcome wizard should be pending");

        assert_eq!(session.definition.id, WELCOME_WIZARD_ID);
        assert!(!session.forced);
        assert_eq!(session.definition.completion_key(), "welcome:v1");
        assert_eq!(
            session
                .definition
                .steps
                .iter()
                .map(|step| step.target)
                .collect::<Vec<_>>(),
            vec![
                WizardStepTarget::GithubSetup,
                WizardStepTarget::TutorialReview,
                WizardStepTarget::GuidedReview,
                WizardStepTarget::LocalReview,
                WizardStepTarget::ReviewFeedback,
            ]
        );
    }

    #[test]
    fn completion_suppresses_wizard_but_force_overrides() {
        let mut progress = OnboardingProgress::default();
        let welcome = wizard_by_id(WELCOME_WIZARD_ID).expect("welcome wizard exists");
        mark_wizard_completed(&mut progress, &welcome);

        assert!(initial_wizard_session(&progress, &StartupWizardOptions::default()).is_none());

        let forced = initial_wizard_session(&progress, &StartupWizardOptions::force_welcome())
            .expect("forced welcome wizard should open");
        assert_eq!(forced.definition.id, WELCOME_WIZARD_ID);
        assert!(forced.forced);
    }

    #[test]
    fn parses_force_wizard_flags() {
        let options = parse_startup_wizard_options(
            ["--force-wizard", "welcome"],
            Some("future-feature"),
            false,
        );
        assert_eq!(options.force_wizard_id.as_deref(), Some("welcome"));

        let options = parse_startup_wizard_options(["--force-wizard=welcome"], None, false);
        assert_eq!(options.force_wizard_id.as_deref(), Some("welcome"));

        let options = parse_startup_wizard_options(Vec::<String>::new(), None, true);
        assert_eq!(options.force_wizard_id.as_deref(), Some(WELCOME_WIZARD_ID));
    }

    #[test]
    fn gh_setup_status_ready_when_installed_and_authenticated() {
        let status = derive_gh_setup_status(
            Ok(command_output(Some(0), "gh version 2.70.0\n", "")),
            Ok(auth_state(true)),
        );

        assert_eq!(status.state, GhSetupState::Ready);
        assert_eq!(status.version.as_deref(), Some("gh version 2.70.0"));
        assert_eq!(status.login.as_deref(), Some("octo"));
    }

    #[test]
    fn gh_setup_status_needs_auth_when_installed_without_auth() {
        let status = derive_gh_setup_status(
            Ok(command_output(Some(0), "gh version 2.70.0\n", "")),
            Ok(auth_state(false)),
        );

        assert_eq!(status.state, GhSetupState::NeedsAuth);
        assert_eq!(status.version.as_deref(), Some("gh version 2.70.0"));
        assert!(status.message.contains("not authenticated"));
    }

    #[test]
    fn gh_setup_status_missing_when_binary_probe_fails() {
        let status = derive_gh_setup_status(
            Err("GitHub CLI (`gh`) is not installed.".to_string()),
            Ok(auth_state(false)),
        );

        assert_eq!(status.state, GhSetupState::Missing);
        assert!(status.message.contains("not installed"));
    }
}
