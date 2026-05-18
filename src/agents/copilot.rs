use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant as StdInstant, SystemTime, UNIX_EPOCH};

use github_copilot_sdk::resolve::{copilot_binary_with_source, BinarySource};
use github_copilot_sdk::{
    Client, ClientOptions, Error as SdkError, LogLevel, MessageOptions, PermissionRequestData,
    PermissionRequestKind, RecvError, SessionConfig, SessionError, SessionEvent,
};
use serde_json::{json, Value};
use tokio::time::{Instant as TokioInstant, MissedTickBehavior};

use crate::{
    app_storage,
    code_tour::{
        CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
        GeneratedCodeTour,
    },
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
const COPILOT_DIAGNOSTICS_INCLUDE_PROMPT_ENV: &str = "REMISS_COPILOT_DIAGNOSTICS_INCLUDE_PROMPT";
const COPILOT_DIAGNOSTIC_LOG_DIR: &str = "copilot-diagnostics";
const MAX_DIAGNOSTIC_EVENTS: usize = 240;
const MAX_DIAGNOSTIC_LINE_CHARS: usize = 4_000;
const MAX_DIAGNOSTIC_RESPONSE_CHARS: usize = 80_000;
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
    saw_tool_request_message: bool,
    current_turn_stream: String,
    tool_names: HashMap<String, String>,
}

struct CopilotRun {
    outcome: CopilotOutcome,
    diagnostic_log_path: Option<String>,
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

        let CopilotRun {
            outcome,
            diagnostic_log_path,
        } = run_copilot_with_tool_allowlist_retries(
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
                let summary = append_diagnostic_log_suffix(
                    generation_abort_message("GitHub Copilot", "the code tour", abort),
                    diagnostic_log_path.as_deref(),
                );
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
            return Err(append_diagnostic_log_suffix(
                error.clone(),
                diagnostic_log_path.as_deref(),
            ));
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
                append_diagnostic_log_suffix(
                    fallback_reason(&outcome),
                    diagnostic_log_path.as_deref(),
                ),
            ));
        };

        let trimmed = final_text.trim();
        if trimmed.is_empty() {
            return Ok(build_copilot_fallback_tour(
                input,
                outcome.model.clone(),
                append_diagnostic_log_suffix(
                    fallback_reason(&outcome),
                    diagnostic_log_path.as_deref(),
                ),
            ));
        }

        match parse_tolerant::<TourResponse>(trimmed) {
            Ok(response) => Ok(merge_tour(response, input, outcome.model)),
            Err(error) => Ok(build_copilot_fallback_tour(
                input,
                outcome.model,
                append_diagnostic_log_suffix(
                    format!(
                        "GitHub Copilot did not return a usable JSON code tour: {}",
                        error.message
                    ),
                    diagnostic_log_path.as_deref(),
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

    let CopilotRun {
        outcome,
        diagnostic_log_path,
    } = runtime::shared().block_on(run_copilot_sdk_session(
        working_directory,
        &prompt,
        &[],
        options.copilot_overall_timeout_ms,
        options.copilot_inactivity_timeout_ms,
        options.task_label,
        true,
        on_progress,
        1,
        1,
    ))?;

    if let Some(abort) = &outcome.abort {
        if !has_usable_final_text(&outcome) {
            return Err(append_diagnostic_log_suffix(
                generation_abort_message("GitHub Copilot", options.task_label, abort),
                diagnostic_log_path.as_deref(),
            ));
        }
    }

    if let Some(error) = &outcome.error {
        return Err(append_diagnostic_log_suffix(
            error.clone(),
            diagnostic_log_path.as_deref(),
        ));
    }

    let Some(final_text) = outcome.final_text.as_deref() else {
        return Err(append_diagnostic_log_suffix(
            fallback_reason(&outcome),
            diagnostic_log_path.as_deref(),
        ));
    };

    let trimmed = final_text.trim();
    if trimmed.is_empty() {
        return Err(append_diagnostic_log_suffix(
            fallback_reason(&outcome),
            diagnostic_log_path.as_deref(),
        ));
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

    let max_attempts = COPILOT_TOOL_ALLOWLISTS.len();
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
            attempt_ix + 1,
            max_attempts,
        ))?;

        if run
            .outcome
            .error
            .as_deref()
            .map(is_unknown_tool_allowlist_error)
            .unwrap_or(false)
            && !has_usable_final_text(&run.outcome)
        {
            last_tool_error = run.outcome.error.clone().map(|error| {
                append_diagnostic_log_suffix(error, run.diagnostic_log_path.as_deref())
            });
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
    attempt: usize,
    max_attempts: usize,
) -> Result<CopilotRun, String> {
    let started_at_ms = now_ms();
    let run_started_at = StdInstant::now();
    let binary = copilot_binary_with_source()
        .ok()
        .map(|(path, _)| path.display().to_string());
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
            let (summary, detail, activity) = if available_tools.is_empty() {
                (
                    "GitHub Copilot is drafting the response",
                    "Waiting for streamed Copilot SDK events without repository tools.",
                    "Waiting for Copilot SDK event stream (repository tools disabled)".to_string(),
                )
            } else {
                (
                    "GitHub Copilot is inspecting the checkout",
                    "Waiting for streamed Copilot SDK events from the linked repository.",
                    format!(
                        "Waiting for Copilot SDK event stream ({})",
                        available_tools.join(",")
                    ),
                )
            };
            on_progress(make_progress(
                "running",
                summary,
                Some(detail.to_string()),
                Some(activity),
            ));
        }

        let mut outcome = CopilotOutcome::default();
        let mut diagnostic_events = Vec::new();
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
                            record_diagnostic_event(&mut diagnostic_events, "sdk", &event);
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
                            record_diagnostic_line(
                                &mut diagnostic_events,
                                "sdk-lagged",
                                &format!("skipped {}", lagged.skipped()),
                            );
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
                    record_diagnostic_event(&mut diagnostic_events, "sdk-final", &final_event);
                    handle_terminal_sdk_event(
                        final_event,
                        &mut outcome,
                        emit_progress,
                        on_progress,
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    handle_sdk_send_error(error, &mut outcome, task_label);
                }
            }
        }

        if let Ok(events) = session.get_messages().await {
            for event in events {
                record_diagnostic_event(&mut diagnostic_events, "sdk-history", &event);
                record_final_text_from_event(&event, &mut outcome);
            }
        }
        promote_stream_to_final(&mut outcome);
        let duration_ms = u64::try_from(run_started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let diagnostic_log_path = write_copilot_diagnostic_log(CopilotDiagnosticLogInput {
            started_at_ms,
            duration_ms,
            task_label,
            binary: binary.as_deref(),
            working_directory,
            available_tools,
            overall_timeout_ms,
            inactivity_timeout_ms,
            prompt,
            outcome: &outcome,
            stream_events: &diagnostic_events,
            attempt,
            max_attempts,
        });
        let _ = session.disconnect().await;
        Ok(CopilotRun {
            outcome,
            diagnostic_log_path,
        })
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
                let has_tool_requests = event_has_tool_requests(&event.data);
                if has_tool_requests {
                    outcome.saw_tool_request_message = true;
                }
                if has_tool_requests || !content.trim().is_empty() {
                    outcome.saw_meaningful_progress = true;
                }
                if !has_tool_requests || text_contains_json_payload(&content) {
                    record_final_text_candidate(outcome, content);
                }
                outcome.current_turn_stream.clear();
                outcome.saw_meaningful_progress = true;
                outcome.last_visible_activity = Some(if has_tool_requests {
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

fn handle_terminal_sdk_event(
    event: SessionEvent,
    outcome: &mut CopilotOutcome,
    emit_progress: bool,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    match event.event_type.as_str() {
        "assistant.message" | "session.task_complete" => {
            record_final_text_from_event(&event, outcome);
        }
        _ => handle_sdk_event(event, outcome, emit_progress, on_progress),
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
                let has_tool_requests = event_has_tool_requests(&event.data);
                if has_tool_requests {
                    outcome.saw_tool_request_message = true;
                }
                if has_tool_requests || !content.trim().is_empty() {
                    outcome.saw_meaningful_progress = true;
                }
                if !has_tool_requests || text_contains_json_payload(&content) {
                    record_final_text_candidate(outcome, content);
                }
            }
            if let Some(model) = first_string(&event.data, &["model"]) {
                outcome.model = Some(model);
            }
        }
        "session.task_complete" => {
            if let Some(summary) = first_string(&event.data, &["summary"]) {
                if !summary.trim().is_empty() {
                    outcome.saw_meaningful_progress = true;
                }
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
    if outcome.saw_tool_request_message {
        return "GitHub Copilot ended after requesting repository tools without returning the required JSON response.".to_string();
    }
    if outcome.last_visible_activity.as_deref() == Some("Copilot session is idle") {
        return "GitHub Copilot finished without returning the required JSON response.".to_string();
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

struct CopilotDiagnosticLogInput<'a> {
    started_at_ms: u64,
    duration_ms: u64,
    task_label: &'a str,
    binary: Option<&'a str>,
    working_directory: &'a str,
    available_tools: &'a [&'a str],
    overall_timeout_ms: u64,
    inactivity_timeout_ms: u64,
    prompt: &'a str,
    outcome: &'a CopilotOutcome,
    stream_events: &'a [String],
    attempt: usize,
    max_attempts: usize,
}

fn write_copilot_diagnostic_log(input: CopilotDiagnosticLogInput<'_>) -> Option<String> {
    let log_dir = app_storage::data_dir_root().join(COPILOT_DIAGNOSTIC_LOG_DIR);
    if let Err(error) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "Failed to create Copilot diagnostic log directory '{}': {error}",
            log_dir.display()
        );
        return None;
    }

    let task_slug = sanitize_log_path_component(input.task_label);
    let path = log_dir.join(format!(
        "{}-{}-attempt-{}.json",
        input.started_at_ms, task_slug, input.attempt
    ));
    let latest_path = log_dir.join("latest.json");
    let task_latest_path = log_dir.join(copilot_diagnostic_task_latest_file_name(input.task_label));
    let problem_latest_path =
        diagnostic_log_has_problem(input.outcome).then(|| log_dir.join("latest-problem.json"));
    let response_text = diagnostic_response_text(input.outcome);
    let prompt_included = copilot_diagnostics_include_prompt();
    let abort = input.outcome.abort.as_ref().map(|reason| {
        json!({
            "kind": abort_kind_label(reason.kind),
            "timeoutMs": reason.timeout_ms,
            "lastVisibleActivity": reason.last_visible_activity,
        })
    });
    let available_tools = if input.available_tools.is_empty() {
        "none".to_string()
    } else {
        input.available_tools.join(",")
    };
    let path_display = path.display().to_string();
    let latest_path_display = latest_path.display().to_string();
    let task_latest_path_display = task_latest_path.display().to_string();
    let problem_latest_path_display = problem_latest_path
        .as_ref()
        .map(|path| path.display().to_string());

    let log = json!({
        "diagnosticLogPath": path_display,
        "latestLogPath": latest_path_display,
        "taskLatestLogPath": task_latest_path_display,
        "problemLatestLogPath": problem_latest_path_display,
        "startedAtMs": input.started_at_ms,
        "durationMs": input.duration_ms,
        "taskLabel": input.task_label,
        "binary": input.binary,
        "workingDirectory": input.working_directory,
        "availableTools": available_tools,
        "attempt": input.attempt,
        "maxAttempts": input.max_attempts,
        "timeouts": {
            "overallMs": input.overall_timeout_ms,
            "inactivityMs": input.inactivity_timeout_ms,
        },
        "model": input.outcome.model,
        "exitStatusCode": null,
        "abort": abort,
        "error": input.outcome.error,
        "lastVisibleActivity": input.outcome.last_visible_activity,
        "sawMeaningfulProgress": input.outcome.saw_meaningful_progress,
        "stderr": "",
        "finalTextBytes": input.outcome.final_text.as_deref().map(str::len),
        "currentTurnStreamBytes": response_text.map(str::len).unwrap_or(0),
        "currentTurnStreamPreview": response_text.map(limit_diagnostic_text).unwrap_or_default(),
        "streamEvents": input.stream_events,
        "promptBytes": input.prompt.len(),
        "promptIncluded": prompt_included,
        "prompt": if prompt_included {
            Some(input.prompt)
        } else {
            None
        },
    });

    let serialized = match serde_json::to_string_pretty(&log) {
        Ok(serialized) => serialized,
        Err(error) => {
            eprintln!("Failed to serialize Copilot diagnostic log: {error}");
            return None;
        }
    };

    if let Err(error) = std::fs::write(&path, &serialized) {
        eprintln!(
            "Failed to write Copilot diagnostic log '{}': {error}",
            path.display()
        );
        return None;
    }

    write_copilot_diagnostic_alias(&latest_path, &serialized, "latest");
    write_copilot_diagnostic_alias(&task_latest_path, &serialized, "task latest");
    if let Some(problem_latest_path) = problem_latest_path.as_ref() {
        write_copilot_diagnostic_alias(problem_latest_path, &serialized, "problem latest");
    }

    let path = path.display().to_string();
    eprintln!("Copilot diagnostic log written: {path}");
    Some(path)
}

fn write_copilot_diagnostic_alias(path: &Path, serialized: &str, label: &str) {
    if let Err(error) = std::fs::write(path, serialized) {
        eprintln!(
            "Failed to write {label} Copilot diagnostic log '{}': {error}",
            path.display()
        );
    }
}

fn diagnostic_response_text(outcome: &CopilotOutcome) -> Option<&str> {
    outcome.final_text.as_deref().or_else(|| {
        (!outcome.current_turn_stream.is_empty()).then_some(outcome.current_turn_stream.as_str())
    })
}

fn diagnostic_log_has_problem(outcome: &CopilotOutcome) -> bool {
    outcome.abort.is_some()
        || outcome
            .error
            .as_deref()
            .map(|error| !error.trim().is_empty())
            .unwrap_or(false)
        || !has_usable_final_text(outcome)
}

fn copilot_diagnostic_task_latest_file_name(task_label: &str) -> String {
    format!("latest-{}.json", sanitize_log_path_component(task_label))
}

fn record_diagnostic_event(target: &mut Vec<String>, kind: &str, event: &SessionEvent) {
    let serialized = serde_json::to_string(event)
        .unwrap_or_else(|_| format!("{{\"type\":\"{}\"}}", event.event_type));
    record_diagnostic_line(target, kind, &serialized);
}

fn record_diagnostic_line(target: &mut Vec<String>, kind: &str, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    target.push(format!(
        "{kind}: {}",
        limit_text(trimmed, MAX_DIAGNOSTIC_LINE_CHARS)
    ));
    if target.len() > MAX_DIAGNOSTIC_EVENTS {
        let remove_count = target.len() - MAX_DIAGNOSTIC_EVENTS;
        target.drain(0..remove_count);
    }
}

fn limit_diagnostic_text(value: &str) -> String {
    limit_text(value, MAX_DIAGNOSTIC_RESPONSE_CHARS)
}

fn copilot_diagnostics_include_prompt() -> bool {
    env_truthy(COPILOT_DIAGNOSTICS_INCLUDE_PROMPT_ENV)
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

fn append_diagnostic_log_suffix(mut message: String, diagnostic_log_path: Option<&str>) -> String {
    if let Some(path) = diagnostic_log_path {
        message.push_str(" Diagnostic log: ");
        message.push_str(path);
        message.push('.');
    }
    message
}

fn abort_kind_label(kind: AbortKind) -> &'static str {
    match kind {
        AbortKind::Overall => "overall",
        AbortKind::Inactivity => "inactivity",
    }
}

fn sanitize_log_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "copilot".to_string()
    } else {
        sanitized.to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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
    fn terminal_tool_request_message_does_not_replace_idle_activity() {
        let mut outcome = CopilotOutcome {
            last_visible_activity: Some("Copilot session is idle".to_string()),
            ..CopilotOutcome::default()
        };

        record_final_text_from_event(
            &event(
                "assistant.message",
                json!({
                    "messageId": "m1",
                    "content": "I'll inspect the current PR diff first.",
                    "toolRequests": [{ "toolCallId": "tool-1", "name": "view" }]
                }),
            ),
            &mut outcome,
        );

        assert!(outcome.final_text.is_none());
        assert!(outcome.saw_tool_request_message);
        assert_eq!(
            outcome.last_visible_activity.as_deref(),
            Some("Copilot session is idle")
        );
    }

    #[test]
    fn fallback_reason_explains_tool_request_without_json() {
        let outcome = CopilotOutcome {
            saw_meaningful_progress: true,
            saw_tool_request_message: true,
            last_visible_activity: Some("Copilot requested repository tools".to_string()),
            ..CopilotOutcome::default()
        };

        assert_eq!(
            fallback_reason(&outcome),
            "GitHub Copilot ended after requesting repository tools without returning the required JSON response."
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
