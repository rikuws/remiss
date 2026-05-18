use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde_json::{json, Value};

use crate::app_storage;
use crate::code_tour::{
    CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
    GeneratedCodeTour,
};

use super::binary::{find_copilot_binary, prepend_binary_parent_to_command_path};
use super::errors::{generation_abort_message, AbortKind, AbortReason};
use super::jsonrepair::parse_tolerant;
use super::merge::{build_copilot_fallback_tour, merge_tour, TourResponse};
use super::progress::{limit_text, make_progress};
use super::prompt::build_tour_prompt;
use super::{AgentJsonPromptOptions, AgentTextResponse, CodingAgentBackend};

const OVERALL_TIMEOUT_MS: u64 = 480_000;
const INACTIVITY_TIMEOUT_MS: u64 = 120_000;
const RUNNING_TICKER_MS: u64 = 10_000;
const POLL_INTERVAL: Duration = Duration::from_millis(120);
const MAX_PROMPT_BYTES: usize = 120_000;
const MAX_STACK_PLAN_PROMPT_BYTES: usize = 140_000;
const COPILOT_TOOL_ALLOWLISTS: &[&str] = &["view,rg,glob", "view,grep,glob", "view,glob"];
const COPILOT_DIAGNOSTICS_ENV: &str = "REMISS_COPILOT_DIAGNOSTICS";
const COPILOT_DIAGNOSTICS_INCLUDE_PROMPT_ENV: &str = "REMISS_COPILOT_DIAGNOSTICS_INCLUDE_PROMPT";
const COPILOT_DIAGNOSTICS_FLAG_FILE: &str = "copilot-diagnostics.enabled";
const COPILOT_DIAGNOSTIC_LOG_DIR: &str = "copilot-diagnostics";
const MAX_DIAGNOSTIC_EVENTS: usize = 240;
const MAX_DIAGNOSTIC_LINE_CHARS: usize = 4_000;
const MAX_DIAGNOSTIC_RESPONSE_CHARS: usize = 80_000;

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
    stderr_text: String,
    current_turn_stream: String,
    exit_code: Option<i32>,
    stream_events: Vec<String>,
}

struct CopilotRun {
    outcome: CopilotOutcome,
    exit_status: Option<ExitStatus>,
    diagnostic_log_path: Option<String>,
}

enum StreamLine {
    Stdout(String),
    Stderr(String),
}

#[derive(Copy, Clone)]
enum StreamKind {
    Stdout,
    Stderr,
}

impl CodingAgentBackend for CopilotBackend {
    fn provider(&self) -> CodeTourProvider {
        CodeTourProvider::Copilot
    }

    fn status(&self) -> Result<CodeTourProviderStatus, String> {
        let Some(binary) = find_copilot_binary() else {
            return Ok(CodeTourProviderStatus {
                provider: CodeTourProvider::Copilot,
                label: "Copilot".to_string(),
                available: false,
                authenticated: false,
                message: "GitHub Copilot CLI was not found.".to_string(),
                detail: "Install the GitHub Copilot CLI, or set REMISS_COPILOT_BINARY to its full path, then sign in with `copilot login` to enable AI code tours.".to_string(),
                default_model: None,
            });
        };

        let version = probe_version(&binary).unwrap_or_else(|_| "installed".to_string());

        Ok(CodeTourProviderStatus {
            provider: CodeTourProvider::Copilot,
            label: "Copilot".to_string(),
            available: true,
            authenticated: true,
            message: format!("GitHub Copilot CLI detected ({}).", version),
            detail:
                "Uses the detected Copilot CLI session. Auth errors surface on the first generate."
                    .to_string(),
            default_model: None,
        })
    }

    fn generate(
        &self,
        input: &GenerateCodeTourInput,
        on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
    ) -> Result<GeneratedCodeTour, String> {
        let Some(binary) = find_copilot_binary() else {
            return Err(
                "GitHub Copilot CLI was not found. Install it or set REMISS_COPILOT_BINARY."
                    .to_string(),
            );
        };

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
                "Launching the local Copilot CLI with streamed progress in the prepared local checkout."
                    .to_string(),
            ),
            Some("Starting Copilot CLI".to_string()),
        ));

        let mut prompt = build_tour_prompt(input);
        if prompt.len() > MAX_PROMPT_BYTES {
            truncate_to_byte_limit(&mut prompt, MAX_PROMPT_BYTES);
        }

        let CopilotRun {
            outcome,
            exit_status,
            diagnostic_log_path,
        } = run_copilot_with_tool_allowlist_retries(
            &binary,
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
                    generation_abort_message("GitHub Copilot", "the code tour", &abort),
                    diagnostic_log_path.as_deref(),
                );
                on_progress(make_progress(
                    "timeout",
                    summary.clone(),
                    Some(
                        "Aborting the Copilot run so the app can surface the failure without waiting."
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
                append_diagnostic_log_suffix(
                    fallback_reason(&outcome, exit_status.as_ref()),
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
                    fallback_reason(&outcome, exit_status.as_ref()),
                    diagnostic_log_path.as_deref(),
                ),
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
    mut prompt: String,
    options: AgentJsonPromptOptions,
) -> Result<AgentTextResponse, String> {
    let Some(binary) = find_copilot_binary() else {
        return Err(
            "GitHub Copilot CLI was not found. Install it or set REMISS_COPILOT_BINARY."
                .to_string(),
        );
    };

    if !Path::new(working_directory).is_dir() {
        return Err(format!(
            "The local checkout '{working_directory}' does not exist."
        ));
    }

    if prompt.len() > MAX_STACK_PLAN_PROMPT_BYTES {
        truncate_to_byte_limit(&mut prompt, MAX_STACK_PLAN_PROMPT_BYTES);
    }

    let mut ignore_progress = |_progress: CodeTourProgressUpdate| {};

    let CopilotRun {
        outcome,
        exit_status,
        diagnostic_log_path,
    } = run_copilot_with_tool_allowlist_retries(
        &binary,
        working_directory,
        &prompt,
        options.copilot_overall_timeout_ms,
        options.copilot_inactivity_timeout_ms,
        options.task_label,
        false,
        &mut ignore_progress,
    )?;

    if let Some(abort) = &outcome.abort {
        if !has_usable_final_text(&outcome) {
            return Err(append_diagnostic_log_suffix(
                generation_abort_message("GitHub Copilot", options.task_label, abort),
                diagnostic_log_path.as_deref(),
            ));
        }
    }

    if let Some(error) = &outcome.error {
        return Err(error.clone());
    }

    let Some(final_text) = outcome.final_text.as_deref() else {
        return Err(append_diagnostic_log_suffix(
            fallback_reason(&outcome, exit_status.as_ref()),
            diagnostic_log_path.as_deref(),
        ));
    };

    let trimmed = final_text.trim();
    if trimmed.is_empty() {
        return Err(append_diagnostic_log_suffix(
            fallback_reason(&outcome, exit_status.as_ref()),
            diagnostic_log_path.as_deref(),
        ));
    }

    Ok(AgentTextResponse {
        text: trimmed.to_string(),
        model: outcome.model,
    })
}

fn run_copilot_with_tool_allowlist_retries(
    binary: &str,
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
                Some(format!("Retrying Copilot with tools: {available_tools}")),
            ));
        }

        let run = run_copilot_cli(
            binary,
            working_directory,
            prompt,
            available_tools,
            overall_timeout_ms,
            inactivity_timeout_ms,
            task_label,
            attempt_ix + 1,
            COPILOT_TOOL_ALLOWLISTS.len(),
            emit_progress,
            on_progress,
        )?;

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

fn run_copilot_cli(
    binary: &str,
    working_directory: &str,
    prompt: &str,
    available_tools: &str,
    overall_timeout_ms: u64,
    inactivity_timeout_ms: u64,
    task_label: &str,
    attempt: usize,
    max_attempts: usize,
    emit_progress: bool,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) -> Result<CopilotRun, String> {
    let mut command = Command::new(binary);
    prepend_binary_parent_to_command_path(&mut command, binary);
    configure_copilot_child_process(&mut command);

    let mut child = command
        .arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--stream")
        .arg("on")
        .arg("--allow-all-tools")
        .arg("--available-tools")
        .arg(available_tools)
        .arg("--no-ask-user")
        .arg("--no-color")
        .arg("--log-level")
        .arg("error")
        .current_dir(working_directory)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to launch the Copilot CLI: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture the Copilot CLI stdout.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture the Copilot CLI stderr.".to_string())?;

    let (line_tx, line_rx) = mpsc::channel::<StreamLine>();
    let stdout_handle = spawn_line_reader(stdout, StreamKind::Stdout, line_tx.clone());
    let stderr_handle = spawn_line_reader(stderr, StreamKind::Stderr, line_tx);

    if emit_progress {
        on_progress(make_progress(
            "running",
            "GitHub Copilot is inspecting the checkout",
            Some("Waiting for streamed Copilot events from the linked repository.".to_string()),
            Some(format!(
                "Waiting for Copilot event stream ({available_tools})"
            )),
        ));
    }

    let start = Instant::now();
    let started_at_ms = now_ms();
    let mut last_activity = Instant::now();
    let mut last_ticker = Instant::now();
    let mut exit_status: Option<ExitStatus> = None;
    let mut outcome = CopilotOutcome::default();

    loop {
        while let Ok(line) = line_rx.try_recv() {
            handle_stream_line(line, &mut outcome, on_progress);
            last_activity = Instant::now();
        }

        if outcome_has_unknown_tool_allowlist_error(&outcome) && !has_usable_final_text(&outcome) {
            break;
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                outcome.exit_code = status.code();
                exit_status = Some(status);
                break;
            }
            Ok(None) => {}
            Err(error) => {
                terminate_copilot_child_tree(&mut child);
                return Err(format!("Failed to poll the Copilot CLI: {error}"));
            }
        }

        let now = Instant::now();

        if now.duration_since(start) > Duration::from_millis(overall_timeout_ms) {
            outcome.abort = Some(AbortReason {
                kind: AbortKind::Overall,
                timeout_ms: overall_timeout_ms,
                last_visible_activity: outcome.last_visible_activity.clone(),
            });
            break;
        }

        if now.duration_since(last_activity) > Duration::from_millis(inactivity_timeout_ms) {
            outcome.abort = Some(AbortReason {
                kind: AbortKind::Inactivity,
                timeout_ms: inactivity_timeout_ms,
                last_visible_activity: outcome.last_visible_activity.clone(),
            });
            break;
        }

        if emit_progress
            && now.duration_since(last_ticker) >= Duration::from_millis(RUNNING_TICKER_MS)
        {
            last_ticker = now;
            let elapsed_s = now.duration_since(start).as_secs();
            on_progress(make_progress(
                "running",
                "GitHub Copilot is still working",
                Some(format!("Elapsed: {elapsed_s}s.")),
                outcome.last_visible_activity.clone(),
            ));
        }

        match line_rx.recv_timeout(POLL_INTERVAL) {
            Ok(line) => {
                handle_stream_line(line, &mut outcome, on_progress);
                last_activity = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }

        if outcome_has_unknown_tool_allowlist_error(&outcome) && !has_usable_final_text(&outcome) {
            break;
        }
    }

    let should_stop_child =
        outcome.abort.is_some() || outcome_has_unknown_tool_allowlist_error(&outcome);
    if should_stop_child {
        terminate_copilot_child_tree(&mut child);
    }

    let _ = stdout_handle.join();
    let _ = stderr_handle.join();
    while let Ok(line) = line_rx.try_recv() {
        handle_stream_line(line, &mut outcome, on_progress);
    }
    promote_stream_to_final(&mut outcome);
    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let diagnostic_log_path = if copilot_diagnostics_enabled() {
        write_copilot_diagnostic_log(CopilotDiagnosticLogInput {
            started_at_ms,
            duration_ms,
            task_label,
            binary,
            working_directory,
            available_tools,
            overall_timeout_ms,
            inactivity_timeout_ms,
            prompt,
            outcome: &outcome,
            exit_status: exit_status.as_ref(),
            attempt,
            max_attempts,
        })
    } else {
        None
    };

    Ok(CopilotRun {
        outcome,
        exit_status,
        diagnostic_log_path,
    })
}

#[cfg(unix)]
fn configure_copilot_child_process(command: &mut Command) {
    // SAFETY: `pre_exec` runs after fork and before exec. The closure only calls
    // libc `setpgid`, which is safe in this context, so timeout cancellation can
    // terminate the Copilot CLI plus its Node child processes as one group.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(not(unix))]
fn configure_copilot_child_process(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_copilot_child_tree(child: &mut Child) {
    signal_copilot_process_group(child, libc::SIGTERM);
    for _ in 0..10 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    signal_copilot_process_group(child, libc::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn signal_copilot_process_group(child: &Child, signal: libc::c_int) {
    let process_group_id = child.id() as libc::pid_t;
    if process_group_id <= 0 {
        return;
    }

    // SAFETY: `configure_copilot_child_process` puts the spawned Copilot process
    // in a process group whose pgid is the child pid. A negative pid signals
    // that process group.
    unsafe {
        let _ = libc::kill(-process_group_id, signal);
    }
}

#[cfg(not(unix))]
fn terminate_copilot_child_tree(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn handle_stream_line(
    line: StreamLine,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    match line {
        StreamLine::Stdout(line) => {
            record_stream_event(outcome, "stdout", &line);
            handle_stdout_line(&line, outcome, on_progress)
        }
        StreamLine::Stderr(line) => {
            record_stream_event(outcome, "stderr", &line);
            append_line(&mut outcome.stderr_text, &line);
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                if is_unknown_tool_allowlist_error(trimmed) {
                    outcome.error =
                        Some(format!("GitHub Copilot CLI configuration error: {trimmed}"));
                }
                outcome.last_visible_activity = Some(limit_text(trimmed, 180));
            }
        }
    }
}

fn handle_stdout_line(
    line: &str,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    let event = match serde_json::from_str::<Value>(trimmed) {
        Ok(event) => event,
        Err(_) => {
            if is_unknown_tool_allowlist_error(trimmed) {
                outcome.error = Some(format!("GitHub Copilot CLI configuration error: {trimmed}"));
                return;
            }
            append_line(&mut outcome.current_turn_stream, trimmed);
            outcome.last_visible_activity = Some(limit_text(trimmed, 180));
            return;
        }
    };

    handle_json_event(&event, outcome, on_progress);
}

fn handle_json_event(
    event: &Value,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = event.get("data").cloned().unwrap_or(Value::Null);

    match event_type {
        "session.tools_updated" => {
            if let Some(model) = data.get("model").and_then(Value::as_str) {
                outcome.model = Some(model.to_string());
                outcome.last_visible_activity = Some(format!("Using model {model}"));
            }
        }
        "assistant.turn_start" => {
            outcome.current_turn_stream.clear();
            outcome.saw_meaningful_progress = true;
            let turn_id = data.get("turnId").and_then(Value::as_str).unwrap_or("0");
            let (summary, detail, log) = if turn_id == "0" {
                (
                    "GitHub Copilot is inspecting the checkout",
                    "Copilot started its first turn and is gathering repository context.",
                    "Started Copilot turn 0".to_string(),
                )
            } else {
                (
                    "GitHub Copilot is drafting the code tour",
                    "Copilot started another turn and is preparing the final structured response.",
                    format!("Started Copilot turn {turn_id}"),
                )
            };
            on_progress(make_progress(
                "running",
                summary,
                Some(detail.to_string()),
                Some(log.clone()),
            ));
            outcome.last_visible_activity = Some(log);
        }
        "assistant.message_delta" => {
            if let Some(delta) = data.get("deltaContent").and_then(Value::as_str) {
                outcome.saw_meaningful_progress = true;
                outcome.current_turn_stream.push_str(delta);
                let snippet = limit_text(&outcome.current_turn_stream, 180);
                if !snippet.is_empty() {
                    outcome.last_visible_activity = Some(snippet);
                }
            }
        }
        "assistant.message" => handle_assistant_message(&data, outcome, on_progress),
        "tool.execution_start" => {
            outcome.saw_meaningful_progress = true;
            let log = tool_activity_summary(&data);
            on_progress(make_progress(
                "tool",
                "GitHub Copilot is using a repository tool",
                Some(log.clone()),
                Some(log.clone()),
            ));
            outcome.last_visible_activity = Some(log);
        }
        "tool.execution_complete" => {
            outcome.saw_meaningful_progress = true;
            let success = data.get("success").and_then(Value::as_bool).unwrap_or(true);
            let log = tool_activity_summary(&data);
            if success {
                outcome.last_visible_activity = Some(format!("Completed {log}"));
            } else {
                let detail = format!("Tool failed: {log}");
                on_progress(make_progress(
                    "tool_failed",
                    "A GitHub Copilot tool step failed",
                    Some(detail.clone()),
                    Some(detail.clone()),
                ));
                outcome.last_visible_activity = Some(detail);
            }
        }
        "session.info" => {
            if let Some(message) = data.get("message").and_then(Value::as_str) {
                let trimmed = message.trim();
                if !trimmed.is_empty() {
                    if is_unknown_tool_allowlist_error(trimmed) {
                        outcome.error =
                            Some(format!("GitHub Copilot CLI configuration error: {trimmed}"));
                    }
                    outcome.last_visible_activity = Some(limit_text(trimmed, 180));
                }
            }
        }
        "assistant.reasoning" => {
            if let Some(content) = data.get("content").and_then(Value::as_str) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    outcome.last_visible_activity = Some(limit_text(trimmed, 180));
                }
            }
        }
        "result" => {
            if let Some(code) = event.get("exitCode").and_then(Value::as_i64) {
                outcome.exit_code = Some(code as i32);
            }
        }
        _ => {}
    }
}

fn handle_assistant_message(
    data: &Value,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    let content = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if content.is_empty() {
        return;
    }

    outcome.saw_meaningful_progress = true;
    let phase = data.get("phase").and_then(Value::as_str);
    let tool_requests = data
        .get("toolRequests")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or_default();

    if phase == Some("final_answer") {
        outcome.final_text = Some(content.to_string());
        outcome.current_turn_stream = content.to_string();
        on_progress(make_progress(
            "drafting",
            "GitHub Copilot drafted the code tour response",
            Some(limit_text(content, 240)),
            Some("Copilot drafted the final response".to_string()),
        ));
        outcome.last_visible_activity = Some("Copilot drafted the final response".to_string());
        return;
    }

    let detail = limit_text(content, 240);
    let log = summarize_tool_request(data).unwrap_or_else(|| detail.clone());

    if tool_requests > 0 {
        on_progress(make_progress(
            "running",
            "GitHub Copilot is inspecting the checkout",
            Some(detail.clone()),
            Some(log.clone()),
        ));
    } else {
        on_progress(make_progress(
            "running",
            "GitHub Copilot sent a progress update",
            Some(detail.clone()),
            Some(log.clone()),
        ));
    }

    outcome.last_visible_activity = Some(log);
}

fn summarize_tool_request(data: &Value) -> Option<String> {
    data.get("toolRequests")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("name").and_then(Value::as_str))
        .map(|name| format!("Tool: {name}"))
}

fn tool_activity_summary(data: &Value) -> String {
    let tool_name = data
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("tool");

    match preferred_argument(data.get("arguments").unwrap_or(&Value::Null)) {
        Some(arg) => format!("{tool_name}: {}", limit_text(&arg, 180)),
        None => format!("Tool: {tool_name}"),
    }
}

fn preferred_argument(arguments: &Value) -> Option<String> {
    let object = arguments.as_object()?;

    for key in ["path", "pattern", "query", "command", "url"] {
        if let Some(value) = object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }

    if object.is_empty() {
        None
    } else {
        serde_json::to_string(arguments).ok()
    }
}

fn fallback_reason(outcome: &CopilotOutcome, exit_status: Option<&ExitStatus>) -> String {
    if let Some(error) = &outcome.error {
        return error.clone();
    }

    let stderr_text = outcome.stderr_text.trim();
    if !stderr_text.is_empty() {
        return format!("GitHub Copilot reported: {stderr_text}");
    }

    if let Some(activity) = outcome
        .last_visible_activity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!(
            "GitHub Copilot returned no final response. Last visible activity: {activity}."
        );
    }

    match exit_status.and_then(ExitStatus::code).or(outcome.exit_code) {
        Some(code) => format!("GitHub Copilot exited with status code {code}."),
        None => "GitHub Copilot returned an empty code tour response.".to_string(),
    }
}

struct CopilotDiagnosticLogInput<'a> {
    started_at_ms: u64,
    duration_ms: u64,
    task_label: &'a str,
    binary: &'a str,
    working_directory: &'a str,
    available_tools: &'a str,
    overall_timeout_ms: u64,
    inactivity_timeout_ms: u64,
    prompt: &'a str,
    outcome: &'a CopilotOutcome,
    exit_status: Option<&'a ExitStatus>,
    attempt: usize,
    max_attempts: usize,
}

fn copilot_diagnostics_enabled() -> bool {
    env_truthy(COPILOT_DIAGNOSTICS_ENV)
        || app_storage::data_dir_root()
            .join(COPILOT_DIAGNOSTICS_FLAG_FILE)
            .exists()
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
    let file_name = format!(
        "{}-{}-attempt-{}.json",
        input.started_at_ms, task_slug, input.attempt
    );
    let path = log_dir.join(file_name);
    let latest_path = log_dir.join("latest.json");
    let task_latest_path = log_dir.join(copilot_diagnostic_task_latest_file_name(input.task_label));
    let problem_latest_path = diagnostic_log_has_problem(input.outcome, input.exit_status)
        .then(|| log_dir.join("latest-problem.json"));
    let path_display = path.display().to_string();
    let latest_path_display = latest_path.display().to_string();
    let task_latest_path_display = task_latest_path.display().to_string();
    let problem_latest_path_display = problem_latest_path
        .as_ref()
        .map(|path| path.display().to_string());
    let abort = input.outcome.abort.as_ref().map(|reason| {
        json!({
            "kind": abort_kind_label(reason.kind),
            "timeoutMs": reason.timeout_ms,
            "lastVisibleActivity": reason.last_visible_activity,
        })
    });
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
        "availableTools": input.available_tools,
        "attempt": input.attempt,
        "maxAttempts": input.max_attempts,
        "timeouts": {
            "overallMs": input.overall_timeout_ms,
            "inactivityMs": input.inactivity_timeout_ms,
        },
        "model": input.outcome.model,
        "exitStatusCode": input.exit_status.and_then(ExitStatus::code).or(input.outcome.exit_code),
        "abort": abort,
        "error": input.outcome.error,
        "lastVisibleActivity": input.outcome.last_visible_activity,
        "sawMeaningfulProgress": input.outcome.saw_meaningful_progress,
        "stderr": input.outcome.stderr_text,
        "finalTextBytes": input.outcome.final_text.as_deref().map(str::len),
        "currentTurnStreamBytes": input.outcome.current_turn_stream.len(),
        "currentTurnStreamPreview": limit_diagnostic_text(&input.outcome.current_turn_stream),
        "streamEvents": input.outcome.stream_events,
        "promptBytes": input.prompt.len(),
        "promptIncluded": copilot_diagnostics_include_prompt(),
        "prompt": if copilot_diagnostics_include_prompt() {
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

fn diagnostic_log_has_problem(outcome: &CopilotOutcome, exit_status: Option<&ExitStatus>) -> bool {
    outcome.abort.is_some()
        || outcome
            .error
            .as_deref()
            .map(|error| !error.trim().is_empty())
            .unwrap_or(false)
        || exit_status
            .map(|status| !status.success())
            .unwrap_or_else(|| outcome.exit_code.map(|code| code != 0).unwrap_or(false))
}

fn copilot_diagnostic_task_latest_file_name(task_label: &str) -> String {
    format!("latest-{}.json", sanitize_log_path_component(task_label))
}

fn record_stream_event(outcome: &mut CopilotOutcome, kind: &str, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    outcome.stream_events.push(format!(
        "{kind}: {}",
        limit_text(trimmed, MAX_DIAGNOSTIC_LINE_CHARS)
    ));
    if outcome.stream_events.len() > MAX_DIAGNOSTIC_EVENTS {
        let remove_count = outcome.stream_events.len() - MAX_DIAGNOSTIC_EVENTS;
        outcome.stream_events.drain(0..remove_count);
    }
}

fn limit_diagnostic_text(value: &str) -> String {
    limit_text(value, MAX_DIAGNOSTIC_RESPONSE_CHARS)
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

fn promote_stream_to_final(outcome: &mut CopilotOutcome) {
    if outcome.final_text.is_none() {
        let trimmed = outcome.current_turn_stream.trim();
        if !trimmed.is_empty() {
            outcome.final_text = Some(trimmed.to_string());
        }
    }
}

fn has_usable_final_text(outcome: &CopilotOutcome) -> bool {
    outcome
        .final_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
}

fn outcome_has_unknown_tool_allowlist_error(outcome: &CopilotOutcome) -> bool {
    outcome
        .error
        .as_deref()
        .map(is_unknown_tool_allowlist_error)
        .unwrap_or(false)
}

fn is_unknown_tool_allowlist_error(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("unknown tool name") && normalized.contains("tool allowlist")
}

fn append_line(target: &mut String, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(trimmed);
}

fn truncate_to_byte_limit(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }

    let mut cutoff = max_bytes;
    while !value.is_char_boundary(cutoff) {
        cutoff = cutoff.saturating_sub(1);
    }
    value.truncate(cutoff);
}

fn probe_version(binary: &str) -> Result<String, String> {
    let mut command = Command::new(binary);
    prepend_binary_parent_to_command_path(&mut command, binary);

    let output = command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("Failed to run `{binary} --version`: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("installed")
        .to_string())
}

fn spawn_line_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    kind: StreamKind,
    sender: mpsc::Sender<StreamLine>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let message = match kind {
                StreamKind::Stdout => StreamLine::Stdout(line),
                StreamKind::Stderr => StreamLine::Stderr(line),
            };
            if sender.send(message).is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn session_tools_updated_captures_model() {
        let event = json!({
            "type": "session.tools_updated",
            "data": {
                "model": "gpt-5.4"
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(outcome.model.as_deref(), Some("gpt-5.4"));
        assert!(progress.is_empty());
    }

    #[test]
    fn assistant_message_delta_updates_current_turn_stream() {
        let event = json!({
            "type": "assistant.message_delta",
            "data": {
                "deltaContent": "{\"summary\""
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(outcome.current_turn_stream, "{\"summary\"");
        assert_eq!(
            outcome.last_visible_activity.as_deref(),
            Some("{\"summary\"")
        );
        assert!(progress.is_empty());
    }

    #[test]
    fn assistant_final_answer_sets_final_text_and_progress() {
        let event = json!({
            "type": "assistant.message",
            "data": {
                "content": "{\"summary\":\"done\"}",
                "toolRequests": [],
                "phase": "final_answer"
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(
            outcome.final_text.as_deref(),
            Some("{\"summary\":\"done\"}")
        );
        assert_eq!(outcome.current_turn_stream, "{\"summary\":\"done\"}");
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].stage, "drafting");
        assert!(progress[0].summary.contains("drafted"));
    }

    #[test]
    fn assistant_turn_start_marks_meaningful_progress() {
        let event = json!({
            "type": "assistant.turn_start",
            "data": {
                "turnId": "6"
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert!(outcome.saw_meaningful_progress);
        assert_eq!(progress.len(), 1);
        assert_eq!(
            progress[0].summary,
            "GitHub Copilot is drafting the code tour"
        );
    }

    #[test]
    fn promote_stream_to_final_uses_buffered_stream() {
        let mut outcome = CopilotOutcome {
            current_turn_stream: "{\"summary\":\"done\"}".to_string(),
            ..CopilotOutcome::default()
        };

        promote_stream_to_final(&mut outcome);

        assert_eq!(
            outcome.final_text.as_deref(),
            Some("{\"summary\":\"done\"}")
        );
        assert!(has_usable_final_text(&outcome));
    }

    #[test]
    fn prompt_truncation_respects_utf8_boundaries() {
        let mut prompt = "aaaaébbbb".to_string();

        truncate_to_byte_limit(&mut prompt, 5);

        assert_eq!(prompt, "aaaa");
        assert!(prompt.len() <= 5);
    }

    #[test]
    fn unknown_tool_allowlist_errors_are_retriable() {
        assert!(is_unknown_tool_allowlist_error(
            "GitHub Copilot CLI configuration error: Unknown tool name in the tool allowlist: \"rg\""
        ));
        assert!(is_unknown_tool_allowlist_error(
            "GitHub Copilot CLI configuration error: Unknown tool name in the tool allowlist: \"grep\""
        ));
        assert!(!is_unknown_tool_allowlist_error(
            "GitHub Copilot reported an authentication error"
        ));
    }

    #[test]
    fn copilot_tool_allowlists_prefer_current_search_tool_before_legacy_grep() {
        assert_eq!(COPILOT_TOOL_ALLOWLISTS[0], "view,rg,glob");
        assert_eq!(COPILOT_TOOL_ALLOWLISTS[1], "view,grep,glob");
        assert_eq!(COPILOT_TOOL_ALLOWLISTS[2], "view,glob");
    }

    #[test]
    fn diagnostic_task_latest_file_name_is_task_scoped() {
        assert_eq!(
            copilot_diagnostic_task_latest_file_name("Review Partner context"),
            "latest-Review_Partner_context.json"
        );
        assert_eq!(
            copilot_diagnostic_task_latest_file_name("Review Memory candidate extraction"),
            "latest-Review_Memory_candidate_extraction.json"
        );
    }

    #[test]
    fn stderr_unknown_tool_allowlist_sets_retryable_error() {
        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();

        handle_stream_line(
            StreamLine::Stderr("Unknown tool name in the tool allowlist: \"rg\"".to_string()),
            &mut outcome,
            &mut |update| progress.push(update),
        );

        assert!(outcome_has_unknown_tool_allowlist_error(&outcome));
        assert!(progress.is_empty());
    }

    #[test]
    fn stdout_unknown_tool_allowlist_sets_retryable_error() {
        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();

        handle_stream_line(
            StreamLine::Stdout("unknown tool name in the tool allowlist: \"grep\"".to_string()),
            &mut outcome,
            &mut |update| progress.push(update),
        );

        assert!(outcome_has_unknown_tool_allowlist_error(&outcome));
        assert!(outcome.current_turn_stream.is_empty());
        assert!(progress.is_empty());
    }

    #[test]
    fn tool_execution_start_reports_tool_progress() {
        let event = json!({
            "type": "tool.execution_start",
            "data": {
                "toolName": "view",
                "arguments": {
                    "path": "/tmp/repo"
                }
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].stage, "tool");
        assert_eq!(progress[0].detail.as_deref(), Some("view: /tmp/repo"));
        assert_eq!(
            outcome.last_visible_activity.as_deref(),
            Some("view: /tmp/repo")
        );
    }

    #[test]
    fn session_info_unknown_tool_sets_error() {
        let event = json!({
            "type": "session.info",
            "data": {
                "message": "Unknown tool name in the tool allowlist: \"grep\""
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(
            outcome.error.as_deref(),
            Some("GitHub Copilot CLI configuration error: Unknown tool name in the tool allowlist: \"grep\"")
        );
        assert!(progress.is_empty());
    }

    #[test]
    fn session_info_lowercase_unknown_tool_sets_error() {
        let event = json!({
            "type": "session.info",
            "data": {
                "message": "unknown tool name in the tool allowlist: \"grep\""
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(
            outcome.error.as_deref(),
            Some("GitHub Copilot CLI configuration error: unknown tool name in the tool allowlist: \"grep\"")
        );
        assert!(progress.is_empty());
    }
}
