use crate::review_ai::{ReviewAiProgressUpdate, ReviewAiProvider, ReviewAiProviderStatus};

pub mod binary;
pub mod codex;
pub mod copilot;
pub mod errors;
pub mod jsonrepair;
pub mod progress;
pub mod prompt;
pub mod runtime;
pub mod schema;

pub trait CodingAgentBackend: Send + Sync {
    #[allow(dead_code)]
    fn provider(&self) -> ReviewAiProvider;
    fn status(&self) -> Result<ReviewAiProviderStatus, String>;
}

#[derive(Clone, Debug)]
pub struct AgentTextResponse {
    pub text: String,
    pub model: Option<String>,
    pub used_checkout_context: bool,
    pub checkout_command_count: usize,
    pub inspected_path_hints: Vec<String>,
    pub prompt_bytes: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct AgentJsonPromptOptions {
    pub task_label: &'static str,
    pub codex_overall_timeout_ms: u64,
    pub codex_inactivity_timeout_ms: u64,
    pub copilot_overall_timeout_ms: u64,
    pub copilot_inactivity_timeout_ms: u64,
    pub max_prompt_bytes: usize,
}

impl AgentJsonPromptOptions {
    pub const fn stack_planning() -> Self {
        Self {
            task_label: "Guided Review stack planning",
            codex_overall_timeout_ms: 90_000,
            codex_inactivity_timeout_ms: 35_000,
            copilot_overall_timeout_ms: 420_000,
            copilot_inactivity_timeout_ms: 300_000,
            max_prompt_bytes: 140_000,
        }
    }

    pub const fn stack_title_polish() -> Self {
        Self {
            task_label: "Guided Review stack title polish",
            codex_overall_timeout_ms: 20_000,
            codex_inactivity_timeout_ms: 10_000,
            copilot_overall_timeout_ms: 35_000,
            copilot_inactivity_timeout_ms: 14_000,
            max_prompt_bytes: 40_000,
        }
    }

    pub const fn review_partner() -> Self {
        Self {
            task_label: "Review Partner context",
            codex_overall_timeout_ms: 3_600_000,
            codex_inactivity_timeout_ms: 600_000,
            copilot_overall_timeout_ms: 720_000,
            copilot_inactivity_timeout_ms: 240_000,
            max_prompt_bytes: 220_000,
        }
    }

    pub const fn review_partner_focus() -> Self {
        Self {
            task_label: "Review Partner focus context",
            codex_overall_timeout_ms: 1_800_000,
            codex_inactivity_timeout_ms: 360_000,
            copilot_overall_timeout_ms: 480_000,
            copilot_inactivity_timeout_ms: 180_000,
            max_prompt_bytes: 160_000,
        }
    }

    pub const fn review_brief() -> Self {
        Self {
            task_label: "Review Brief generation",
            codex_overall_timeout_ms: 90_000,
            codex_inactivity_timeout_ms: 35_000,
            copilot_overall_timeout_ms: 420_000,
            copilot_inactivity_timeout_ms: 300_000,
            max_prompt_bytes: 200_000,
        }
    }

    pub const fn review_memory() -> Self {
        Self {
            task_label: "Review Memory candidate extraction",
            codex_overall_timeout_ms: 600_000,
            codex_inactivity_timeout_ms: 180_000,
            copilot_overall_timeout_ms: 600_000,
            copilot_inactivity_timeout_ms: 180_000,
            max_prompt_bytes: 120_000,
        }
    }
}

pub fn run_json_prompt(
    provider: ReviewAiProvider,
    working_directory: &str,
    prompt: String,
) -> Result<AgentTextResponse, String> {
    run_json_prompt_with_options_and_progress(
        provider,
        working_directory,
        prompt,
        AgentJsonPromptOptions::stack_planning(),
        &mut |_| {},
    )
}

pub fn run_json_prompt_with_progress(
    provider: ReviewAiProvider,
    working_directory: &str,
    prompt: String,
    on_progress: &mut dyn FnMut(ReviewAiProgressUpdate),
) -> Result<AgentTextResponse, String> {
    run_json_prompt_with_options_and_progress(
        provider,
        working_directory,
        prompt,
        AgentJsonPromptOptions::stack_planning(),
        on_progress,
    )
}

pub fn run_json_prompt_with_options(
    provider: ReviewAiProvider,
    working_directory: &str,
    prompt: String,
    options: AgentJsonPromptOptions,
) -> Result<AgentTextResponse, String> {
    run_json_prompt_with_options_and_progress(
        provider,
        working_directory,
        prompt,
        options,
        &mut |_| {},
    )
}

pub fn run_json_prompt_with_options_and_progress(
    provider: ReviewAiProvider,
    working_directory: &str,
    prompt: String,
    options: AgentJsonPromptOptions,
    on_progress: &mut dyn FnMut(ReviewAiProgressUpdate),
) -> Result<AgentTextResponse, String> {
    match provider {
        ReviewAiProvider::Codex => {
            codex::run_json_prompt_with_progress(working_directory, prompt, options, on_progress)
        }
        ReviewAiProvider::Copilot => {
            copilot::run_json_prompt_with_progress(working_directory, prompt, options, on_progress)
        }
    }
}

pub fn backend_for(provider: ReviewAiProvider) -> Box<dyn CodingAgentBackend> {
    match provider {
        ReviewAiProvider::Codex => Box::new(codex::CodexBackend::new()),
        ReviewAiProvider::Copilot => Box::new(copilot::CopilotBackend::new()),
    }
}

pub fn load_all_statuses() -> Vec<ReviewAiProviderStatus> {
    ReviewAiProvider::all()
        .iter()
        .map(|provider| {
            let backend = backend_for(*provider);
            backend
                .status()
                .unwrap_or_else(|error| ReviewAiProviderStatus {
                    provider: *provider,
                    label: provider.label().to_string(),
                    available: false,
                    authenticated: false,
                    message: error.clone(),
                    detail: error,
                    default_model: None,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_stack_planning_timeout_allows_silent_long_first_turns() {
        let options = AgentJsonPromptOptions::stack_planning();

        assert_eq!(options.copilot_overall_timeout_ms, 420_000);
        assert_eq!(options.copilot_inactivity_timeout_ms, 300_000);
    }

    #[test]
    fn review_brief_options_do_not_reuse_stack_planning_label_or_budget() {
        let options = AgentJsonPromptOptions::review_brief();

        assert_eq!(options.task_label, "Review Brief generation");
        assert!(
            options.max_prompt_bytes > AgentJsonPromptOptions::stack_planning().max_prompt_bytes
        );
    }
}
