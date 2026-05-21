use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde_json::{json, Value};

use crate::managed_lsp::{self, ManagedServerKind};

const LSP_SHUTDOWN_GRACE: Duration = Duration::from_millis(40);
const LSP_DROP_SHUTDOWN_GRACE: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Default)]
pub struct LspServerCapabilities {
    pub hover_supported: bool,
    pub signature_help_supported: bool,
    pub definition_supported: bool,
    pub references_supported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspServerState {
    Ready,
    UnsupportedLanguage,
    MissingServer,
    CheckoutUnavailable,
    Error,
}

#[derive(Clone, Debug)]
pub struct LspServerStatus {
    pub state: LspServerState,
    pub language_id: Option<String>,
    pub command: Option<String>,
    pub capabilities: LspServerCapabilities,
    pub message: String,
}

impl LspServerStatus {
    pub fn ready(
        language_id: String,
        command: String,
        capabilities: LspServerCapabilities,
    ) -> Self {
        let capability_summary = capability_summary(&capabilities);
        let display_command = Path::new(&command)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(command.as_str());
        Self {
            state: LspServerState::Ready,
            language_id: Some(language_id),
            command: Some(command.clone()),
            capabilities,
            message: format!("{display_command} is ready ({capability_summary})."),
        }
    }

    pub fn unsupported_language(file_path: &str) -> Self {
        let label = Path::new(file_path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!(".{ext}"))
            .unwrap_or_else(|| file_path.to_string());

        Self {
            state: LspServerState::UnsupportedLanguage,
            language_id: None,
            command: None,
            capabilities: LspServerCapabilities::default(),
            message: format!("No configured language server for {label} files yet."),
        }
    }

    pub fn missing_server(language_id: &str, command: &str) -> Self {
        Self {
            state: LspServerState::MissingServer,
            language_id: Some(language_id.to_string()),
            command: Some(command.to_string()),
            capabilities: LspServerCapabilities::default(),
            message: format!("{command} is not available in PATH for {language_id} files."),
        }
    }

    pub fn checkout_unavailable(message: impl Into<String>) -> Self {
        Self {
            state: LspServerState::CheckoutUnavailable,
            language_id: None,
            command: None,
            capabilities: LspServerCapabilities::default(),
            message: message.into(),
        }
    }

    pub fn error(
        language_id: Option<String>,
        command: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            state: LspServerState::Error,
            language_id,
            command,
            capabilities: LspServerCapabilities::default(),
            message: message.into(),
        }
    }

    pub fn badge_label(&self) -> &'static str {
        match self.state {
            LspServerState::Ready => "LSP ready",
            LspServerState::UnsupportedLanguage => "LSP unsupported",
            LspServerState::MissingServer => "server missing",
            LspServerState::CheckoutUnavailable => "LSP blocked",
            LspServerState::Error => "LSP error",
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.state, LspServerState::Ready)
    }
}

#[derive(Clone, Debug)]
pub struct LspTextDocumentRequest {
    pub file_path: String,
    pub document_text: Arc<str>,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Default)]
pub struct LspSymbolDetails {
    pub hover: Option<LspHoverResult>,
    pub signature_help: Option<LspSignatureHelp>,
    pub definition_targets: Vec<LspDefinitionTarget>,
    pub reference_targets: Vec<LspReferenceTarget>,
    pub warnings: Vec<String>,
}

impl LspSymbolDetails {
    pub fn is_empty(&self) -> bool {
        self.hover.is_none()
            && self.signature_help.is_none()
            && self.definition_targets.is_empty()
            && self.reference_targets.is_empty()
            && self.warnings.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct LspHoverResult {
    pub markdown: String,
}

#[derive(Clone, Debug)]
pub struct LspSignatureHelp {
    pub label: String,
    pub documentation: Option<String>,
    pub active_parameter: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspDefinitionTarget {
    pub uri: String,
    pub path: String,
    pub package_path: Option<String>,
    pub line: usize,
    pub column: usize,
}

impl LspDefinitionTarget {
    pub fn display_path(&self) -> &str {
        self.package_path.as_deref().unwrap_or(self.path.as_str())
    }
}

pub type LspReferenceTarget = LspDefinitionTarget;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SessionKey {
    repo_root: PathBuf,
    language_id: String,
    command: String,
}

#[derive(Clone, Debug)]
struct ResolvedServerConfiguration {
    language_id: String,
    command: String,
    args: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct LanguageServerSpec {
    language_id: &'static str,
    command_candidates: &'static [&'static str],
    args: &'static [&'static str],
    managed: Option<ManagedServerKind>,
}

pub struct LspSessionManager {
    sessions: Mutex<HashMap<SessionKey, Arc<LspSession>>>,
}

impl LspSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn status_for_file(&self, repo_root: &Path, file_path: &str) -> LspServerStatus {
        let config = match resolve_server_configuration_for_path(repo_root, file_path) {
            Ok(config) => config,
            Err(status) => return status,
        };

        let session = match self.session_for(repo_root, &config) {
            Ok(session) => session,
            Err(error) => {
                return LspServerStatus::error(
                    Some(config.language_id.clone()),
                    Some(config.command.clone()),
                    error,
                );
            }
        };

        match session.capabilities() {
            Ok(capabilities) => {
                let mut status =
                    LspServerStatus::ready(config.language_id, config.command, capabilities);
                if status.language_id.as_deref() == Some("kotlin") {
                    if let Some(note) = kotlin_workspace_note(repo_root, file_path) {
                        status.message = format!("{} {note}", status.message);
                    }
                }
                status
            }
            Err(error) => {
                LspServerStatus::error(Some(config.language_id), Some(config.command), error)
            }
        }
    }

    pub fn symbol_details(
        &self,
        repo_root: &Path,
        request: &LspTextDocumentRequest,
    ) -> Result<LspSymbolDetails, String> {
        let config = resolve_server_configuration_for_path(repo_root, &request.file_path)
            .map_err(|status| status.message.clone())?;
        let session = self.session_for(repo_root, &config)?;
        let capabilities = session.capabilities()?;
        let mut details = session.symbol_details(request, &capabilities)?;
        if capabilities.definition_supported {
            if let Ok(targets) = session.definition_targets(request, &capabilities) {
                details.definition_targets = targets;
            }
        }
        if capabilities.references_supported {
            if let Ok(targets) = session.reference_targets(request, &capabilities) {
                details.reference_targets = targets;
            }
        }
        let warnings = lsp_analysis_warnings(&config.language_id, request, &details);
        details.warnings.extend(warnings);
        Ok(details)
    }

    pub fn definition(
        &self,
        repo_root: &Path,
        request: &LspTextDocumentRequest,
    ) -> Result<Vec<LspDefinitionTarget>, String> {
        let config = resolve_server_configuration_for_path(repo_root, &request.file_path)
            .map_err(|status| status.message.clone())?;
        let session = self.session_for(repo_root, &config)?;
        let capabilities = session.capabilities()?;
        session.definition_targets(request, &capabilities)
    }

    pub fn shutdown_all(&self) {
        let sessions = self
            .sessions
            .lock()
            .map(|mut sessions| {
                sessions
                    .drain()
                    .map(|(_, session)| session)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        for session in sessions {
            session.shutdown(LSP_SHUTDOWN_GRACE);
        }
    }

    fn session_for(
        &self,
        repo_root: &Path,
        config: &ResolvedServerConfiguration,
    ) -> Result<Arc<LspSession>, String> {
        let key = SessionKey {
            repo_root: repo_root.to_path_buf(),
            language_id: config.language_id.clone(),
            command: config.command.clone(),
        };

        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "LSP session registry is unavailable.".to_string())?;

        if let Some(existing) = sessions.get(&key) {
            if existing.is_running().unwrap_or(false) {
                return Ok(existing.clone());
            }
            sessions.remove(&key);
        }

        let session = Arc::new(LspSession::spawn(repo_root.to_path_buf(), config.clone())?);
        sessions.insert(key, session.clone());
        Ok(session)
    }
}

struct LspSession {
    repo_root: PathBuf,
    language_id: String,
    command: String,
    child_process_id: u32,
    next_request_id: AtomicI64,
    io: Mutex<LspIo>,
    capabilities: Mutex<Option<LspServerCapabilities>>,
    documents: Mutex<HashMap<PathBuf, OpenDocumentState>>,
}

struct LspIo {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    workspace_folders: Vec<Value>,
}

#[derive(Clone)]
struct OpenDocumentState {
    text: Arc<str>,
    version: i32,
}

impl LspSession {
    fn spawn(repo_root: PathBuf, config: ResolvedServerConfiguration) -> Result<Self, String> {
        let root_uri = file_uri(&repo_root)?;
        let root_name = repo_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repository")
            .to_string();
        let workspace_folders = vec![json!({
            "uri": root_uri,
            "name": root_name,
        })];

        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(&repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_lsp_child_process(&mut command);
        let mut child = command.spawn().map_err(|error| {
            format!(
                "Failed to start {} in {}: {error}",
                config.command,
                repo_root.display()
            )
        })?;
        let child_process_id = child.id();

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{} did not expose stdin.", config.command))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{} did not expose stdout.", config.command))?;

        Ok(Self {
            repo_root,
            language_id: config.language_id,
            command: config.command,
            child_process_id,
            next_request_id: AtomicI64::new(1),
            io: Mutex::new(LspIo {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                workspace_folders,
            }),
            capabilities: Mutex::new(None),
            documents: Mutex::new(HashMap::new()),
        })
    }

    fn shutdown(&self, grace: Duration) {
        match self.io.try_lock() {
            Ok(mut io) => shutdown_lsp_io(&mut io, &self.next_request_id, grace),
            Err(_) => terminate_lsp_process_tree_by_id(self.child_process_id, grace),
        }
    }

    fn is_running(&self) -> Result<bool, String> {
        let mut io = self
            .io
            .lock()
            .map_err(|_| "LSP session IO is unavailable.".to_string())?;
        Ok(io
            .child
            .try_wait()
            .map_err(|error| format!("Failed to inspect {}: {error}", self.command))?
            .is_none())
    }

    fn capabilities(&self) -> Result<LspServerCapabilities, String> {
        if let Some(capabilities) = self
            .capabilities
            .lock()
            .map_err(|_| "LSP capability cache is unavailable.".to_string())?
            .clone()
        {
            return Ok(capabilities);
        }

        let mut io = self
            .io
            .lock()
            .map_err(|_| "LSP session IO is unavailable.".to_string())?;

        if let Some(capabilities) = self
            .capabilities
            .lock()
            .map_err(|_| "LSP capability cache is unavailable.".to_string())?
            .clone()
        {
            return Ok(capabilities);
        }

        let workspace_folders = io.workspace_folders.clone();
        let root_uri = workspace_folders
            .first()
            .and_then(|folder| folder.get("uri"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(file_uri(&self.repo_root)?);

        let result = send_request(
            &mut io,
            &self.next_request_id,
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": {
                    "name": crate::branding::APP_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "rootUri": root_uri,
                "workspaceFolders": workspace_folders,
                "capabilities": {
                    "workspace": {
                        "configuration": true,
                        "workspaceFolders": true,
                    },
                    "textDocument": {
                        "synchronization": {
                            "dynamicRegistration": false,
                            "didSave": true,
                        },
                        "hover": {
                            "contentFormat": ["markdown", "plaintext"],
                        },
                        "definition": {
                            "linkSupport": true,
                        },
                        "references": {},
                        "signatureHelp": {
                            "signatureInformation": {
                                "documentationFormat": ["markdown", "plaintext"],
                                "parameterInformation": {
                                    "labelOffsetSupport": true,
                                },
                            },
                        },
                        "publishDiagnostics": {
                            "relatedInformation": true,
                        },
                    }
                }
            }),
        )?;

        let capabilities = parse_server_capabilities(&result);
        send_notification(&mut io, "initialized", json!({}))?;

        *self
            .capabilities
            .lock()
            .map_err(|_| "LSP capability cache is unavailable.".to_string())? =
            Some(capabilities.clone());

        Ok(capabilities)
    }

    fn symbol_details(
        &self,
        request: &LspTextDocumentRequest,
        capabilities: &LspServerCapabilities,
    ) -> Result<LspSymbolDetails, String> {
        let document_path = resolve_repo_document_path(&self.repo_root, &request.file_path)?;
        let document_uri = file_uri(&document_path)?;
        let position =
            lsp_position_for_text(request.document_text.as_ref(), request.line, request.column)?;

        let mut io = self
            .io
            .lock()
            .map_err(|_| "LSP session IO is unavailable.".to_string())?;
        self.ensure_document_open(&mut io, &document_path, request.document_text.as_ref())?;

        let hover = if capabilities.hover_supported {
            parse_hover_result(&send_request(
                &mut io,
                &self.next_request_id,
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": document_uri.clone() },
                    "position": position.clone(),
                }),
            )?)
        } else {
            None
        };

        let signature_help = if capabilities.signature_help_supported {
            parse_signature_help(&send_request(
                &mut io,
                &self.next_request_id,
                "textDocument/signatureHelp",
                json!({
                    "textDocument": { "uri": document_uri },
                    "position": position,
                }),
            )?)
        } else {
            None
        };

        Ok(LspSymbolDetails {
            hover,
            signature_help,
            definition_targets: Vec::new(),
            reference_targets: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn definition_targets(
        &self,
        request: &LspTextDocumentRequest,
        capabilities: &LspServerCapabilities,
    ) -> Result<Vec<LspDefinitionTarget>, String> {
        if !capabilities.definition_supported {
            return Ok(Vec::new());
        }

        let document_path = resolve_repo_document_path(&self.repo_root, &request.file_path)?;
        let document_uri = file_uri(&document_path)?;
        let position =
            lsp_position_for_text(request.document_text.as_ref(), request.line, request.column)?;

        let mut io = self
            .io
            .lock()
            .map_err(|_| "LSP session IO is unavailable.".to_string())?;
        self.ensure_document_open(&mut io, &document_path, request.document_text.as_ref())?;

        let result = send_request(
            &mut io,
            &self.next_request_id,
            "textDocument/definition",
            json!({
                "textDocument": { "uri": document_uri },
                "position": position,
            }),
        )?;

        Ok(parse_definition_targets(&self.repo_root, &result))
    }

    fn reference_targets(
        &self,
        request: &LspTextDocumentRequest,
        capabilities: &LspServerCapabilities,
    ) -> Result<Vec<LspReferenceTarget>, String> {
        if !capabilities.references_supported {
            return Ok(Vec::new());
        }

        let document_path = resolve_repo_document_path(&self.repo_root, &request.file_path)?;
        let document_uri = file_uri(&document_path)?;
        let position =
            lsp_position_for_text(request.document_text.as_ref(), request.line, request.column)?;

        let mut io = self
            .io
            .lock()
            .map_err(|_| "LSP session IO is unavailable.".to_string())?;
        self.ensure_document_open(&mut io, &document_path, request.document_text.as_ref())?;

        let result = send_request(
            &mut io,
            &self.next_request_id,
            "textDocument/references",
            json!({
                "textDocument": { "uri": document_uri },
                "position": position,
                "context": {
                    "includeDeclaration": false,
                }
            }),
        )?;

        Ok(parse_reference_targets(&self.repo_root, &result))
    }

    fn ensure_document_open(&self, io: &mut LspIo, path: &Path, text: &str) -> Result<(), String> {
        let uri = file_uri(path)?;
        let mut documents = self
            .documents
            .lock()
            .map_err(|_| "LSP document cache is unavailable.".to_string())?;

        match documents.get_mut(path) {
            Some(document) => {
                if document.text.as_ref() == text {
                    return Ok(());
                }

                let version = document.version.saturating_add(1);
                send_notification(
                    io,
                    "textDocument/didChange",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "version": version,
                        },
                        "contentChanges": [
                            {
                                "text": text,
                            }
                        ]
                    }),
                )?;
                document.text = Arc::<str>::from(text);
                document.version = version;
            }
            None => {
                send_notification(
                    io,
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": self.language_id.clone(),
                            "version": 1,
                            "text": text,
                        }
                    }),
                )?;
                documents.insert(
                    path.to_path_buf(),
                    OpenDocumentState {
                        text: Arc::<str>::from(text),
                        version: 1,
                    },
                );
            }
        }

        Ok(())
    }
}

impl Drop for LspSession {
    fn drop(&mut self) {
        self.shutdown(LSP_DROP_SHUTDOWN_GRACE);
    }
}

#[cfg(unix)]
fn configure_lsp_child_process(command: &mut Command) {
    // SAFETY: `pre_exec` runs in the child process after fork and before exec.
    // The closure only calls async-signal-safe `setpgid` and reports errno.
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
fn configure_lsp_child_process(_command: &mut Command) {}

fn shutdown_lsp_io(io: &mut LspIo, next_request_id: &AtomicI64, grace: Duration) {
    if child_has_exited(&mut io.child) {
        return;
    }

    let _ = send_lsp_shutdown_messages(io, next_request_id);
    if wait_for_child_exit(&mut io.child, grace) {
        return;
    }

    terminate_lsp_child_tree(&mut io.child, grace);
}

fn child_has_exited(child: &mut Child) -> bool {
    child.try_wait().ok().flatten().is_some()
}

fn send_lsp_shutdown_messages(io: &mut LspIo, next_request_id: &AtomicI64) -> Result<(), String> {
    let request_id = next_request_id.fetch_add(1, Ordering::Relaxed);
    write_message_to(
        &mut io.stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "shutdown",
        }),
    )?;
    write_message_to(
        &mut io.stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "exit",
        }),
    )
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> bool {
    let started_at = std::time::Instant::now();
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        if started_at.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn terminate_lsp_child_tree(child: &mut Child, grace: Duration) {
    signal_lsp_process_group(child.id(), libc::SIGTERM);
    if wait_for_child_exit(child, grace) {
        return;
    }
    signal_lsp_process_group(child.id(), libc::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate_lsp_child_tree(child: &mut Child, _grace: Duration) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_lsp_process_tree_by_id(process_id: u32, grace: Duration) {
    signal_lsp_process_group(process_id, libc::SIGTERM);
    thread::sleep(grace);
    signal_lsp_process_group(process_id, libc::SIGKILL);
}

#[cfg(not(unix))]
fn terminate_lsp_process_tree_by_id(_process_id: u32, _grace: Duration) {}

#[cfg(unix)]
fn signal_lsp_process_group(process_id: u32, signal: libc::c_int) {
    let process_group_id = process_id as libc::pid_t;
    if process_group_id <= 0 {
        return;
    }

    // SAFETY: LSP commands are launched into their own process group with pgid
    // equal to child pid. A negative pid signals that whole process group.
    unsafe {
        let _ = libc::kill(-process_group_id, signal);
    }
}

fn capability_summary(capabilities: &LspServerCapabilities) -> String {
    let mut features = Vec::new();
    if capabilities.hover_supported {
        features.push("hover");
    }
    if capabilities.signature_help_supported {
        features.push("signature");
    }
    if capabilities.definition_supported {
        features.push("definition");
    }
    if capabilities.references_supported {
        features.push("references");
    }

    if features.is_empty() {
        "no supported requests yet".to_string()
    } else {
        features.join(", ")
    }
}

fn kotlin_workspace_note(repo_root: &Path, file_path: &str) -> Option<String> {
    let relative_path = validated_repo_relative_path(file_path).ok()?;
    let document_path = repo_root.join(relative_path);
    let document_dir = document_path.parent().unwrap_or(repo_root);
    let settings_file = find_nearest_file_upwards(
        document_dir,
        repo_root,
        &["settings.gradle.kts", "settings.gradle"],
    );
    let build_file = find_nearest_file_upwards(
        document_dir,
        repo_root,
        &["build.gradle.kts", "build.gradle"],
    );

    let gradle_summary = match (settings_file.as_ref(), build_file.as_ref()) {
        (Some(settings), Some(build)) => format!(
            "nearest Gradle settings {}; nearest build file {}",
            display_repo_relative_path(repo_root, settings),
            display_repo_relative_path(repo_root, build)
        ),
        (Some(settings), None) => format!(
            "nearest Gradle settings {}; no build.gradle(.kts) found above the file",
            display_repo_relative_path(repo_root, settings)
        ),
        (None, Some(build)) => format!(
            "nearest build file {}; no settings.gradle(.kts) found above the file",
            display_repo_relative_path(repo_root, build)
        ),
        (None, None) => {
            "no Gradle settings/build file found above the file in this checkout".to_string()
        }
    };

    Some(format!(
        "Kotlin workspace root {}; {gradle_summary}.",
        display_repo_relative_path(repo_root, repo_root)
    ))
}

fn find_nearest_file_upwards(start: &Path, repo_root: &Path, names: &[&str]) -> Option<PathBuf> {
    let mut current = start;
    loop {
        for name in names {
            let candidate = current.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        if current == repo_root {
            break;
        }
        current = current.parent()?;
        if !current.starts_with(repo_root) {
            break;
        }
    }

    None
}

fn display_repo_relative_path(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| ".".to_string())
}

fn resolve_server_configuration_for_path(
    repo_root: &Path,
    file_path: &str,
) -> Result<ResolvedServerConfiguration, LspServerStatus> {
    let Some(spec) = language_server_spec_for_path(file_path) else {
        return Err(LspServerStatus::unsupported_language(file_path));
    };

    let cache_key = format!("{}::{}", repo_root.display(), spec.language_id);
    if let Some(config) = resolved_server_configuration_cache()
        .lock()
        .map_err(|_| {
            LspServerStatus::error(
                Some(spec.language_id.to_string()),
                None,
                "LSP resolver cache is unavailable.",
            )
        })?
        .get(&cache_key)
        .cloned()
    {
        return Ok(config);
    }

    let preferred_command = spec
        .command_candidates
        .first()
        .copied()
        .unwrap_or("language-server");
    let config = if let Some(command) = resolve_server_command(spec.command_candidates) {
        ResolvedServerConfiguration {
            language_id: spec.language_id.to_string(),
            command,
            args: spec.args.iter().map(|arg| (*arg).to_string()).collect(),
        }
    } else if let Some(kind) = spec.managed {
        let managed = managed_lsp::resolve_managed_server(repo_root, kind).map_err(|error| {
            LspServerStatus::error(
                Some(spec.language_id.to_string()),
                Some(managed_lsp::managed_server_display_name(kind).to_string()),
                error,
            )
        })?;
        ResolvedServerConfiguration {
            language_id: spec.language_id.to_string(),
            command: managed.command,
            args: managed.args,
        }
    } else {
        return Err(LspServerStatus::missing_server(
            spec.language_id,
            preferred_command,
        ));
    };

    resolved_server_configuration_cache()
        .lock()
        .map_err(|_| {
            LspServerStatus::error(
                Some(spec.language_id.to_string()),
                None,
                "LSP resolver cache is unavailable.",
            )
        })?
        .insert(cache_key, config.clone());

    Ok(config)
}

fn resolved_server_configuration_cache(
) -> &'static Mutex<HashMap<String, ResolvedServerConfiguration>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ResolvedServerConfiguration>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn language_server_spec_for_path(file_path: &str) -> Option<LanguageServerSpec> {
    let path = Path::new(file_path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if file_name == "dockerfile" || file_name.ends_with(".dockerfile") {
        return Some(LanguageServerSpec {
            language_id: "dockerfile",
            command_candidates: &["docker-langserver"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::DockerfileLanguageServer),
        });
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();

    match extension.as_str() {
        "rs" => Some(LanguageServerSpec {
            language_id: "rust",
            command_candidates: &["rust-analyzer"],
            args: &[],
            managed: Some(ManagedServerKind::RustAnalyzer),
        }),
        "ts" => Some(LanguageServerSpec {
            language_id: "typescript",
            command_candidates: &["typescript-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::TypescriptLanguageServer),
        }),
        "tsx" => Some(LanguageServerSpec {
            language_id: "typescriptreact",
            command_candidates: &["typescript-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::TypescriptLanguageServer),
        }),
        "js" | "mjs" | "cjs" => Some(LanguageServerSpec {
            language_id: "javascript",
            command_candidates: &["typescript-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::TypescriptLanguageServer),
        }),
        "jsx" => Some(LanguageServerSpec {
            language_id: "javascriptreact",
            command_candidates: &["typescript-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::TypescriptLanguageServer),
        }),
        "py" => Some(LanguageServerSpec {
            language_id: "python",
            command_candidates: &["pyright-langserver", "basedpyright-langserver"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::Pyright),
        }),
        "go" => Some(LanguageServerSpec {
            language_id: "go",
            command_candidates: &["gopls"],
            args: &[],
            managed: Some(ManagedServerKind::Gopls),
        }),
        "kt" | "kts" => Some(LanguageServerSpec {
            language_id: "kotlin",
            command_candidates: &["kotlin-language-server", "kotlin-lsp.sh"],
            args: &[],
            managed: Some(ManagedServerKind::KotlinLsp),
        }),
        "java" => Some(LanguageServerSpec {
            language_id: "java",
            command_candidates: &["jdtls"],
            args: &[],
            managed: Some(ManagedServerKind::Jdtls),
        }),
        "cs" | "csx" => Some(LanguageServerSpec {
            language_id: "csharp",
            command_candidates: &["csharp-ls"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::Roslyn),
        }),
        "html" | "htm" => Some(LanguageServerSpec {
            language_id: "html",
            command_candidates: &["vscode-html-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::VscodeHtmlLanguageServer),
        }),
        "css" => Some(LanguageServerSpec {
            language_id: "css",
            command_candidates: &["vscode-css-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::VscodeCssLanguageServer),
        }),
        "scss" => Some(LanguageServerSpec {
            language_id: "scss",
            command_candidates: &["vscode-css-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::VscodeCssLanguageServer),
        }),
        "less" => Some(LanguageServerSpec {
            language_id: "less",
            command_candidates: &["vscode-css-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::VscodeCssLanguageServer),
        }),
        "json" => Some(LanguageServerSpec {
            language_id: "json",
            command_candidates: &["vscode-json-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::VscodeJsonLanguageServer),
        }),
        "jsonc" => Some(LanguageServerSpec {
            language_id: "jsonc",
            command_candidates: &["vscode-json-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::VscodeJsonLanguageServer),
        }),
        "md" | "markdown" => Some(LanguageServerSpec {
            language_id: "markdown",
            command_candidates: &["vscode-markdown-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::VscodeMarkdownLanguageServer),
        }),
        "yaml" | "yml" => Some(LanguageServerSpec {
            language_id: "yaml",
            command_candidates: &["yaml-language-server"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::YamlLanguageServer),
        }),
        "sh" | "bash" | "zsh" => Some(LanguageServerSpec {
            language_id: "shellscript",
            command_candidates: &["bash-language-server"],
            args: &["start"],
            managed: Some(ManagedServerKind::BashLanguageServer),
        }),
        "php" => Some(LanguageServerSpec {
            language_id: "php",
            command_candidates: &["intelephense"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::Intelephense),
        }),
        "svelte" => Some(LanguageServerSpec {
            language_id: "svelte",
            command_candidates: &["svelteserver"],
            args: &["--stdio"],
            managed: Some(ManagedServerKind::SvelteLanguageServer),
        }),
        "c" => Some(LanguageServerSpec {
            language_id: "c",
            command_candidates: &["clangd"],
            args: &[],
            managed: None,
        }),
        "h" => Some(LanguageServerSpec {
            language_id: "c",
            command_candidates: &["clangd"],
            args: &[],
            managed: None,
        }),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some(LanguageServerSpec {
            language_id: "cpp",
            command_candidates: &["clangd"],
            args: &[],
            managed: None,
        }),
        "lua" => Some(LanguageServerSpec {
            language_id: "lua",
            command_candidates: &["lua-language-server"],
            args: &[],
            managed: None,
        }),
        "swift" => Some(LanguageServerSpec {
            language_id: "swift",
            command_candidates: &["sourcekit-lsp"],
            args: &[],
            managed: None,
        }),
        "zig" => Some(LanguageServerSpec {
            language_id: "zig",
            command_candidates: &["zls"],
            args: &[],
            managed: None,
        }),
        "nix" => Some(LanguageServerSpec {
            language_id: "nix",
            command_candidates: &["nil", "nixd"],
            args: &[],
            managed: None,
        }),
        "toml" => Some(LanguageServerSpec {
            language_id: "toml",
            command_candidates: &["taplo"],
            args: &["lsp", "stdio"],
            managed: None,
        }),
        "tf" | "tfvars" => Some(LanguageServerSpec {
            language_id: "terraform",
            command_candidates: &["terraform-ls"],
            args: &["serve"],
            managed: None,
        }),
        "graphql" | "gql" => Some(LanguageServerSpec {
            language_id: "graphql",
            command_candidates: &["graphql-lsp"],
            args: &["server", "-m", "stream"],
            managed: None,
        }),
        "sql" => Some(LanguageServerSpec {
            language_id: "sql",
            command_candidates: &["sqls"],
            args: &[],
            managed: None,
        }),
        "rb" => Some(LanguageServerSpec {
            language_id: "ruby",
            command_candidates: &["ruby-lsp"],
            args: &[],
            managed: None,
        }),
        _ => None,
    }
}

fn resolve_server_command(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|candidate| {
        let resolved = resolve_binary_path(candidate)?;
        if command_candidate_is_usable(candidate, &resolved) {
            Some(resolved.to_string_lossy().to_string())
        } else {
            None
        }
    })
}

fn resolve_binary_path(binary: &str) -> Option<PathBuf> {
    let binary_path = Path::new(binary);
    if binary_path.components().count() > 1 {
        return binary_path.is_file().then(|| binary_path.to_path_buf());
    }

    env::var_os("PATH")
        .map(|paths| {
            env::split_paths(&paths).find_map(|directory| {
                executable_name_candidates(binary).find_map(|name| {
                    let candidate = directory.join(name);
                    candidate.is_file().then_some(candidate)
                })
            })
        })
        .unwrap_or(None)
}

fn executable_name_candidates(binary: &str) -> Box<dyn Iterator<Item = String> + '_> {
    if cfg!(windows) && Path::new(binary).extension().is_none() {
        Box::new(
            [
                binary.to_string(),
                format!("{binary}.exe"),
                format!("{binary}.cmd"),
                format!("{binary}.bat"),
            ]
            .into_iter(),
        )
    } else {
        Box::new(std::iter::once(binary.to_string()))
    }
}

fn command_candidate_is_usable(binary: &str, resolved_path: &Path) -> bool {
    if !resolved_path.is_file() {
        return false;
    }

    if !is_rust_analyzer_candidate(binary, resolved_path) {
        return true;
    }

    Command::new(resolved_path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn is_rust_analyzer_candidate(binary: &str, resolved_path: &Path) -> bool {
    binary.eq_ignore_ascii_case("rust-analyzer")
        || resolved_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| {
                name.eq_ignore_ascii_case("rust-analyzer")
                    || name.eq_ignore_ascii_case("rust-analyzer.exe")
            })
            .unwrap_or(false)
}

fn parse_server_capabilities(result: &Value) -> LspServerCapabilities {
    let capabilities = result.get("capabilities").unwrap_or(result);

    LspServerCapabilities {
        hover_supported: capability_is_enabled(capabilities.get("hoverProvider")),
        signature_help_supported: capability_is_enabled(capabilities.get("signatureHelpProvider")),
        definition_supported: capability_is_enabled(capabilities.get("definitionProvider")),
        references_supported: capability_is_enabled(capabilities.get("referencesProvider")),
    }
}

fn parse_hover_result(result: &Value) -> Option<LspHoverResult> {
    let markdown = markdown_from_lsp_value(Some(result.get("contents")?))?;
    let markdown = markdown.trim().to_string();
    if markdown.is_empty() {
        None
    } else {
        Some(LspHoverResult { markdown })
    }
}

fn lsp_analysis_warnings(
    language_id: &str,
    request: &LspTextDocumentRequest,
    details: &LspSymbolDetails,
) -> Vec<String> {
    if language_id == "kotlin" {
        kotlin_lsp_analysis_warnings(request, details)
    } else {
        Vec::new()
    }
}

fn kotlin_lsp_analysis_warnings(
    request: &LspTextDocumentRequest,
    details: &LspSymbolDetails,
) -> Vec<String> {
    if details.is_empty() {
        return word_at_text_position(request.document_text.as_ref(), request.line, request.column)
            .map(|symbol| {
                vec![format!(
                    "Kotlin LSP returned no hover, definition, or reference data for `{symbol}`. If this is an imported dependency symbol, the current LSP workspace may be missing its Gradle/classpath model."
                )]
            })
            .unwrap_or_default();
    }

    let Some(hover) = details.hover.as_ref() else {
        return Vec::new();
    };
    let Some((variable, reported_type)) = parse_kotlin_variable_hover_type(&hover.markdown) else {
        return Vec::new();
    };
    let Some(line_text) = request
        .document_text
        .lines()
        .nth(request.line.saturating_sub(1))
    else {
        return Vec::new();
    };
    let Some(initializer_call) = kotlin_initializer_call_name(line_text, &variable) else {
        return Vec::new();
    };

    if initializer_call == reported_type && starts_lowercase_identifier(&reported_type) {
        vec![format!(
            "Kotlin LSP reported `{reported_type}` as the inferred type for `{variable}`, matching the initializer call name. That usually means the server could not resolve that call from the current Gradle/classpath model."
        )]
    } else {
        Vec::new()
    }
}

fn parse_kotlin_variable_hover_type(markdown: &str) -> Option<(String, String)> {
    let line = first_lsp_code_line(markdown)?;
    let line = line.trim();
    let declaration = line
        .strip_prefix("val ")
        .or_else(|| line.strip_prefix("var "))?;
    let (name, type_name) = declaration.split_once(':')?;
    let name = name.trim();
    let type_name = type_name.trim();
    if name.is_empty() || type_name.is_empty() {
        return None;
    }

    Some((name.to_string(), type_name.to_string()))
}

fn first_lsp_code_line(markdown: &str) -> Option<&str> {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| !line.starts_with("```") && !line.starts_with("~~~"))
}

fn kotlin_initializer_call_name(line_text: &str, variable: &str) -> Option<String> {
    let (left, right) = line_text.split_once('=')?;
    let left = left.trim();
    let declaration = left
        .strip_prefix("val ")
        .or_else(|| left.strip_prefix("var "))?;
    if declaration.trim() != variable {
        return None;
    }

    leading_identifier(right.trim())
}

fn word_at_text_position(text: &str, line: usize, column: usize) -> Option<String> {
    let line_text = text.lines().nth(line.checked_sub(1)?)?;
    let target = column.checked_sub(1)?;
    let byte_index = byte_index_for_char_column(line_text, target)?;
    word_at_byte_index(line_text, byte_index)
}

fn byte_index_for_char_column(line_text: &str, column_zero_based: usize) -> Option<usize> {
    if column_zero_based == line_text.chars().count() {
        return Some(line_text.len());
    }
    line_text
        .char_indices()
        .nth(column_zero_based)
        .map(|(byte_index, _)| byte_index)
}

fn word_at_byte_index(line_text: &str, byte_index: usize) -> Option<String> {
    if byte_index > line_text.len() || !line_text.is_char_boundary(byte_index) {
        return None;
    }

    let mut start = byte_index;
    while start > 0 {
        let (previous, character) = line_text[..start].char_indices().next_back()?;
        if !is_identifier_character(character) {
            break;
        }
        start = previous;
    }

    let mut end = byte_index;
    for (offset, character) in line_text[byte_index..].char_indices() {
        if !is_identifier_character(character) {
            break;
        }
        end = byte_index + offset + character.len_utf8();
    }

    (start < end).then(|| line_text[start..end].to_string())
}

fn leading_identifier(text: &str) -> Option<String> {
    let identifier = text
        .chars()
        .take_while(|character| is_identifier_character(*character))
        .collect::<String>();
    (!identifier.is_empty()).then_some(identifier)
}

fn starts_lowercase_identifier(text: &str) -> bool {
    text.chars()
        .next()
        .map(|character| character == '_' || character.is_lowercase())
        .unwrap_or(false)
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn parse_signature_help(result: &Value) -> Option<LspSignatureHelp> {
    let signatures = result.get("signatures")?.as_array()?;
    if signatures.is_empty() {
        return None;
    }

    let active_signature = result
        .get("activeSignature")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(0)
        .min(signatures.len().saturating_sub(1));
    let signature = signatures.get(active_signature)?;
    let label = signature.get("label")?.as_str()?.trim().to_string();
    if label.is_empty() {
        return None;
    }

    let active_parameter_index = signature
        .get("activeParameter")
        .or_else(|| result.get("activeParameter"))
        .and_then(Value::as_u64)
        .map(|value| value as usize);

    Some(LspSignatureHelp {
        label: label.clone(),
        documentation: markdown_from_lsp_value(signature.get("documentation"))
            .map(|markdown| markdown.trim().to_string())
            .filter(|markdown| !markdown.is_empty()),
        active_parameter: active_parameter_index
            .and_then(|index| parse_signature_parameter_label(signature, index, &label)),
    })
}

fn parse_signature_parameter_label(
    signature: &Value,
    index: usize,
    signature_label: &str,
) -> Option<String> {
    let parameter = signature.get("parameters")?.as_array()?.get(index)?;
    match parameter.get("label")? {
        Value::String(label) => Some(label.to_string()),
        Value::Array(range) if range.len() == 2 => {
            let start = range.first()?.as_u64()? as usize;
            let end = range.get(1)?.as_u64()? as usize;
            slice_utf16_range(signature_label, start, end)
        }
        _ => None,
    }
}

fn parse_definition_targets(repo_root: &Path, result: &Value) -> Vec<LspDefinitionTarget> {
    let mut package_cache = HashMap::new();
    match result {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| parse_definition_target(repo_root, item, &mut package_cache))
            .collect(),
        _ => parse_definition_target(repo_root, result, &mut package_cache)
            .into_iter()
            .collect(),
    }
}

fn parse_reference_targets(repo_root: &Path, result: &Value) -> Vec<LspReferenceTarget> {
    parse_definition_targets(repo_root, result)
}

fn parse_definition_target(
    repo_root: &Path,
    value: &Value,
    package_cache: &mut HashMap<String, Option<String>>,
) -> Option<LspDefinitionTarget> {
    let uri = value
        .get("targetUri")
        .or_else(|| value.get("uri"))
        .and_then(Value::as_str)?;
    let path = path_from_file_uri(repo_root, uri)?;
    let package_path = package_path_for_target(repo_root, path.as_str(), package_cache);
    let start = value
        .pointer("/targetSelectionRange/start")
        .or_else(|| value.pointer("/targetRange/start"))
        .or_else(|| value.pointer("/range/start"))?;

    Some(LspDefinitionTarget {
        uri: uri.to_string(),
        path,
        package_path,
        line: start.get("line").and_then(Value::as_u64)? as usize + 1,
        column: start.get("character").and_then(Value::as_u64)? as usize + 1,
    })
}

fn package_path_for_target(
    repo_root: &Path,
    path: &str,
    package_cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    if let Some(package_path) = package_cache.get(path) {
        return package_path.clone();
    }

    let package_path = fs::read_to_string(repo_root.join(path))
        .ok()
        .and_then(|source| package_path_from_source(&source));
    package_cache.insert(path.to_string(), package_path.clone());
    package_path
}

fn package_path_from_source(source: &str) -> Option<String> {
    let mut in_block_comment = false;
    for line in source.lines().take(120) {
        let trimmed = line.trim_start();
        if in_block_comment {
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
            continue;
        }
        if let Some(package_path) = package_path_from_line(line) {
            return Some(package_path);
        }
    }
    None
}

fn package_path_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
    {
        return None;
    }

    package_path_after_keyword(trimmed, "package")
        .or_else(|| package_path_after_keyword(trimmed, "namespace"))
}

fn package_path_after_keyword(line: &str, keyword: &str) -> Option<String> {
    let rest = line.strip_prefix(keyword)?;
    if !rest
        .chars()
        .next()
        .map(char::is_whitespace)
        .unwrap_or(false)
    {
        return None;
    }

    let mut value = String::new();
    for character in rest.trim_start().chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '.' | '`' | '\\') {
            value.push(character);
        } else {
            break;
        }
    }

    let value = value.replace('\\', ".").replace('`', "");
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

fn markdown_from_lsp_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        Value::String(text) => Some(text.to_string()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(|item| markdown_from_lsp_value(Some(item)))
                .filter(|item| !item.trim().is_empty())
                .collect::<Vec<_>>();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n\n"))
            }
        }
        Value::Object(map) => {
            if let Some(value) = map.get("value").and_then(Value::as_str) {
                if let Some(language) = map.get("language").and_then(Value::as_str) {
                    let fence = if language.is_empty() {
                        "```".to_string()
                    } else {
                        format!("```{language}")
                    };
                    return Some(format!("{fence}\n{value}\n```"));
                }

                if map.get("kind").is_some() {
                    return Some(value.to_string());
                }
            }

            None
        }
        _ => None,
    }
}

fn capability_is_enabled(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(enabled)) => *enabled,
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

fn send_request(
    io: &mut LspIo,
    next_request_id: &AtomicI64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let request_id = next_request_id.fetch_add(1, Ordering::Relaxed);
    let message = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    });

    write_message_to(&mut io.stdin, &message)?;

    loop {
        let response = read_message_from(&mut io.stdout)?.ok_or_else(|| {
            format!(
                "{} closed stdout while waiting for {method}.",
                io.child.id()
            )
        })?;
        if handle_server_request(io, &response)? {
            continue;
        }
        if response.get("id").and_then(Value::as_i64) != Some(request_id) {
            continue;
        }

        if let Some(error) = response.get("error") {
            return Err(format_lsp_error(error));
        }

        return Ok(response.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn send_notification(io: &mut LspIo, method: &str, params: Value) -> Result<(), String> {
    write_message_to(
        &mut io.stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }),
    )
}

fn handle_server_request(io: &mut LspIo, message: &Value) -> Result<bool, String> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(false);
    };

    if let Some(id) = message.get("id").cloned() {
        let result = server_request_result(method, message.get("params"), &io.workspace_folders);
        send_response(io, id, result)?;
    }

    match method {
        "window/logMessage" | "window/showMessage" | "textDocument/publishDiagnostics" => {}
        _ => {}
    }

    Ok(true)
}

fn server_request_result(
    method: &str,
    params: Option<&Value>,
    workspace_folders: &[Value],
) -> Value {
    match method {
        "workspace/configuration" => workspace_configuration_result(params),
        "workspace/workspaceFolders" => Value::Array(workspace_folders.to_vec()),
        "workspace/applyEdit" => json!({ "applied": false }),
        "window/showMessageRequest" => Value::Null,
        "window/workDoneProgress/create" => Value::Null,
        "client/registerCapability" | "client/unregisterCapability" => Value::Null,
        _ => Value::Null,
    }
}

fn workspace_configuration_result(params: Option<&Value>) -> Value {
    let item_count = params
        .and_then(|params| params.get("items"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    Value::Array((0..item_count).map(|_| json!({})).collect())
}

fn send_response(io: &mut LspIo, id: Value, result: Value) -> Result<(), String> {
    write_message_to(
        &mut io.stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
}

fn write_message_to<W: Write>(writer: &mut W, value: &Value) -> Result<(), String> {
    let payload = serde_json::to_vec(value)
        .map_err(|error| format!("Failed to encode LSP message: {error}"))?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", payload.len()).as_bytes())
        .and_then(|_| writer.write_all(&payload))
        .and_then(|_| writer.flush())
        .map_err(|error| format!("Failed to write LSP message: {error}"))
}

fn read_message_from<R: BufRead>(reader: &mut R) -> Result<Option<Value>, String> {
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| format!("Failed to read LSP header: {error}"))?;

        if read == 0 {
            return if content_length.is_some() {
                Err("Unexpected end of stream while reading LSP headers.".to_string())
            } else {
                Ok(None)
            };
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| format!("Invalid Content-Length header: {error}"))?,
            );
        }
    }

    let content_length = content_length
        .ok_or_else(|| "Missing Content-Length header in LSP message.".to_string())?;
    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("Failed to read LSP body: {error}"))?;

    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| format!("Failed to decode LSP JSON payload: {error}"))
}

fn format_lsp_error(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Language server returned an error: {error}"))
}

fn resolve_repo_document_path(repo_root: &Path, file_path: &str) -> Result<PathBuf, String> {
    Ok(repo_root.join(validated_repo_relative_path(file_path)?))
}

fn validated_repo_relative_path(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    let mut result = PathBuf::new();

    for component in candidate.components() {
        match component {
            Component::Normal(segment) => result.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("Unsupported repository-relative path '{path}'."));
            }
        }
    }

    if result.as_os_str().is_empty() {
        return Err("The repository-relative file path is empty.".to_string());
    }

    Ok(result)
}

fn file_uri(path: &Path) -> Result<String, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("Failed to resolve the current directory: {error}"))?
            .join(path)
    };
    let encoded = percent_encode_path(&absolute.to_string_lossy().replace('\\', "/"));
    if cfg!(windows) {
        Ok(format!("file:///{}", encoded.trim_start_matches('/')))
    } else {
        Ok(format!("file://{encoded}"))
    }
}

fn path_from_file_uri(repo_root: &Path, uri: &str) -> Option<String> {
    let path = decode_file_uri(uri)?;
    path.strip_prefix(repo_root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .or_else(|| Some(path.to_string_lossy().replace('\\', "/")))
}

fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    #[cfg(windows)]
    let encoded = encoded.strip_prefix('/').unwrap_or(encoded);
    percent_decode_path(encoded).map(PathBuf::from)
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{:02X}", byte)),
        }
    }
    encoded
}

fn percent_decode_path(path: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.as_bytes().iter().copied();

    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next()?;
            let low = bytes.next()?;
            let high = (high as char).to_digit(16)? as u8;
            let low = (low as char).to_digit(16)? as u8;
            decoded.push((high << 4) | low);
        } else {
            decoded.push(byte);
        }
    }

    String::from_utf8(decoded).ok()
}

fn lsp_position_for_text(text: &str, line: usize, column: usize) -> Result<Value, String> {
    let line_index = line
        .checked_sub(1)
        .ok_or_else(|| "LSP line numbers must be 1-based.".to_string())?;
    let character = utf16_character_for_text_position(text, line, column)?;

    Ok(json!({
        "line": line_index,
        "character": character,
    }))
}

fn utf16_character_for_text_position(
    text: &str,
    line: usize,
    column: usize,
) -> Result<usize, String> {
    let line_text = text
        .lines()
        .nth(line.saturating_sub(1))
        .ok_or_else(|| format!("Line {line} is outside the current document."))?;
    utf16_character_for_line_column(line_text, column)
}

fn utf16_character_for_line_column(line_text: &str, column: usize) -> Result<usize, String> {
    let target = column
        .checked_sub(1)
        .ok_or_else(|| "LSP columns must be 1-based.".to_string())?;
    let line_len = line_text.chars().count();
    if target > line_len {
        return Err(format!(
            "Column {column} is outside the current line (max {}).",
            line_len + 1
        ));
    }

    Ok(line_text
        .chars()
        .take(target)
        .map(char::len_utf16)
        .sum::<usize>())
}

fn slice_utf16_range(text: &str, start: usize, end: usize) -> Option<String> {
    if start > end {
        return None;
    }

    let mut utf16_offset = 0usize;
    let mut start_byte = None;
    let mut end_byte = None;

    for (byte_index, ch) in text.char_indices() {
        if start_byte.is_none() && utf16_offset == start {
            start_byte = Some(byte_index);
        }
        if end_byte.is_none() && utf16_offset == end {
            end_byte = Some(byte_index);
            break;
        }
        utf16_offset += ch.len_utf16();
    }

    if start_byte.is_none() && utf16_offset == start {
        start_byte = Some(text.len());
    }
    if end_byte.is_none() && utf16_offset == end {
        end_byte = Some(text.len());
    }

    Some(text.get(start_byte?..end_byte?)?.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Cursor,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[cfg(unix)]
    fn unique_test_directory(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "remiss-lsp-{prefix}-{nanos}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("failed to create temp directory");
        path
    }

    #[cfg(unix)]
    fn write_executable_script(path: &Path, body: &str) {
        fs::write(path, body).expect("failed to write script");
        let mut permissions = fs::metadata(path)
            .expect("failed to stat script")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("failed to chmod script");
    }

    #[test]
    fn resolves_rust_server_spec() {
        let spec = language_server_spec_for_path("src/lib.rs").expect("expected spec");
        assert_eq!(spec.language_id, "rust");
        assert_eq!(spec.command_candidates, ["rust-analyzer"]);
        assert!(matches!(
            spec.managed,
            Some(ManagedServerKind::RustAnalyzer)
        ));
    }

    #[test]
    fn resolves_typescript_server_spec() {
        let spec = language_server_spec_for_path("src/app.tsx").expect("expected spec");
        assert_eq!(spec.language_id, "typescriptreact");
        assert_eq!(spec.command_candidates, ["typescript-language-server"]);
        assert!(matches!(
            spec.managed,
            Some(ManagedServerKind::TypescriptLanguageServer)
        ));
    }

    #[test]
    fn resolves_python_server_spec() {
        let spec = language_server_spec_for_path("src/app.py").expect("expected spec");
        assert_eq!(spec.language_id, "python");
        assert_eq!(
            spec.command_candidates,
            ["pyright-langserver", "basedpyright-langserver"]
        );
        assert!(matches!(spec.managed, Some(ManagedServerKind::Pyright)));
    }

    #[test]
    fn resolves_go_server_spec() {
        let spec = language_server_spec_for_path("src/main.go").expect("expected spec");
        assert_eq!(spec.language_id, "go");
        assert_eq!(spec.command_candidates, ["gopls"]);
        assert!(matches!(spec.managed, Some(ManagedServerKind::Gopls)));
    }

    #[test]
    fn resolves_kotlin_server_spec() {
        let spec = language_server_spec_for_path("src/App.kt").expect("expected spec");
        assert_eq!(spec.language_id, "kotlin");
        assert_eq!(
            spec.command_candidates,
            ["kotlin-language-server", "kotlin-lsp.sh"]
        );
        assert!(matches!(spec.managed, Some(ManagedServerKind::KotlinLsp)));
    }

    #[test]
    fn resolves_java_server_spec() {
        let spec = language_server_spec_for_path("src/Main.java").expect("expected spec");
        assert_eq!(spec.language_id, "java");
        assert_eq!(spec.command_candidates, ["jdtls"]);
        assert!(matches!(spec.managed, Some(ManagedServerKind::Jdtls)));
    }

    #[test]
    fn resolves_csharp_server_spec() {
        let spec = language_server_spec_for_path("src/Program.cs").expect("expected spec");
        assert_eq!(spec.language_id, "csharp");
        assert_eq!(spec.command_candidates, ["csharp-ls"]);
        assert!(matches!(spec.managed, Some(ManagedServerKind::Roslyn)));
    }

    #[test]
    fn resolves_managed_web_and_config_server_specs() {
        let cases = [
            (
                "src/index.html",
                "html",
                ManagedServerKind::VscodeHtmlLanguageServer,
            ),
            (
                "src/styles.scss",
                "scss",
                ManagedServerKind::VscodeCssLanguageServer,
            ),
            (
                "package.json",
                "json",
                ManagedServerKind::VscodeJsonLanguageServer,
            ),
            (
                "README.md",
                "markdown",
                ManagedServerKind::VscodeMarkdownLanguageServer,
            ),
            ("config.yaml", "yaml", ManagedServerKind::YamlLanguageServer),
            (
                "scripts/build.sh",
                "shellscript",
                ManagedServerKind::BashLanguageServer,
            ),
            (
                "Dockerfile",
                "dockerfile",
                ManagedServerKind::DockerfileLanguageServer,
            ),
            ("src/index.php", "php", ManagedServerKind::Intelephense),
            (
                "src/App.svelte",
                "svelte",
                ManagedServerKind::SvelteLanguageServer,
            ),
        ];

        for (path, language_id, managed) in cases {
            let spec = language_server_spec_for_path(path).expect("expected spec");
            assert_eq!(spec.language_id, language_id);
            assert!(matches!(spec.managed, Some(kind) if kind == managed));
        }
    }

    #[test]
    fn resolves_path_based_native_server_specs() {
        let cases = [
            ("src/main.c", "c", "clangd"),
            ("src/main.cpp", "cpp", "clangd"),
            ("src/main.swift", "swift", "sourcekit-lsp"),
            ("src/main.zig", "zig", "zls"),
            ("flake.nix", "nix", "nil"),
            ("taplo.toml", "toml", "taplo"),
            ("main.tf", "terraform", "terraform-ls"),
            ("schema.graphql", "graphql", "graphql-lsp"),
            ("query.sql", "sql", "sqls"),
            ("lib/app.rb", "ruby", "ruby-lsp"),
        ];

        for (path, language_id, command) in cases {
            let spec = language_server_spec_for_path(path).expect("expected spec");
            assert_eq!(spec.language_id, language_id);
            assert_eq!(spec.command_candidates.first().copied(), Some(command));
            assert!(spec.managed.is_none());
        }
    }

    #[test]
    fn parses_server_capabilities_from_initialize_result() {
        let capabilities = parse_server_capabilities(&json!({
            "capabilities": {
                "hoverProvider": true,
                "signatureHelpProvider": {
                    "triggerCharacters": ["("]
                },
                "definitionProvider": {
                    "workDoneProgress": false
                }
            }
        }));

        assert!(capabilities.hover_supported);
        assert!(capabilities.signature_help_supported);
        assert!(capabilities.definition_supported);
    }

    #[test]
    fn answers_workspace_configuration_requests_with_empty_settings() {
        let result = server_request_result(
            "workspace/configuration",
            Some(&json!({
                "items": [
                    { "section": "rust-analyzer" },
                    { "section": "gopls" }
                ]
            })),
            &[],
        );

        assert_eq!(result, json!([{}, {}]));
    }

    #[test]
    fn answers_workspace_folder_requests_with_current_folders() {
        let folders = vec![json!({
            "uri": "file:///tmp/remiss",
            "name": "remiss",
        })];

        assert_eq!(
            server_request_result("workspace/workspaceFolders", None, &folders),
            Value::Array(folders)
        );
    }

    #[test]
    fn parses_hover_markdown_from_marked_strings() {
        let hover = parse_hover_result(&json!({
            "contents": [
                {
                    "language": "rust",
                    "value": "fn value() -> i32"
                },
                "Returns the current value."
            ]
        }))
        .expect("expected hover");

        assert!(hover.markdown.contains("```rust"));
        assert!(hover.markdown.contains("Returns the current value."));
    }

    #[test]
    fn warns_when_kotlin_variable_type_matches_initializer_call() {
        let request = LspTextDocumentRequest {
            file_path: "src/main/kotlin/demo/Demo.kt".to_string(),
            document_text: std::sync::Arc::<str>::from(
                "package demo\n\nsuspend fun demo() {\n    val reittiData = coroutineScope {\n        42\n    }\n}\n",
            ),
            line: 4,
            column: 9,
        };
        let details = LspSymbolDetails {
            hover: Some(LspHoverResult {
                markdown: "````kotlin\nval reittiData: coroutineScope\n````\n".to_string(),
            }),
            ..Default::default()
        };

        let warnings = lsp_analysis_warnings("kotlin", &request, &details);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Gradle/classpath model"));
        assert!(warnings[0].contains("coroutineScope"));
    }

    #[test]
    fn warns_when_kotlin_symbol_has_no_details() {
        let request = LspTextDocumentRequest {
            file_path: "src/main/kotlin/demo/Demo.kt".to_string(),
            document_text: std::sync::Arc::<str>::from(
                "package demo\n\nsuspend fun demo() {\n    val reittiData = coroutineScope {\n        42\n    }\n}\n",
            ),
            line: 4,
            column: 22,
        };

        let warnings = lsp_analysis_warnings("kotlin", &request, &LspSymbolDetails::default());

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("coroutineScope"));
        assert!(warnings[0].contains("no hover, definition, or reference data"));
    }

    #[test]
    fn parses_signature_help_for_active_parameter() {
        let signature = parse_signature_help(&json!({
            "signatures": [
                {
                    "label": "render(name: String, count: usize)",
                    "documentation": {
                        "kind": "markdown",
                        "value": "Renders the named item."
                    },
                    "parameters": [
                        { "label": "name: String" },
                        { "label": "count: usize" }
                    ]
                }
            ],
            "activeSignature": 0,
            "activeParameter": 1
        }))
        .expect("expected signature help");

        assert_eq!(signature.label, "render(name: String, count: usize)");
        assert_eq!(signature.active_parameter.as_deref(), Some("count: usize"));
        assert_eq!(
            signature.documentation.as_deref(),
            Some("Renders the named item.")
        );
    }

    #[test]
    fn parses_definition_targets_from_location_links() {
        let repo_root = std::env::temp_dir().join("remiss-lsp-test");
        let target_path = repo_root.join("src").join("lsp.rs");
        let target_uri = file_uri(&target_path).expect("expected target URI");
        let targets = parse_definition_targets(
            &repo_root,
            &json!([
                {
                    "targetUri": target_uri.clone(),
                    "targetSelectionRange": {
                        "start": {
                            "line": 9,
                            "character": 4
                        }
                    }
                }
            ]),
        );

        assert_eq!(
            targets,
            vec![LspDefinitionTarget {
                uri: target_uri,
                path: "src/lsp.rs".to_string(),
                package_path: None,
                line: 10,
                column: 5,
            }]
        );
    }

    #[test]
    fn parses_definition_target_package_path_from_source_file() {
        let repo_root = std::env::temp_dir().join(format!(
            "remiss-lsp-package-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after unix epoch")
                .as_nanos()
        ));
        let target_path = repo_root
            .join("src")
            .join("main")
            .join("kotlin")
            .join("demo")
            .join("User.kt");
        std::fs::create_dir_all(target_path.parent().expect("target parent"))
            .expect("create target parent");
        std::fs::write(
            &target_path,
            "// Copyright\n\npackage com.example.demo\n\nval name: String = \"Ada\"\n",
        )
        .expect("write target");

        let target_uri = file_uri(&target_path).expect("expected target URI");
        let targets = parse_definition_targets(
            &repo_root,
            &json!({
                "uri": target_uri.clone(),
                "range": {
                    "start": {
                        "line": 4,
                        "character": 4
                    }
                }
            }),
        );

        std::fs::remove_dir_all(&repo_root).ok();

        assert_eq!(
            targets,
            vec![LspDefinitionTarget {
                uri: target_uri,
                path: "src/main/kotlin/demo/User.kt".to_string(),
                package_path: Some("com.example.demo".to_string()),
                line: 5,
                column: 5,
            }]
        );
        assert_eq!(targets[0].display_path(), "com.example.demo");
    }

    #[test]
    fn converts_positions_to_utf16_offsets() {
        let position = lsp_position_for_text("let 😀 = value();", 1, 6).expect("expected position");
        assert_eq!(
            position,
            json!({
                "line": 0,
                "character": 6
            })
        );
    }

    #[test]
    fn reads_and_writes_lsp_frames() {
        let value = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "ok": true
            }
        });

        let mut bytes = Vec::new();
        write_message_to(&mut bytes, &value).expect("failed to write frame");

        let mut cursor = Cursor::new(bytes);
        let decoded = read_message_from(&mut cursor)
            .expect("failed to read frame")
            .expect("expected frame");

        assert_eq!(decoded, value);
    }

    #[cfg(unix)]
    #[test]
    fn ignores_broken_rust_analyzer_candidates() {
        let dir = unique_test_directory("broken-rust-analyzer");
        let candidate = dir.join("rust-analyzer");
        write_executable_script(&candidate, "#!/bin/sh\nexit 1\n");

        assert!(!command_candidate_is_usable(
            candidate.to_str().expect("utf-8 path"),
            &candidate
        ));
        assert_eq!(
            resolve_server_command(&[candidate.to_str().expect("utf-8 path")]),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepts_working_rust_analyzer_candidates() {
        let dir = unique_test_directory("working-rust-analyzer");
        let candidate = dir.join("rust-analyzer");
        write_executable_script(&candidate, "#!/bin/sh\nexit 0\n");

        assert!(command_candidate_is_usable(
            candidate.to_str().expect("utf-8 path"),
            &candidate
        ));
        assert_eq!(
            resolve_server_command(&[candidate.to_str().expect("utf-8 path")]),
            Some(candidate.to_str().expect("utf-8 path").to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn lsp_session_shutdown_terminates_child_process_group() {
        let dir = unique_test_directory("shutdown-process-group");
        let server = dir.join("test-lsp");
        write_executable_script(
            &server,
            "#!/bin/sh\ntrap '' TERM\nwhile true; do sleep 1; done\n",
        );
        let config = ResolvedServerConfiguration {
            language_id: "test".to_string(),
            command: server.to_string_lossy().to_string(),
            args: Vec::new(),
        };
        let session = LspSession::spawn(dir.clone(), config).expect("spawn test LSP");

        assert_eq!(
            process_group_id(session.child_process_id),
            Some(session.child_process_id as libc::pid_t)
        );

        session.shutdown(std::time::Duration::from_millis(20));

        assert!(!process_is_running(session.child_process_id));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    fn process_group_id(process_id: u32) -> Option<libc::pid_t> {
        let pgid = unsafe { libc::getpgid(process_id as libc::pid_t) };
        (pgid >= 0).then_some(pgid)
    }

    #[cfg(unix)]
    fn process_is_running(process_id: u32) -> bool {
        unsafe { libc::kill(process_id as libc::pid_t, 0) == 0 }
    }
}
