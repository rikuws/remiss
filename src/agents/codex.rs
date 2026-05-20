use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use codex_codes::cli::AppServerBuilder;
use codex_codes::client_async::AsyncClient;
use codex_codes::jsonrpc::RequestId;
use codex_codes::protocol::{
    methods, AgentMessageDeltaNotification, CommandApprovalDecision,
    CommandExecutionApprovalParams, CommandExecutionApprovalResponse, ErrorNotification,
    FileChangeApprovalDecision, FileChangeApprovalResponse, ItemCompletedNotification,
    ItemStartedNotification, ReasoningDeltaNotification, ServerMessage, ThreadStartParams,
    ThreadStartedNotification, TurnCompletedNotification, TurnStartedNotification, TurnStatus,
};
use codex_codes::{CommandExecutionStatus, McpToolCallStatus, ThreadItem};
use serde_json::{json, Value};
use tokio::time::timeout as tokio_timeout;

use crate::review_ai::{ReviewAiProgressUpdate, ReviewAiProvider, ReviewAiProviderStatus};

use super::binary::find_codex_binary;
use super::errors::{generation_abort_message, AbortKind, AbortReason};
use super::progress::make_progress;
use super::runtime;
use super::{AgentJsonPromptOptions, AgentTextResponse, CodingAgentBackend};

const RUNNING_TICKER_MS: u64 = 10_000;
const NEXT_MESSAGE_POLL: Duration = Duration::from_millis(250);

pub struct CodexBackend;

impl CodexBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodexBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CodingAgentBackend for CodexBackend {
    fn provider(&self) -> ReviewAiProvider {
        ReviewAiProvider::Codex
    }

    fn status(&self) -> Result<ReviewAiProviderStatus, String> {
        let Some(_binary) = find_codex_binary() else {
            return Ok(ReviewAiProviderStatus {
                provider: ReviewAiProvider::Codex,
                label: "Codex".to_string(),
                available: false,
                authenticated: false,
                message: "Codex CLI is not installed on PATH.".to_string(),
                detail: "Install the Codex CLI (https://platform.openai.com/docs/codex) and sign in with `codex login` to enable AI review intelligence.".to_string(),
                default_model: None,
            });
        };

        Ok(ReviewAiProviderStatus {
            provider: ReviewAiProvider::Codex,
            label: "Codex".to_string(),
            available: true,
            authenticated: true,
            message: "Codex CLI detected.".to_string(),
            detail: "Uses the detected Codex CLI session.".to_string(),
            default_model: None,
        })
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
    prompt: String,
    options: AgentJsonPromptOptions,
    on_progress: &mut dyn FnMut(ReviewAiProgressUpdate),
) -> Result<AgentTextResponse, String> {
    let Some(binary) = find_codex_binary() else {
        return Err("Codex CLI is not installed on PATH.".to_string());
    };

    if !std::path::Path::new(working_directory).is_dir() {
        return Err(format!(
            "The local checkout '{working_directory}' does not exist."
        ));
    }

    let prompt_bytes = prompt.len();
    let working_directory = PathBuf::from(working_directory);
    let (progress_tx, progress_rx) = mpsc::channel::<ReviewAiProgressUpdate>();
    let (result_tx, result_rx) = mpsc::channel::<Result<CodexTurnOutcome, String>>();

    let worker = thread::spawn(move || {
        let outcome = runtime::shared().block_on(run_codex_turn(
            binary,
            working_directory,
            prompt,
            progress_tx,
            options.codex_overall_timeout_ms,
            options.codex_inactivity_timeout_ms,
        ));
        let _ = result_tx.send(outcome);
    });

    loop {
        while let Ok(progress) = progress_rx.try_recv() {
            on_progress(progress);
        }

        match result_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(outcome) => {
                while let Ok(progress) = progress_rx.try_recv() {
                    on_progress(progress);
                }
                let _ = worker.join();
                return finalize_text_turn(outcome, options.task_label, prompt_bytes);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                return Err("Codex worker thread exited without reporting a result.".to_string());
            }
        }
    }
}

struct CodexTurnOutcome {
    final_text: Option<String>,
    last_visible_activity: Option<String>,
    abort: Option<AbortReason>,
    model: Option<String>,
    error: Option<String>,
    used_checkout_context: bool,
    checkout_command_count: usize,
    inspected_path_hints: Vec<String>,
}

fn finalize_text_turn(
    outcome: Result<CodexTurnOutcome, String>,
    task_label: &str,
    prompt_bytes: usize,
) -> Result<AgentTextResponse, String> {
    let outcome = outcome?;

    if let Some(abort) = &outcome.abort {
        return Err(generation_abort_message("Codex", task_label, abort));
    }

    if let Some(error) = &outcome.error {
        return Err(format!("Codex reported an error: {error}"));
    }

    let Some(final_text) = outcome.final_text.as_deref() else {
        let reason = outcome
            .last_visible_activity
            .unwrap_or_else(|| "Codex did not return a final message.".to_string());
        return Err(format!("Codex returned no final agent message: {reason}"));
    };

    let trimmed = final_text.trim();
    if trimmed.is_empty() {
        return Err("Codex returned an empty JSON response.".to_string());
    }

    Ok(AgentTextResponse {
        text: trimmed.to_string(),
        model: outcome.model,
        used_checkout_context: outcome.used_checkout_context,
        checkout_command_count: outcome.checkout_command_count,
        inspected_path_hints: outcome.inspected_path_hints,
        prompt_bytes,
    })
}

async fn run_codex_turn(
    binary: String,
    working_directory: PathBuf,
    prompt: String,
    progress_tx: mpsc::Sender<ReviewAiProgressUpdate>,
    overall_timeout_ms: u64,
    inactivity_timeout_ms: u64,
) -> Result<CodexTurnOutcome, String> {
    let builder = AppServerBuilder::new()
        .command(&binary)
        .working_directory(&working_directory);

    let mut client = AsyncClient::start_with(builder)
        .await
        .map_err(|error| format!("Failed to start the Codex app-server: {error}"))?;

    let thread_response = client
        .thread_start(&ThreadStartParams {
            instructions: Some(codex_review_instructions()),
            tools: None,
        })
        .await
        .map_err(|error| format!("Failed to open a Codex thread: {error}"))?;
    let thread_id = thread_response.thread_id().to_string();
    let model = thread_response.model.clone();

    let turn_start_params = build_turn_start_params(&thread_id, &prompt);

    client
        .request::<_, Value>(methods::TURN_START, &turn_start_params)
        .await
        .map_err(|error| format!("Failed to start a Codex turn: {error}"))?;

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_ticker = Instant::now();

    let mut outcome = CodexTurnOutcome {
        final_text: None,
        last_visible_activity: None,
        abort: None,
        model,
        error: None,
        used_checkout_context: false,
        checkout_command_count: 0,
        inspected_path_hints: Vec::new(),
    };
    let mut streaming_message = String::new();

    loop {
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

        if now.duration_since(last_ticker) >= Duration::from_millis(RUNNING_TICKER_MS) {
            last_ticker = now;
            let elapsed_s = now.duration_since(start).as_secs();
            let _ = progress_tx.send(make_progress(
                "running",
                "Codex is still working",
                Some(format!("Elapsed: {elapsed_s}s.")),
                Some("Codex still working".to_string()),
            ));
        }

        let next = tokio_timeout(NEXT_MESSAGE_POLL, client.next_message()).await;
        let message = match next {
            Err(_) => continue,
            Ok(Ok(Some(message))) => message,
            Ok(Ok(None)) => {
                outcome.error = Some("Codex app-server closed the connection.".to_string());
                break;
            }
            Ok(Err(error)) => {
                outcome.error = Some(format!("Codex app-server error: {error}"));
                break;
            }
        };

        last_activity = Instant::now();

        match message {
            ServerMessage::Notification { method, params } => {
                let finished = handle_notification(
                    &method,
                    params,
                    &progress_tx,
                    &mut outcome,
                    &mut streaming_message,
                );
                if finished {
                    break;
                }
            }
            ServerMessage::Request { id, method, params } => {
                handle_request(&mut client, id, &method, params, &progress_tx).await;
            }
        }
    }

    let _ = client.shutdown().await;
    Ok(outcome)
}

fn handle_notification(
    method: &str,
    params: Option<Value>,
    progress_tx: &mpsc::Sender<ReviewAiProgressUpdate>,
    outcome: &mut CodexTurnOutcome,
    streaming_message: &mut String,
) -> bool {
    match method {
        methods::THREAD_STARTED => {
            if let Some(params) = params.clone() {
                let _: Result<ThreadStartedNotification, _> = serde_json::from_value(params);
            }
            let _ = progress_tx.send(make_progress(
                "thread",
                "Codex started a new thread",
                Some("The agent is ready to inspect the prepared local checkout.".to_string()),
                Some("Started Codex thread".to_string()),
            ));
            outcome.last_visible_activity = Some("Started Codex thread".to_string());
        }
        methods::TURN_STARTED => {
            if let Some(params) = params.clone() {
                let _: Result<TurnStartedNotification, _> = serde_json::from_value(params);
            }
            let _ = progress_tx.send(make_progress(
                "turn",
                "Codex is inspecting the change",
                Some(
                    "Walking the changed files and related callsites from the checkout."
                        .to_string(),
                ),
                Some("Inspecting the changed files".to_string()),
            ));
            outcome.last_visible_activity = Some("Inspecting the changed files".to_string());
        }
        methods::ITEM_STARTED => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ItemStartedNotification>(params) {
                    progress_for_item(&notif.item, ItemLifecycle::Started, progress_tx, outcome);
                }
            }
        }
        methods::ITEM_COMPLETED => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ItemCompletedNotification>(params) {
                    if let ThreadItem::AgentMessage(ref msg) = notif.item {
                        if !msg.text.trim().is_empty() {
                            outcome.final_text = Some(msg.text.clone());
                        }
                    }
                    progress_for_item(&notif.item, ItemLifecycle::Completed, progress_tx, outcome);
                }
            }
        }
        methods::AGENT_MESSAGE_DELTA => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<AgentMessageDeltaNotification>(params) {
                    streaming_message.push_str(&notif.delta);
                }
            }
        }
        methods::REASONING_DELTA => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ReasoningDeltaNotification>(params) {
                    let trimmed = notif.delta.trim();
                    if !trimmed.is_empty() {
                        let snippet = short_text(trimmed, 240);
                        let _ = progress_tx.send(make_progress(
                            "reasoning",
                            "Codex is reasoning through the change",
                            Some(snippet.clone()),
                            Some(short_text(trimmed, 180)),
                        ));
                        outcome.last_visible_activity = Some(snippet);
                    }
                }
            }
        }
        methods::TURN_COMPLETED => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<TurnCompletedNotification>(params) {
                    if outcome.final_text.is_none() {
                        outcome.final_text = final_agent_message(&notif);
                    }
                    if matches!(notif.turn.status, TurnStatus::Failed) {
                        outcome.error = notif
                            .turn
                            .error
                            .map(|err| err.message)
                            .or_else(|| Some("Codex turn failed.".to_string()));
                    }
                }
            }
            if outcome.final_text.is_none() && !streaming_message.trim().is_empty() {
                outcome.final_text = Some(std::mem::take(streaming_message));
            }
            let _ = progress_tx.send(make_progress(
                "finalizing",
                "Codex finished gathering context",
                Some("Formatting the structured Guided Review walkthrough response.".to_string()),
                Some("Codex finished its turn".to_string()),
            ));
            return true;
        }
        methods::ERROR => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ErrorNotification>(params) {
                    outcome.error = Some(notif.error);
                }
            }
        }
        _ => {}
    }

    false
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum ItemLifecycle {
    Started,
    Completed,
}

fn progress_for_item(
    item: &ThreadItem,
    lifecycle: ItemLifecycle,
    progress_tx: &mpsc::Sender<ReviewAiProgressUpdate>,
    outcome: &mut CodexTurnOutcome,
) {
    match item {
        ThreadItem::CommandExecution(cmd) if lifecycle == ItemLifecycle::Started => {
            let summary = format!("Command: {}", short_text(&cmd.command, 160));
            outcome.used_checkout_context = true;
            outcome.checkout_command_count += 1;
            record_path_hints_from_text(outcome, &cmd.command);
            let _ = progress_tx.send(make_progress(
                "command",
                "Codex is inspecting checkout files",
                Some(short_text(&cmd.command, 240)),
                Some(summary.clone()),
            ));
            outcome.last_visible_activity = Some(summary);
        }
        ThreadItem::CommandExecution(cmd)
            if lifecycle == ItemLifecycle::Completed
                && matches!(cmd.status, CommandExecutionStatus::Failed) =>
        {
            let summary = format!("Command failed: {}", short_text(&cmd.command, 160));
            let _ = progress_tx.send(make_progress(
                "command_failed",
                "A Codex command failed",
                Some(short_text(&cmd.command, 240)),
                Some(summary.clone()),
            ));
            outcome.last_visible_activity = Some(summary);
        }
        ThreadItem::McpToolCall(tool) if lifecycle == ItemLifecycle::Started => {
            let tool_ref = format!("{}/{}", tool.server, tool.tool);
            outcome.used_checkout_context = true;
            outcome.checkout_command_count += 1;
            record_path_hints_from_text(outcome, &tool.arguments.to_string());
            let _ = progress_tx.send(make_progress(
                "tool",
                "Codex is inspecting checkout context",
                Some(tool_ref.clone()),
                Some(format!("Tool: {tool_ref}")),
            ));
            outcome.last_visible_activity = Some(format!("Tool: {tool_ref}"));
        }
        ThreadItem::McpToolCall(tool)
            if lifecycle == ItemLifecycle::Completed
                && matches!(tool.status, McpToolCallStatus::Failed) =>
        {
            let tool_ref = format!("{}/{}", tool.server, tool.tool);
            let detail = tool
                .error
                .as_ref()
                .map(|err| short_text(&err.message, 240))
                .unwrap_or_else(|| format!("Tool failed: {tool_ref}"));
            let _ = progress_tx.send(make_progress(
                "tool_failed",
                "A Codex tool step failed",
                Some(detail.clone()),
                Some(format!("Tool failed: {tool_ref}")),
            ));
            outcome.last_visible_activity = Some(format!("Tool failed: {tool_ref}"));
        }
        ThreadItem::TodoList(list) => {
            let next = list
                .items
                .iter()
                .find(|entry| !entry.completed)
                .map(|entry| short_text(&entry.text, 240))
                .unwrap_or_else(|| {
                    "Updating the current plan for the Guided Review walkthrough run.".to_string()
                });
            let _ = progress_tx.send(make_progress(
                "planning",
                "Codex is updating its review plan",
                Some(next.clone()),
                Some(next.clone()),
            ));
            outcome.last_visible_activity = Some(next);
        }
        ThreadItem::Reasoning(reasoning) if lifecycle == ItemLifecycle::Completed => {
            let detail = short_text(&reasoning.text, 240);
            let _ = progress_tx.send(make_progress(
                "reasoning",
                "Codex is reasoning through the change",
                Some(detail.clone()),
                Some(short_text(&reasoning.text, 180)),
            ));
            outcome.last_visible_activity = Some(detail);
        }
        ThreadItem::WebSearch(search) if lifecycle == ItemLifecycle::Started => {
            let detail = short_text(&search.query, 240);
            let _ = progress_tx.send(make_progress(
                "search",
                "Codex is searching for context",
                Some(detail.clone()),
                Some(short_text(&search.query, 180)),
            ));
            outcome.last_visible_activity = Some(detail);
        }
        ThreadItem::AgentMessage(_) if lifecycle == ItemLifecycle::Completed => {
            let _ = progress_tx.send(make_progress(
                "drafting",
                "Codex drafted the Guided Review walkthrough response",
                Some("Finalizing the structured output for the app.".to_string()),
                Some("Codex drafted the final response".to_string()),
            ));
            outcome.last_visible_activity = Some("Codex drafted the final response".to_string());
        }
        _ => {}
    }
}

fn final_agent_message(notif: &TurnCompletedNotification) -> Option<String> {
    for item in notif.turn.items.iter().rev() {
        if let ThreadItem::AgentMessage(msg) = item {
            if !msg.text.trim().is_empty() {
                return Some(msg.text.clone());
            }
        }
    }
    None
}

async fn handle_request(
    client: &mut AsyncClient,
    id: RequestId,
    method: &str,
    params: Option<Value>,
    progress_tx: &mpsc::Sender<ReviewAiProgressUpdate>,
) {
    match method {
        methods::CMD_EXEC_APPROVAL => {
            let approval = params.and_then(|params| {
                serde_json::from_value::<CommandExecutionApprovalParams>(params).ok()
            });
            let decision = approval
                .as_ref()
                .map(|approval| command_approval_decision(&approval.command))
                .unwrap_or(CommandApprovalDecision::Decline);
            let accepted = matches!(
                decision,
                CommandApprovalDecision::Accept | CommandApprovalDecision::AcceptForSession
            );
            let (summary, detail, log) = if let Some(approval) = approval.as_ref() {
                if accepted {
                    (
                        "Codex may run a read-only checkout command",
                        format!(
                            "Approved read-only command: {}",
                            short_text(&approval.command, 220)
                        ),
                        "Approved a Codex read-only command".to_string(),
                    )
                } else {
                    (
                        "Codex requested a command that is not allowed",
                        format!(
                            "Declined command outside the read-only allowlist: {}",
                            short_text(&approval.command, 220)
                        ),
                        "Declined a Codex command approval".to_string(),
                    )
                }
            } else {
                (
                    "Codex requested a command that is not allowed",
                    "The command approval payload was not recognized.".to_string(),
                    "Declined an unrecognized Codex command approval".to_string(),
                )
            };
            let _ = progress_tx.send(make_progress(
                if accepted {
                    "command_approved"
                } else {
                    "tool_failed"
                },
                summary,
                Some(detail),
                Some(log),
            ));
            let response = CommandExecutionApprovalResponse { decision };
            let _ = client.respond(id, &response).await;
        }
        methods::FILE_CHANGE_APPROVAL => {
            let _ = progress_tx.send(make_progress(
                "tool_failed",
                "Codex requested a file change that is not allowed",
                Some(
                    "Review intelligence never edits files; the change was declined automatically."
                        .to_string(),
                ),
                Some("Declined a Codex file change approval".to_string()),
            ));
            let response = FileChangeApprovalResponse {
                decision: FileChangeApprovalDecision::Decline,
            };
            let _ = client.respond(id, &response).await;
        }
        _ => {
            let _ = client
                .respond_error(id, -32601, "method not implemented")
                .await;
        }
    }
}

fn short_text(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", truncated.trim_end())
}

fn build_turn_start_params(thread_id: &str, prompt: &str) -> Value {
    json!({
        "threadId": thread_id,
        "input": [
            {
                "type": "text",
                "text": prompt,
            }
        ],
        // Newer Codex app-server builds use `effort`; older ones still expect
        // `reasoningEffort`. Sending both keeps this request compatible across
        // the CLI versions we have seen in the wild.
        "effort": "low",
        "reasoningEffort": "low",
        "sandboxPolicy": compatible_read_only_sandbox_policy(),
    })
}

fn codex_review_instructions() -> String {
    [
        "You are helping Remiss generate read-only pull request review intelligence.",
        "You may inspect the prepared local checkout or generated context workspace with narrow read-only shell commands when that helps ground the answer.",
        "Allowed command families are pwd, ls, find, rg, grep, sed -n, head, tail, wc, and read-only git commands: git diff, git show, git status, git grep, git ls-files, plus the same git commands through git -C checkout.",
        "Do not write files, modify git state, run network commands, install dependencies, or chain commands through shell operators.",
        "Return only the JSON requested by the user prompt.",
    ]
    .join("\n")
}

fn command_approval_decision(command: &str) -> CommandApprovalDecision {
    if is_allowed_read_only_command(command) {
        CommandApprovalDecision::Accept
    } else {
        CommandApprovalDecision::Decline
    }
}

fn is_allowed_read_only_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() || contains_shell_control(trimmed) {
        return false;
    }

    let Some(tokens) = split_shell_words(trimmed) else {
        return false;
    };
    let Some(program) = tokens.first().map(String::as_str) else {
        return false;
    };

    match program {
        "pwd" => tokens.len() == 1,
        "ls" | "rg" | "grep" | "head" | "tail" | "wc" => true,
        "find" => !tokens.iter().any(|token| {
            matches!(
                token.as_str(),
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fprint" | "-fprintf"
            )
        }),
        "sed" => {
            tokens.iter().any(|token| token == "-n")
                && !tokens
                    .iter()
                    .any(|token| token == "-i" || token.starts_with("-i") || token == "--in-place")
        }
        "git" => is_allowed_git_command(&tokens),
        _ => false,
    }
}

fn is_allowed_git_command(tokens: &[String]) -> bool {
    let mut command_index = 1usize;
    if tokens.get(command_index).map(String::as_str) == Some("-C") {
        let Some(path) = tokens.get(command_index + 1).map(String::as_str) else {
            return false;
        };
        if path != "checkout" && !path.starts_with("checkout/") {
            return false;
        }
        command_index += 2;
    }

    matches!(
        tokens.get(command_index).map(String::as_str),
        Some("diff" | "show" | "status" | "grep" | "ls-files")
    )
}

fn contains_shell_control(command: &str) -> bool {
    command
        .chars()
        .any(|ch| matches!(ch, ';' | '|' | '&' | '>' | '<' | '`' | '$' | '\n' | '\r'))
}

fn split_shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }

    if escaped || quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        words.push(current);
    }
    Some(words)
}

fn record_path_hints_from_text(outcome: &mut CodexTurnOutcome, text: &str) {
    let mut existing = outcome
        .inspected_path_hints
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    for hint in path_hints_from_text(text) {
        if existing.insert(hint.clone()) {
            outcome.inspected_path_hints.push(hint);
            if outcome.inspected_path_hints.len() >= 8 {
                break;
            }
        }
    }
}

fn path_hints_from_text(text: &str) -> Vec<String> {
    split_shell_words(text)
        .unwrap_or_else(|| text.split_whitespace().map(str::to_string).collect())
        .into_iter()
        .filter_map(|token| {
            let trimmed = token.trim_matches(|ch: char| {
                matches!(ch, '"' | '\'' | ',' | ':' | '[' | ']' | '{' | '}')
            });
            if trimmed.starts_with('-') {
                return None;
            }
            let looks_like_path = trimmed.contains('/')
                || trimmed.ends_with(".rs")
                || trimmed.ends_with(".toml")
                || trimmed.ends_with(".json")
                || trimmed.ends_with(".md")
                || trimmed.ends_with(".yml")
                || trimmed.ends_with(".yaml");
            looks_like_path.then(|| short_text(trimmed, 120))
        })
        .take(8)
        .collect()
}

fn compatible_read_only_sandbox_policy() -> Value {
    json!({
        // Current app-server builds require a tagged sandbox policy object.
        "type": "readOnly",
        "networkAccess": false,
        // Older app-server builds accepted the legacy `mode` field.
        "mode": "read-only",
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_turn_start_params, command_approval_decision, compatible_read_only_sandbox_policy,
        is_allowed_read_only_command,
    };
    use codex_codes::protocol::CommandApprovalDecision;

    #[test]
    fn compatible_read_only_sandbox_policy_includes_new_and_legacy_fields() {
        let policy = compatible_read_only_sandbox_policy();

        assert_eq!(policy["type"], "readOnly");
        assert_eq!(policy["mode"], "read-only");
        assert_eq!(policy["networkAccess"], false);
    }

    #[test]
    fn turn_start_params_cover_old_and_new_codex_field_names() {
        let params = build_turn_start_params("thread-123", "hello");

        assert_eq!(params["threadId"], "thread-123");
        assert_eq!(params["effort"], "low");
        assert_eq!(params["reasoningEffort"], "low");
        assert_eq!(params.pointer("/input/0/type").unwrap(), "text");
        assert_eq!(params.pointer("/input/0/text").unwrap(), "hello");
        assert_eq!(params.pointer("/sandboxPolicy/type").unwrap(), "readOnly");
    }

    #[test]
    fn codex_allows_narrow_read_only_checkout_commands() {
        for command in [
            "pwd",
            "ls src",
            "find src -name '*.rs'",
            "rg ReviewPartner src",
            "grep -R ReviewPartner src",
            "sed -n '1,80p' src/review_partner.rs",
            "head -40 src/main.rs",
            "tail -20 Cargo.toml",
            "wc -l src/main.rs",
            "git diff -- src/review_partner.rs",
            "git show HEAD:src/main.rs",
            "git status --short",
            "git grep ReviewPartner",
            "git ls-files src",
            "git -C checkout diff -- src/review_partner.rs",
            "git -C checkout show HEAD:src/main.rs",
            "git -C checkout status --short",
            "git -C checkout grep ReviewPartner",
            "git -C checkout ls-files src",
        ] {
            assert!(is_allowed_read_only_command(command), "{command}");
        }
    }

    #[test]
    fn codex_denies_writes_redirection_and_chained_commands() {
        for command in [
            "rm -rf target",
            "git checkout -- src/main.rs",
            "git commit -m nope",
            "git reset --hard",
            "git -C /tmp status --short",
            "git -C checkout checkout -- src/main.rs",
            "sed -i 's/a/b/' src/main.rs",
            "find . -delete",
            "find . -exec rm {} \\;",
            "rg foo > /tmp/out",
            "pwd && rm -rf target",
            "ls; rm -rf target",
            "curl https://example.com",
        ] {
            assert!(!is_allowed_read_only_command(command), "{command}");
        }
    }

    #[test]
    fn command_approval_declines_unknown_payloads_by_default() {
        assert_eq!(
            command_approval_decision("rg ReviewPartner src"),
            CommandApprovalDecision::Accept
        );
        assert_eq!(
            command_approval_decision("git push origin main"),
            CommandApprovalDecision::Decline
        );
    }
}
