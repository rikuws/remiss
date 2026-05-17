use crate::code_tour::{
    CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
    GeneratedCodeTour,
};

pub mod binary;
pub mod codex;
pub mod copilot;
pub mod errors;
pub mod jsonrepair;
pub mod merge;
pub mod progress;
pub mod prompt;
pub mod runtime;
pub mod schema;

pub trait CodingAgentBackend: Send + Sync {
    #[allow(dead_code)]
    fn provider(&self) -> CodeTourProvider;
    fn status(&self) -> Result<CodeTourProviderStatus, String>;
    fn generate(
        &self,
        input: &GenerateCodeTourInput,
        on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
    ) -> Result<GeneratedCodeTour, String>;
}

#[derive(Clone, Debug)]
pub struct AgentTextResponse {
    pub text: String,
    pub model: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct AgentJsonPromptOptions {
    pub task_label: &'static str,
    pub codex_overall_timeout_ms: u64,
    pub codex_inactivity_timeout_ms: u64,
    pub copilot_overall_timeout_ms: u64,
    pub copilot_inactivity_timeout_ms: u64,
}

impl AgentJsonPromptOptions {
    pub const fn stack_planning() -> Self {
        Self {
            task_label: "Guided Review stack planning",
            codex_overall_timeout_ms: 90_000,
            codex_inactivity_timeout_ms: 35_000,
            copilot_overall_timeout_ms: 120_000,
            copilot_inactivity_timeout_ms: 45_000,
        }
    }

    pub const fn review_partner() -> Self {
        Self {
            task_label: "Review Partner context",
            codex_overall_timeout_ms: 1_800_000,
            codex_inactivity_timeout_ms: 300_000,
            copilot_overall_timeout_ms: 1_800_000,
            copilot_inactivity_timeout_ms: 300_000,
        }
    }

    pub const fn review_partner_focus() -> Self {
        Self {
            task_label: "Review Partner focus context",
            codex_overall_timeout_ms: 600_000,
            codex_inactivity_timeout_ms: 180_000,
            copilot_overall_timeout_ms: 600_000,
            copilot_inactivity_timeout_ms: 180_000,
        }
    }
}

pub fn run_json_prompt(
    provider: CodeTourProvider,
    working_directory: &str,
    prompt: String,
) -> Result<AgentTextResponse, String> {
    run_json_prompt_with_options(
        provider,
        working_directory,
        prompt,
        AgentJsonPromptOptions::stack_planning(),
    )
}

pub fn run_json_prompt_with_options(
    provider: CodeTourProvider,
    working_directory: &str,
    prompt: String,
    options: AgentJsonPromptOptions,
) -> Result<AgentTextResponse, String> {
    match provider {
        CodeTourProvider::Codex => codex::run_json_prompt(working_directory, prompt, options),
        CodeTourProvider::Copilot => copilot::run_json_prompt(working_directory, prompt, options),
    }
}

pub fn backend_for(provider: CodeTourProvider) -> Box<dyn CodingAgentBackend> {
    match provider {
        CodeTourProvider::Codex => Box::new(codex::CodexBackend::new()),
        CodeTourProvider::Copilot => Box::new(copilot::CopilotBackend::new()),
    }
}

pub fn load_all_statuses() -> Vec<CodeTourProviderStatus> {
    CodeTourProvider::all()
        .iter()
        .map(|provider| {
            let backend = backend_for(*provider);
            backend
                .status()
                .unwrap_or_else(|error| CodeTourProviderStatus {
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
