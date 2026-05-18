use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant as StdInstant};

use github_copilot_sdk::resolve::{copilot_binary_with_source, BinarySource};
use github_copilot_sdk::{
    Client, ClientOptions, Error as SdkError, LogLevel, MessageOptions, PermissionRequestData,
    PermissionRequestKind, RecvError, SessionConfig, SessionError, SessionEvent,
};
use serde_json::Value;
use tokio::time::{Instant as TokioInstant, MissedTickBehavior};

use crate::code_tour::{
    CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
    GeneratedCodeTour,
};

use super::errors::{generation_abort_message, AbortKind, AbortReason};
use super::jsonrepair::parse_tolerant;
use super::merge::{build_copilot_fallback_tour, merge_tour, TourResponse};
use super::progress::{limit_text, make_progress};
use super::prompt::build_tour_prompt;
use super::runtime;
use super::{AgentJsonPromptOptions, AgentTextResponse, CodingAgentBackend};

const OVERALL_TIMEOUT_MS: u64 = 480_000;
const INACTIVITY_TIMEOUT_MS: u64 = 120_000;
const RUNNING_TICKER_MS: u64 = 10_000;
const MAX_PROMPT_BYTES: usize = 120_000;
const MAX_STACK_PLAN_PROMPT_BYTES: usize = 140_000;
const COPILOT_TOOL_ALLOWLISTS: &[&[&str]] = &[
    &["view", "rg", "glob"],
    &["view", "grep", "glob"],
    &["view", "glob"],
];

pub struct CopilotBackend;

impl CopilotBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CopilotBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
struct CopilotOutcome {
    final_text: Option<String>,
    last_visible_activity: Option<String>,
    abort: Option<AbortReason>,
    error: Option<String>,
    model: Option<String>,
    saw_meaningful_progress: bool,
    current_turn_stream: String,
    tool_names: HashMap<String, String>,
}

struct CopilotRun {
    outcome: CopilotOutcome,
}

impl CodingAgentBackend for CopilotBackend {
    fn provider(&self) -> CodeTourProvider {
        CodeTourProvider::Copilot
    }

    fn status(&self) -> Result<CodeTourProviderStatus, String> {
        match copilot_binary_with_source() {
            Ok((binary, source)) => {
                let version = probe_version(&binary).unwrap_or_else(|| "installed".to_string());
                Ok(CodeTourProviderStatus {
                    provider: CodeTourProvider::Copilot,
                    label: "Copilot".to_string(),
                    available: true,
                    authenticated: true,
                    message: format!(
                        "GitHub Copilot CLI detected via {} ({}).",
                        binary_source_label(source),
                        version
                    ),
                    detail: "Uses the Copilot SDK with the detected local CLI session. Set COPILOT_CLI_PATH to override CLI discovery; auth errors surface on the first generate.".to_string(),
                    default_model: None,
                })
            }
            Err(error) => Ok(CodeTourProviderStatus {
                provider: CodeTourProvider::Copilot,
                label: "Copilot".to_string(),
                available: false,
                authenticated: false,
                message: "GitHub Copilot CLI was not found.".to_string(),
                detail: format!(
                    "Install the GitHub Copilot CLI, or set COPILOT_CLI_PATH to its full path, then sign in with `copilot login`. SDK resolver error: {error}"
                ),
                default_model: None,
            }),
        }
    }

    fn generate(
        &self,
        input: &GenerateCodeTourInput,
        on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
    ) -> Result<GeneratedCodeTour, String> {
        if !Path::new(&input.working_directory).is_dir() {
            return Err(format!(
                "The local checkout '{}' does not exist.",
                input.working_directory
            ));
        }

        on_progress(make_progress(
            "startup",
            "Starting GitHub Copilot",
            Some(
                "Opening a Copilot SDK session with streamed progress in the prepared local checkout."
                    .to_string(),
            ),
            Some("Starting Copilot SDK session".to_string()),
        ));

        let mut prompt = build_tour_prompt(input);
        if prompt.len() > MAX_PROMPT_BYTES {
            truncate_to_byte_limit(&mut prompt, MAX_PROMPT_BYTES);
        }

        let CopilotRun { outcome } = run_copilot_with_tool_allowlist_retries(
            &input.working_directory,
            &prompt,
            OVERALL_TIMEOUT_MS,
            INACTIVITY_TIMEOUT_MS,
            "the code tour",
            true,
            on_progress,
        )?;

        if let Some(abort) = &outcome.abort {
            if !has_usable_final_text(&outcome) {
                let summary = generation_abort_message("GitHub Copilot", "the code tour", abort);
                on_progress(make_progress(
                    "timeout",
                    summary.clone(),
                    Some(
                        "Aborting the Copilot SDK session so the app can surface the failure without waiting."
                            .to_string(),
                    ),
                    Some(summary.clone()),
                ));
                return Err(summary);
            }
        }

        if let Some(error) = &outcome.error {
            return Err(error.clone());
        }

        on_progress(make_progress(
            "finalizing",
            "GitHub Copilot finished the draft",
            Some(
                "Parsing the structured response and merging it into the final code tour."
                    .to_string(),
            ),
            Some("Finalizing Copilot output".to_string()),
        ));

        let Some(final_text) = outcome.final_text.as_deref() else {
            return Ok(build_copilot_fallback_tour(
                input,
                outcome.model.clone(),
                fallback_reason(&outcome),
            ));
        };

        let trimmed = final_text.trim();
        if trimmed.is_empty() {
            return Ok(build_copilot_fallback_tour(
                input,
                outcome.model.clone(),
                fallback_reason(&outcome),
            ));
        }

        match parse_tolerant::<TourResponse>(trimmed) {
            Ok(response) => Ok(merge_tour(response, input, outcome.model)),
            Err(error) => Ok(build_copilot_fallback_tour(
                input,
                outcome.model,
                format!(
                    "GitHub Copilot did not return a usable JSON code tour: {}",
                    error.message
                ),
            )),
        }
    }
}

pub fn run_json_prompt(
    working_directory: &str,
    prompt: String,
    options: AgentJsonPromptOptions,
) -> Result<AgentTextResponse, String> {
    run_json_prompt_with_progress(working_directory, prompt, options, &mut |_| {})
}

pub fn run_json_prompt_with_progress(
    working_directory: &str,
    mut prompt: String,
    options: AgentJsonPromptOptions,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) -> Result<AgentTextResponse, String> {
    if !Path::new(working_directory).is_dir() {
        return Err(format!(
            "The local checkout '{working_directory}' does not exist."
        ));
    }

    if prompt.len() > MAX_STACK_PLAN_PROMPT_BYTES {
        truncate_to_byte_limit(&mut prompt, MAX_STACK_PLAN_PROMPT_BYTES);
    }

    let CopilotRun { outcome } = run_copilot_with_tool_allowlist_retries(
        working_directory,
        &prompt,
        options.copilot_overall_timeout_ms,
        options.copilot_inactivity_timeout_ms,
        options.task_label,
        true,
        on_progress,
    )?;

    if let Some(abort) = &outcome.abort {
        if !has_usable_final_text(&outcome) {
            return Err(generation_abort_message(
                "GitHub Copilot",
                options.task_label,
                abort,
            ));
        }
    }

    if let Some(error) = &outcome.error {
        return Err(error.clone());
    }

    let Some(final_text) = outcome.final_text.as_deref() else {
        return Err(fallback_reason(&outcome));
    };

    let trimmed = final_text.trim();
    if trimmed.is_empty() {
        return Err(fallback_reason(&outcome));
    }

    Ok(AgentTextResponse {
        text: trimmed.to_string(),
        model: outcome.model,
    })
}

fn run_copilot_with_tool_allowlist_retries(
    working_directory: &str,
    prompt: &str,
    overall_timeout_ms: u64,
    inactivity_timeout_ms: u64,
    task_label: &str,
    emit_progress: bool,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) -> Result<CopilotRun, String> {
    let mut last_tool_error: Option<String> = None;

    for (attempt_ix, available_tools) in COPILOT_TOOL_ALLOWLISTS.iter().enumerate() {
        if attempt_ix > 0 && emit_progress {
            on_progress(make_progress(
                "startup",
                "Retrying GitHub Copilot with compatible tools",
                Some(
                    "The installed Copilot CLI rejected the previous read-only tool allowlist."
                        .to_string(),
                ),
                Some(format!(
                    "Retrying Copilot with tools: {}",
                    available_tools.join(",")
                )),
            ));
        }

        let run = runtime::shared().block_on(run_copilot_sdk_session(
            working_directory,
            prompt,
            available_tools,
            overall_timeout_ms,
            inactivity_timeout_ms,
            task_label,
            emit_progress,
            on_progress,
        ))?;

        if run
            .outcome
            .error
            .as_deref()
            .map(is_unknown_tool_allowlist_error)
            .unwrap_or(false)
            && !has_usable_final_text(&run.outcome)
        {
            last_tool_error = run.outcome.error.clone();
            continue;
        }

        return Ok(run);
    }

    Err(last_tool_error.unwrap_or_else(|| {
        "GitHub Copilot CLI rejected every configured read-only tool allowlist.".to_string()
    }))
}

#[allow(clippy::too_many_arguments)]
async fn run_copilot_sdk_session(
    working_directory: &str,
    prompt: &str,
    available_tools: &[&str],
    overall_timeout_ms: u64,
    inactivity_timeout_ms: u64,
    task_label: &str,
    emit_progress: bool,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) -> Result<CopilotRun, String> {
    let client_options = ClientOptions::new()
        .with_cwd(working_directory)
        .with_log_level(LogLevel::Error)
        .with_use_logged_in_user(true)
        .with_session_idle_timeout_seconds((overall_timeout_ms / 1000).saturating_add(60));

    let client = Client::start(client_options)
        .await
        .map_err(format_sdk_start_error)?;

    let mut stop_client = true;
    let result = async {
        let session_config = SessionConfig::default()
            .with_client_name("Remiss")
            .with_streaming(true)
            .with_available_tools(available_tools.iter().copied())
            .with_enable_config_discovery(false)
            .with_request_user_input(false)
            .with_request_permission(true)
            .with_request_exit_plan_mode(false)
            .with_request_auto_mode_switch(false)
            .with_request_elicitation(false)
            .approve_permissions_if(is_read_permission_request);

        let session = client
            .create_session(session_config)
            .await
            .map_err(format_sdk_session_error)?;
        let mut events = session.subscribe();

        if emit_progress {
            on_progress(make_progress(
                "running",
                "GitHub Copilot is inspecting the checkout",
                Some("Waiting for streamed Copilot SDK events from the linked repository.".to_string()),
                Some(format!(
                    "Waiting for Copilot SDK event stream ({})",
                    available_tools.join(",")
                )),
            ));
        }

        let mut outcome = CopilotOutcome::default();
        let start = StdInstant::now();
        let mut last_activity = TokioInstant::now();
        let mut overall_sleep = Box::pin(tokio::time::sleep_until(
            TokioInstant::now() + Duration::from_millis(overall_timeout_ms),
        ));
        let mut inactivity_sleep = Box::pin(tokio::time::sleep_until(
            last_activity + Duration::from_millis(inactivity_timeout_ms),
        ));
        let mut ticker = tokio::time::interval(Duration::from_millis(RUNNING_TICKER_MS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let send = session.send_and_wait(
            MessageOptions::new(prompt.to_string())
                .with_wait_timeout(Duration::from_millis(overall_timeout_ms)),
        );
        tokio::pin!(send);

        let send_result = loop {
            tokio::select! {
                result = &mut send => {
                    break Some(result);
                }
                event = events.recv() => {
                    match event {
                        Ok(event) => {
                            handle_sdk_event(event, &mut outcome, emit_progress, on_progress);
                            last_activity = TokioInstant::now();
                            inactivity_sleep.as_mut().reset(
                                last_activity + Duration::from_millis(inactivity_timeout_ms)
                            );
                            if outcome_has_unknown_tool_allowlist_error(&outcome)
                                && !has_usable_final_text(&outcome)
                            {
                                let _ = session.abort().await;
                                break None;
                            }
                        }
                        Err(RecvError::Lagged(lagged)) => {
                            outcome.last_visible_activity = Some(format!(
                                "Skipped {} Copilot SDK event(s) while processing progress.",
                                lagged.skipped()
                            ));
                            last_activity = TokioInstant::now();
                            inactivity_sleep.as_mut().reset(
                                last_activity + Duration::from_millis(inactivity_timeout_ms)
                            );
                        }
                        Err(RecvError::Closed) => {
                            outcome.error = Some(
                                "GitHub Copilot SDK event stream closed before completion.".to_string(),
                            );
                            break None;
                        }
                        Err(error) => {
                            outcome.error = Some(format!(
                                "GitHub Copilot SDK event stream failed: {error}"
                            ));
                            break None;
                        }
                    }
                }
                _ = &mut overall_sleep => {
                    outcome.abort = Some(AbortReason {
                        kind: AbortKind::Overall,
                        timeout_ms: overall_timeout_ms,
                        last_visible_activity: outcome.last_visible_activity.clone(),
                    });
                    let _ = session.abort().await;
                    break None;
                }
                _ = &mut inactivity_sleep => {
                    outcome.abort = Some(AbortReason {
                        kind: AbortKind::Inactivity,
                        timeout_ms: inactivity_timeout_ms,
                        last_visible_activity: outcome.last_visible_activity.clone(),
                    });
                    let _ = session.abort().await;
                    break None;
                }
                _ = ticker.tick() => {
                    if emit_progress {
                        let elapsed_s = start.elapsed().as_secs();
                        on_progress(make_progress(
                            "running",
                            "GitHub Copilot is still working",
                            Some(format!("Elapsed: {elapsed_s}s.")),
                            outcome.last_visible_activity.clone(),
                        ));
                    }
                }
            }
        };

        if let Some(result) = send_result {
            match result {
                Ok(Some(final_event)) => {
                    handle_sdk_event(final_event, &mut outcome, emit_progress, on_progress);
                }
                Ok(None) => {}
                Err(error) => {
                    handle_sdk_send_error(error, &mut outcome, task_label);
                }
            }
        }

        if let Ok(events) = session.get_messages().await {
            for event in events {
                record_final_text_from_event(&event, &mut outcome);
            }
        }
        promote_stream_to_final(&mut outcome);
        let _ = session.disconnect().await;
        Ok(CopilotRun { outcome })
    }
    .await;

    match tokio::time::timeout(Duration::from_secs(5), client.stop()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => {
            client.force_stop();
            stop_client = false;
        }
    }

    if stop_client {
        drop(client);
    }

    result
}

fn handle_sdk_event(
    event: SessionEvent,
    outcome: &mut CopilotOutcome,
    emit_progress: bool,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    match event.event_type.as_str() {
        "session.tools_updated" => {
            if let Some(model) = first_string(&event.data, &["model"]) {
                outcome.model = Some(model);
            }
        }
        "assistant.turn_start" => {
            outcome.saw_meaningful_progress = true;
            outcome.last_visible_activity = Some("Started Copilot turn".to_string());
            if emit_progress {
                on_progress(make_progress(
                    "turn",
                    "GitHub Copilot is inspecting the checkout",
                    Some(
                        "Walking the changed files and related callsites from the checkout."
                            .to_string(),
                    ),
                    Some("Started Copilot turn".to_string()),
                ));
            }
        }
        "assistant.intent" => {
            if let Some(intent) = first_string(&event.data, &["intent", "message", "content"]) {
                let detail = limit_text(intent.trim(), 240);
                outcome.saw_meaningful_progress = true;
                outcome.last_visible_activity = Some(detail.clone());
                if emit_progress {
                    on_progress(make_progress(
                        "intent",
                        "GitHub Copilot is inspecting the checkout",
                        Some(detail.clone()),
                        Some(detail),
                    ));
                }
            }
        }
        "assistant.reasoning" => {
            if let Some(content) = first_string(&event.data, &["content"]) {
                record_reasoning_progress(content, outcome, emit_progress, on_progress);
            }
        }
        "assistant.reasoning_delta" => {
            if let Some(delta) =
                first_string(&event.data, &["deltaContent", "delta_content", "delta"])
            {
                record_reasoning_progress(delta, outcome, emit_progress, on_progress);
            }
        }
        "assistant.message_delta" => {
            if let Some(delta) = first_string(
                &event.data,
                &["deltaContent", "delta_content", "delta", "content"],
            ) {
                outcome.current_turn_stream.push_str(&delta);
                let trimmed = delta.trim();
                if !trimmed.is_empty() {
                    outcome.saw_meaningful_progress = true;
                    outcome.last_visible_activity =
                        Some(format!("Drafting: {}", limit_text(trimmed, 160)));
                    if emit_progress {
                        on_progress(make_progress(
                            "drafting",
                            "GitHub Copilot is drafting the response",
                            Some(limit_text(trimmed, 240)),
                            Some("Drafting Copilot response".to_string()),
                        ));
                    }
                }
            }
        }
        "assistant.message_start" => {
            outcome.current_turn_stream.clear();
        }
        "assistant.message" => {
            if let Some(content) = first_string(&event.data, &["content", "message", "text"]) {
                if !event_has_tool_requests(&event.data) || text_contains_json_payload(&content) {
                    record_final_text_candidate(outcome, content);
                }
                outcome.current_turn_stream.clear();
                outcome.saw_meaningful_progress = true;
                outcome.last_visible_activity = Some(if event_has_tool_requests(&event.data) {
                    "Copilot requested repository tools".to_string()
                } else {
                    "Copilot returned an assistant message".to_string()
                });
            }
            if let Some(model) = first_string(&event.data, &["model"]) {
                outcome.model = Some(model);
            }
        }
        "session.task_complete" => {
            if let Some(summary) = first_string(&event.data, &["summary"]) {
                let trimmed = summary.trim();
                if text_contains_json_payload(trimmed) {
                    record_final_text_candidate(outcome, trimmed.to_string());
                }
                if !trimmed.is_empty() {
                    outcome.saw_meaningful_progress = true;
                    outcome.last_visible_activity =
                        Some(format!("Task complete: {}", limit_text(trimmed, 160)));
                }
            }
        }
        "tool.execution_start" => {
            let tool_name = first_string(&event.data, &["toolName", "tool_name", "name"])
                .unwrap_or_else(|| "tool".to_string());
            if let Some(tool_call_id) = first_string(&event.data, &["toolCallId", "tool_call_id"]) {
                outcome.tool_names.insert(tool_call_id, tool_name.clone());
            }
            outcome.saw_meaningful_progress = true;
            outcome.last_visible_activity = Some(format!("Tool: {tool_name}"));
            if emit_progress {
                on_progress(make_progress(
                    "tool",
                    "GitHub Copilot is using a repository tool",
                    Some(tool_detail(&tool_name, &event.data)),
                    Some(format!("Tool: {tool_name}")),
                ));
            }
        }
        "tool.execution_progress" => {
            let detail = first_string(
                &event.data,
                &["progressMessage", "progress_message", "message"],
            )
            .unwrap_or_else(|| "Repository tool is still running.".to_string());
            let tool_name = tool_name_for_event(outcome, &event.data);
            outcome.saw_meaningful_progress = true;
            outcome.last_visible_activity =
                Some(format!("{tool_name}: {}", limit_text(&detail, 160)));
            if emit_progress {
                on_progress(make_progress(
                    "tool",
                    "GitHub Copilot is using a repository tool",
                    Some(limit_text(&detail, 240)),
                    Some(format!("{tool_name}: {}", limit_text(&detail, 120))),
                ));
            }
        }
        "tool.execution_complete" => {
            let tool_name = tool_name_for_event(outcome, &event.data);
            let success = event
                .data
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if let Some(model) = first_string(&event.data, &["model"]) {
                outcome.model = Some(model);
            }
            outcome.saw_meaningful_progress = true;
            if success {
                outcome.last_visible_activity = Some(format!("Tool finished: {tool_name}"));
            } else {
                let detail = event
                    .data
                    .get("error")
                    .and_then(|error| first_string(error, &["message"]))
                    .unwrap_or_else(|| format!("Tool failed: {tool_name}"));
                outcome.last_visible_activity = Some(limit_text(&detail, 180));
                if emit_progress {
                    on_progress(make_progress(
                        "tool_failed",
                        "A Copilot repository tool step failed",
                        Some(limit_text(&detail, 240)),
                        Some(format!("Tool failed: {tool_name}")),
                    ));
                }
            }
        }
        "session.error" => {
            let message = first_string(&event.data, &["message", "error"])
                .unwrap_or_else(|| "GitHub Copilot reported an error.".to_string());
            outcome.error = Some(format_copilot_error_message(&message));
            outcome.last_visible_activity = Some(message.clone());
            if emit_progress {
                on_progress(make_progress(
                    "error",
                    "GitHub Copilot reported an error",
                    Some(limit_text(&message, 240)),
                    Some(limit_text(&message, 180)),
                ));
            }
        }
        "session.idle" => {
            outcome.saw_meaningful_progress = true;
            outcome.last_visible_activity = Some("Copilot session is idle".to_string());
            if emit_progress {
                on_progress(make_progress(
                    "finalizing",
                    "GitHub Copilot finished gathering context",
                    Some("Formatting the structured response.".to_string()),
                    Some("Copilot session is idle".to_string()),
                ));
            }
        }
        _ => {}
    }
}

fn record_reasoning_progress(
    text: String,
    outcome: &mut CopilotOutcome,
    emit_progress: bool,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    let detail = limit_text(trimmed, 240);
    outcome.saw_meaningful_progress = true;
    outcome.last_visible_activity = Some(detail.clone());
    if emit_progress {
        on_progress(make_progress(
            "reasoning",
            "GitHub Copilot is reasoning through the change",
            Some(detail.clone()),
            Some(limit_text(trimmed, 180)),
        ));
    }
}

fn handle_sdk_send_error(error: SdkError, outcome: &mut CopilotOutcome, task_label: &str) {
    match error {
        SdkError::Session(SessionError::Timeout(timeout)) => {
            let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            outcome.abort = Some(AbortReason {
                kind: AbortKind::Overall,
                timeout_ms,
                last_visible_activity: outcome.last_visible_activity.clone(),
            });
        }
        SdkError::Session(SessionError::AgentError(message)) => {
            outcome.error = Some(format_copilot_error_message(&message));
        }
        other => {
            outcome.error = Some(format!(
                "GitHub Copilot SDK failed during {task_label}: {}",
                format_sdk_error(other)
            ));
        }
    }
}

fn format_sdk_start_error(error: SdkError) -> String {
    let message = format_sdk_error(error);
    if message.to_ascii_lowercase().contains("binary not found") {
        format!(
            "GitHub Copilot CLI was not found. Install it or set COPILOT_CLI_PATH. SDK resolver error: {message}"
        )
    } else {
        format_copilot_error_message(&message)
    }
}

fn format_sdk_session_error(error: SdkError) -> String {
    format_copilot_error_message(&format_sdk_error(error))
}

fn format_sdk_error(error: SdkError) -> String {
    error.to_string()
}

fn format_copilot_error_message(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("auth")
        || lower.contains("login")
        || lower.contains("logged in")
        || lower.contains("unauthorized")
    {
        format!(
            "GitHub Copilot is not authenticated. Run `copilot login` in your shell, then retry. Details: {message}"
        )
    } else {
        format!("GitHub Copilot reported an error: {message}")
    }
}

fn is_read_permission_request(data: &PermissionRequestData) -> bool {
    matches!(data.kind, Some(PermissionRequestKind::Read))
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn tool_name_for_event(outcome: &CopilotOutcome, data: &Value) -> String {
    if let Some(tool_name) = first_string(data, &["toolName", "tool_name", "name"]) {
        return tool_name;
    }
    first_string(data, &["toolCallId", "tool_call_id"])
        .and_then(|tool_call_id| outcome.tool_names.get(&tool_call_id).cloned())
        .unwrap_or_else(|| "tool".to_string())
}

fn tool_detail(tool_name: &str, data: &Value) -> String {
    let Some(arguments) = data.get("arguments") else {
        return tool_name.to_string();
    };
    let argument_text = if let Some(text) = arguments.as_str() {
        text.to_string()
    } else {
        arguments.to_string()
    };
    format!("{tool_name}: {}", limit_text(&argument_text, 240))
}

fn event_has_tool_requests(data: &Value) -> bool {
    ["toolRequests", "tool_requests"].iter().any(|key| {
        data.get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn record_final_text_from_event(event: &SessionEvent, outcome: &mut CopilotOutcome) {
    match event.event_type.as_str() {
        "assistant.message" => {
            if let Some(content) = first_string(&event.data, &["content", "message", "text"]) {
                if !event_has_tool_requests(&event.data) || text_contains_json_payload(&content) {
                    record_final_text_candidate(outcome, content);
                }
            }
            if let Some(model) = first_string(&event.data, &["model"]) {
                outcome.model = Some(model);
            }
        }
        "session.task_complete" => {
            if let Some(summary) = first_string(&event.data, &["summary"]) {
                if text_contains_json_payload(&summary) {
                    record_final_text_candidate(outcome, summary);
                }
            }
        }
        _ => {}
    }
}

fn record_final_text_candidate(outcome: &mut CopilotOutcome, content: String) {
    let candidate = content.trim();
    if candidate.is_empty() {
        return;
    }

    if text_contains_json_payload(candidate) {
        outcome.final_text = Some(candidate.to_string());
    }
}

fn text_contains_json_payload(text: &str) -> bool {
    parse_tolerant::<Value>(text).is_ok()
}

fn has_usable_final_text(outcome: &CopilotOutcome) -> bool {
    outcome
        .final_text
        .as_deref()
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false)
}

fn promote_stream_to_final(outcome: &mut CopilotOutcome) {
    if outcome.final_text.is_none() {
        let stream = outcome.current_turn_stream.trim();
        if !stream.is_empty() && text_contains_json_payload(stream) {
            outcome.final_text = Some(stream.to_string());
            outcome.current_turn_stream.clear();
        }
    }
}

fn fallback_reason(outcome: &CopilotOutcome) -> String {
    if let Some(error) = &outcome.error {
        return error.clone();
    }
    if let Some(abort) = &outcome.abort {
        return generation_abort_message("GitHub Copilot", "the code tour", abort);
    }
    if !outcome.saw_meaningful_progress {
        return "GitHub Copilot did not emit any streamed SDK progress before ending.".to_string();
    }
    outcome
        .last_visible_activity
        .clone()
        .unwrap_or_else(|| "GitHub Copilot returned no final assistant message.".to_string())
}

fn outcome_has_unknown_tool_allowlist_error(outcome: &CopilotOutcome) -> bool {
    outcome
        .error
        .as_deref()
        .map(is_unknown_tool_allowlist_error)
        .unwrap_or(false)
}

fn is_unknown_tool_allowlist_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let mentions_tool = lower.contains("tool");
    let mentions_allowlist =
        lower.contains("allowlist") || lower.contains("available") || lower.contains("unknown");
    mentions_tool && mentions_allowlist
}

fn truncate_to_byte_limit(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }

    let mut cutoff = max_bytes;
    while !text.is_char_boundary(cutoff) {
        cutoff -= 1;
    }
    text.truncate(cutoff);
    text.push_str(
        "\n\n[Remiss truncated the prompt to fit the GitHub Copilot SDK request budget.]",
    );
}

fn probe_version(binary: &Path) -> Option<String> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr)
    } else {
        String::from_utf8_lossy(&output.stdout)
    };
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn binary_source_label(source: BinarySource) -> &'static str {
    match source {
        BinarySource::Bundled => "embedded CLI",
        BinarySource::EnvOverride => "COPILOT_CLI_PATH",
        BinarySource::Local => "local install",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_session_events_into_progress_and_final_text() {
        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();

        for event in [
            event("assistant.turn_start", json!({ "turnId": "1" })),
            event(
                "assistant.intent",
                json!({ "intent": "Inspecting changed files" }),
            ),
            event(
                "tool.execution_start",
                json!({ "toolCallId": "tool-1", "toolName": "rg", "arguments": { "query": "foo" } }),
            ),
            event(
                "tool.execution_progress",
                json!({ "toolCallId": "tool-1", "progressMessage": "Searching src" }),
            ),
            event(
                "tool.execution_complete",
                json!({ "toolCallId": "tool-1", "success": true, "model": "test-model" }),
            ),
            event(
                "assistant.message_delta",
                json!({ "messageId": "m1", "deltaContent": "{\"answer\"" }),
            ),
            event(
                "assistant.message",
                json!({ "messageId": "m1", "content": "{\"answer\":true}", "model": "test-model" }),
            ),
            event("session.idle", json!({})),
        ] {
            handle_sdk_event(event, &mut outcome, true, &mut |update| {
                progress.push(update);
            });
        }

        assert_eq!(outcome.final_text.as_deref(), Some("{\"answer\":true}"));
        assert_eq!(outcome.model.as_deref(), Some("test-model"));
        assert!(outcome.saw_meaningful_progress);
        assert!(progress.iter().any(|update| update.stage == "turn"));
        assert!(progress.iter().any(|update| update.stage == "tool"));
        assert!(progress.iter().any(|update| update.stage == "drafting"));
        assert!(progress.iter().any(|update| update.stage == "finalizing"));
    }

    #[test]
    fn treats_tool_request_messages_as_progress_not_final_json() {
        let mut outcome = CopilotOutcome::default();

        handle_sdk_event(
            event(
                "assistant.message",
                json!({
                    "messageId": "m1",
                    "content": "I'll inspect the current PR diff first.",
                    "toolRequests": [
                        {
                            "toolCallId": "tool-1",
                            "name": "view"
                        }
                    ]
                }),
            ),
            &mut outcome,
            false,
            &mut |_| {},
        );
        promote_stream_to_final(&mut outcome);

        assert!(outcome.final_text.is_none());
        assert_eq!(
            outcome.last_visible_activity.as_deref(),
            Some("Copilot requested repository tools")
        );
    }

    #[test]
    fn task_complete_summary_can_supply_json_payload() {
        let mut outcome = CopilotOutcome::default();

        handle_sdk_event(
            event(
                "assistant.message",
                json!({
                    "messageId": "m1",
                    "content": "I'll inspect the current PR diff first.",
                    "toolRequests": [{ "toolCallId": "tool-1", "name": "view" }]
                }),
            ),
            &mut outcome,
            false,
            &mut |_| {},
        );
        handle_sdk_event(
            event(
                "session.task_complete",
                json!({ "success": true, "summary": "{\"ok\":true}" }),
            ),
            &mut outcome,
            false,
            &mut |_| {},
        );

        assert_eq!(outcome.final_text.as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn message_start_resets_stream_before_promoting_json_delta() {
        let mut outcome = CopilotOutcome::default();

        handle_sdk_event(
            event(
                "assistant.message_delta",
                json!({ "messageId": "m1", "deltaContent": "I'll inspect first." }),
            ),
            &mut outcome,
            false,
            &mut |_| {},
        );
        handle_sdk_event(
            event("assistant.message_start", json!({ "messageId": "m2" })),
            &mut outcome,
            false,
            &mut |_| {},
        );
        handle_sdk_event(
            event(
                "assistant.message_delta",
                json!({ "messageId": "m2", "deltaContent": "{\"ok\":true}" }),
            ),
            &mut outcome,
            false,
            &mut |_| {},
        );
        promote_stream_to_final(&mut outcome);

        assert_eq!(outcome.final_text.as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn maps_session_error_to_visible_provider_error() {
        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();

        handle_sdk_event(
            event(
                "session.error",
                json!({ "errorType": "authentication", "message": "not logged in" }),
            ),
            &mut outcome,
            true,
            &mut |update| progress.push(update),
        );

        assert!(outcome.error.as_deref().unwrap().contains("copilot login"));
        assert_eq!(
            progress.last().map(|update| update.stage.as_str()),
            Some("error")
        );
    }

    #[test]
    fn read_only_permission_policy_allows_only_read_requests() {
        let read = PermissionRequestData {
            kind: Some(PermissionRequestKind::Read),
            tool_call_id: None,
            extra: Value::Null,
        };
        let write = PermissionRequestData {
            kind: Some(PermissionRequestKind::Write),
            tool_call_id: None,
            extra: Value::Null,
        };
        let shell = PermissionRequestData {
            kind: Some(PermissionRequestKind::Shell),
            tool_call_id: None,
            extra: Value::Null,
        };
        let unknown = PermissionRequestData {
            kind: Some(PermissionRequestKind::Unknown),
            tool_call_id: None,
            extra: Value::Null,
        };

        assert!(is_read_permission_request(&read));
        assert!(!is_read_permission_request(&write));
        assert!(!is_read_permission_request(&shell));
        assert!(!is_read_permission_request(&unknown));
    }

    #[test]
    fn tool_allowlist_retries_preserve_expected_order() {
        assert_eq!(
            COPILOT_TOOL_ALLOWLISTS,
            &[
                &["view", "rg", "glob"][..],
                &["view", "grep", "glob"][..],
                &["view", "glob"][..],
            ]
        );
    }

    #[test]
    fn promotes_delta_stream_when_final_message_is_missing() {
        let mut outcome = CopilotOutcome::default();
        handle_sdk_event(
            event(
                "assistant.message_delta",
                json!({ "messageId": "m1", "deltaContent": "{\"ok\":true}" }),
            ),
            &mut outcome,
            false,
            &mut |_| {},
        );

        promote_stream_to_final(&mut outcome);

        assert_eq!(outcome.final_text.as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn rejects_unknown_tool_allowlist_errors_for_retry() {
        assert!(is_unknown_tool_allowlist_error(
            "unknown tool in available tools allowlist: rg"
        ));
        assert!(!is_unknown_tool_allowlist_error("authentication failed"));
    }

    fn event(event_type: &str, data: Value) -> SessionEvent {
        SessionEvent {
            id: format!("event-{event_type}"),
            timestamp: "2026-05-18T00:00:00Z".to_string(),
            parent_id: None,
            ephemeral: None,
            agent_id: None,
            debug_cli_received_at_ms: None,
            debug_ws_forwarded_at_ms: None,
            event_type: event_type.to_string(),
            data,
        }
    }
}
