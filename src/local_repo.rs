use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{app_storage, cache::CacheStore, command_runner::CommandRunner, gh};

const LOCAL_REPO_LINK_KEY_PREFIX: &str = "local-repo-link-v1:";
const CHECKOUT_LOGS_DIR: &str = "checkout-logs";
const CHECKOUT_LOG_OUTPUT_LIMIT_CHARS: usize = 2_000;

static LOCAL_REPOSITORY_PREPARE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepositoryStatus {
    pub repository: String,
    pub path: Option<String>,
    pub source: String,
    pub exists: bool,
    pub is_valid_repository: bool,
    pub current_head_oid: Option<String>,
    pub expected_head_oid: Option<String>,
    pub matches_expected_head: bool,
    pub is_worktree_clean: bool,
    pub ready_for_local_features: bool,
    pub message: String,
}

impl LocalRepositoryStatus {
    pub fn ready_for_snapshot_features(&self) -> bool {
        self.is_valid_repository && self.matches_expected_head && self.path.is_some()
    }

    pub fn should_prefer_worktree_contents(&self) -> bool {
        self.ready_for_snapshot_features() && self.is_worktree_clean
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalRepositoryLink {
    path: String,
}

#[derive(Clone, Debug)]
struct CheckoutLog {
    path: PathBuf,
}

impl CheckoutLog {
    fn for_pull_request(
        repository: &str,
        pull_request_number: i64,
        head_ref_oid: Option<&str>,
    ) -> Self {
        Self {
            path: checkout_log_path(repository, pull_request_number, head_ref_oid),
        }
    }

    fn event(&self, message: impl AsRef<str>) {
        append_checkout_log_line(&self.path, message.as_ref());
    }

    fn command_start(&self, label: &str, command: &str, cwd: Option<&Path>) {
        self.event(format!(
            "command start: {label}; cwd={}; command={command}",
            cwd.map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".to_string())
        ));
    }

    fn command_result(&self, label: &str, output: &gh::CommandOutput) {
        self.event(format!(
            "command finish: {label}; exit={:?}; timed_out={}; duration_ms={}; stdout_bytes={}; stderr_bytes={}; stdout_truncated={}; stderr_truncated={}; stdout=\"{}\"; stderr=\"{}\"",
            output.exit_code,
            output.timed_out,
            output.duration_ms,
            output.stdout_bytes.len(),
            output.stderr_bytes.len(),
            output.stdout_truncated,
            output.stderr_truncated,
            shorten_for_log(&output.stdout),
            shorten_for_log(&output.stderr),
        ));
    }
}

pub fn checkout_log_path(
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> PathBuf {
    app_storage::data_dir_root()
        .join(CHECKOUT_LOGS_DIR)
        .join(format!(
            "{}-pr-{}-{}.log",
            managed_repository_directory_name(repository),
            pull_request_number,
            managed_worktree_head_component(head_ref_oid)
        ))
}

pub fn log_checkout_event(
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
    message: impl AsRef<str>,
) -> PathBuf {
    let path = checkout_log_path(repository, pull_request_number, head_ref_oid);
    append_checkout_log_line(&path, message.as_ref());
    path
}

fn append_checkout_log_line(path: &Path, message: &str) {
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!(
                "Failed to create checkout log directory '{}': {error}",
                parent.display()
            );
            return;
        }
    }

    let line = format!("[{}] {}\n", now_ms(), sanitize_log_line(message));
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(line.as_bytes()) {
                eprintln!("Failed to write checkout log '{}': {error}", path.display());
            }
        }
        Err(error) => {
            eprintln!("Failed to open checkout log '{}': {error}", path.display());
        }
    }
}

fn sanitize_log_line(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

fn shorten_for_log(value: &str) -> String {
    let sanitized = sanitize_log_line(value);
    if sanitized.chars().count() <= CHECKOUT_LOG_OUTPUT_LIMIT_CHARS {
        return sanitized;
    }

    let mut shortened = sanitized
        .chars()
        .take(CHECKOUT_LOG_OUTPUT_LIMIT_CHARS)
        .collect::<String>();
    shortened.push_str("...[truncated]");
    shortened
}

fn format_command_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_./:=@".contains(character))
            {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn summarize_status(status: &LocalRepositoryStatus) -> String {
    format!(
        "repository={}; source={}; path={}; exists={}; valid={}; current_head={}; expected_head={}; matches_expected_head={}; clean={}; ready={}; message=\"{}\"",
        status.repository,
        status.source,
        status.path.as_deref().unwrap_or("<none>"),
        status.exists,
        status.is_valid_repository,
        status.current_head_oid.as_deref().unwrap_or("<none>"),
        status.expected_head_oid.as_deref().unwrap_or("<none>"),
        status.matches_expected_head,
        status.is_worktree_clean,
        status.ready_for_local_features,
        shorten_for_log(&status.message),
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn local_repository_prepare_lock() -> &'static Mutex<()> {
    LOCAL_REPOSITORY_PREPARE_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn load_local_repository_status(
    cache: &CacheStore,
    repository: &str,
) -> Result<LocalRepositoryStatus, String> {
    resolve_local_repository_status(cache, repository, None)
}

pub fn load_local_repository_status_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    head_ref_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    resolve_local_repository_status(cache, repository, head_ref_oid)
}

pub fn load_or_prepare_local_repository_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let log = CheckoutLog::for_pull_request(repository, pull_request_number, head_ref_oid);
    log.event(format!(
        "checkout prepare start: repository={repository}; pr={pull_request_number}; expected_head={}; log_path={}",
        head_ref_oid.unwrap_or("<none>"),
        log.path.display(),
    ));

    log.event("checkout prepare waiting for local repository lock");
    let result = {
        let _guard = local_repository_prepare_lock()
            .lock()
            .expect("local repository prepare lock poisoned");
        log.event("checkout prepare acquired local repository lock");
        load_or_prepare_local_repository_for_pull_request_logged(
            cache,
            repository,
            pull_request_number,
            head_ref_oid,
            &log,
        )
    };
    log.event("checkout prepare released local repository lock");

    match &result {
        Ok(status) => log.event(format!(
            "checkout prepare finish: {}",
            summarize_status(status)
        )),
        Err(error) => log.event(format!("checkout prepare failed: {error}")),
    }

    result
}

fn load_or_prepare_local_repository_for_pull_request_logged(
    cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
    log: &CheckoutLog,
) -> Result<LocalRepositoryStatus, String> {
    let status = resolve_local_repository_status(cache, repository, head_ref_oid)?;
    log.event(format!(
        "initial repository status: {}",
        summarize_status(&status)
    ));

    if status.source == "linked" && status.ready_for_local_features {
        log.event("using linked checkout; no managed checkout needed");
        return Ok(status);
    }

    if status.source == "linked" {
        log.event("linked checkout is not ready; falling back to app-managed checkout");
        return load_or_prepare_managed_repository_for_pull_request(
            cache,
            repository,
            pull_request_number,
            head_ref_oid,
            log,
        );
    }

    if normalized_expected_head_oid(head_ref_oid).is_some() {
        log.event("expected PR head is known; preparing app-managed per-PR worktree");
        return load_or_prepare_managed_repository_for_pull_request(
            cache,
            repository,
            pull_request_number,
            head_ref_oid,
            log,
        );
    }

    if status.source == "managed" && !status.ready_for_local_features {
        log.event("managed checkout exists but is not ready; refreshing app-managed checkout");
        let root = prepare_local_repository_for_pull_request(
            cache,
            repository,
            pull_request_number,
            head_ref_oid,
            log,
        )?;
        return inspect_repository_candidate(
            repository,
            root,
            "managed".to_string(),
            Some("Using the app-managed checkout.".to_string()),
            head_ref_oid,
        );
    }

    Ok(status)
}

pub fn ensure_local_repository_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let status = load_or_prepare_local_repository_for_pull_request(
        cache,
        repository,
        pull_request_number,
        head_ref_oid,
    )?;

    if status.ready_for_local_features {
        return Ok(status);
    }

    if status.source == "linked" {
        let log = CheckoutLog::for_pull_request(repository, pull_request_number, head_ref_oid);
        log.event("ensure fallback: linked checkout still not ready after prepare call; retrying managed checkout");
        let managed_status = load_or_prepare_managed_repository_for_pull_request(
            cache,
            repository,
            pull_request_number,
            head_ref_oid,
            &log,
        )?;

        if managed_status.ready_for_local_features {
            return Ok(managed_status);
        }

        return Err(managed_status.message.clone());
    }

    Err(status.message.clone())
}

fn resolve_local_repository_status(
    cache: &CacheStore,
    repository: &str,
    expected_head_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    if let Some(link) = cache.get::<LocalRepositoryLink>(&local_repo_link_key(repository))? {
        return inspect_repository_candidate(
            repository,
            PathBuf::from(link.value.path),
            "linked".to_string(),
            Some("Using your linked checkout.".to_string()),
            expected_head_oid,
        );
    }

    inspect_managed_repository_candidate(repository, expected_head_oid)
}

fn inspect_repository_candidate(
    repository: &str,
    candidate: PathBuf,
    source: String,
    default_message: Option<String>,
    expected_head_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let exists = candidate.exists();
    let expected_head_oid = normalized_expected_head_oid(expected_head_oid);

    if !exists {
        let message = if source == "linked" {
            "The linked checkout no longer exists. Pick another folder or switch back to the app-managed checkout.".to_string()
        } else {
            "The app will create and manage a hidden checkout when a pull request needs local code context.".to_string()
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(candidate.display().to_string()),
            source,
            exists: false,
            is_valid_repository: false,
            current_head_oid: None,
            expected_head_oid,
            matches_expected_head: false,
            is_worktree_clean: false,
            ready_for_local_features: false,
            message,
        });
    }

    let root = resolve_git_root(&candidate)?;
    let Some(root) = root else {
        let message = if source == "linked" {
            "The linked checkout is not a git repository. Pick the repository root or any folder inside it.".to_string()
        } else {
            "The app-managed checkout is missing its git metadata. Remove it from app storage and try again.".to_string()
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(candidate.display().to_string()),
            source,
            exists: true,
            is_valid_repository: false,
            current_head_oid: None,
            expected_head_oid,
            matches_expected_head: false,
            is_worktree_clean: false,
            ready_for_local_features: false,
            message,
        });
    };

    let current_head_oid = current_head_oid(&root)?;
    if source == "managed" && current_head_oid.is_none() {
        let message = "The app-managed checkout is incomplete. The app will remove it and recreate it before local code features run.".to_string();

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(root.display().to_string()),
            source,
            exists: true,
            is_valid_repository: false,
            current_head_oid: None,
            expected_head_oid,
            matches_expected_head: false,
            is_worktree_clean: false,
            ready_for_local_features: false,
            message,
        });
    }

    if !repository_matches_git_remote(repository, &root)? {
        let message = if source == "linked" {
            format!(
                "The linked checkout does not match {}. Use a clone whose remotes point at that repository.",
                repository
            )
        } else {
            format!(
                "The app-managed checkout does not match {}. Remove it from app storage and try again.",
                repository
            )
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(root.display().to_string()),
            source,
            exists: true,
            is_valid_repository: false,
            current_head_oid,
            expected_head_oid,
            matches_expected_head: false,
            is_worktree_clean: false,
            ready_for_local_features: false,
            message,
        });
    }

    let matches_expected_head = expected_head_oid
        .as_ref()
        .map(|expected| current_head_oid.as_deref() == Some(expected.as_str()))
        .unwrap_or(true);
    let is_worktree_clean = worktree_is_clean(&root)?;
    let ready_for_local_features = matches_expected_head && is_worktree_clean;

    let message = if let Some(expected_head) = expected_head_oid.as_deref() {
        if !matches_expected_head {
            if source == "linked" {
                format!(
                    "The linked checkout is on {}, but this pull request expects {}. Check out the PR head commit or switch back to the app-managed checkout.",
                    current_head_oid.as_deref().unwrap_or("unknown"),
                    expected_head
                )
            } else {
                format!(
                    "The app-managed checkout is out of date. The app will refresh it to pull request head {} before local code features run.",
                    expected_head
                )
            }
        } else if !is_worktree_clean {
            if source == "linked" {
                "The linked checkout has local changes. Commit, stash, or discard them before using local code features, or switch back to the app-managed checkout.".to_string()
            } else {
                "The app-managed checkout has local changes. Remove it from app storage and let the app recreate it for local code features.".to_string()
            }
        } else {
            default_message.unwrap_or_else(|| {
                format!(
                    "Using your checkout at pull request head {}.",
                    expected_head
                )
            })
        }
    } else if !is_worktree_clean {
        if source == "linked" {
            "The linked checkout has local changes. Commit, stash, or discard them before using local code features.".to_string()
        } else {
            "The app-managed checkout has local changes. Remove it from app storage and let the app recreate it.".to_string()
        }
    } else {
        default_message.unwrap_or_else(|| "Using your checkout.".to_string())
    };

    Ok(LocalRepositoryStatus {
        repository: repository.to_string(),
        path: Some(root.display().to_string()),
        source,
        exists: true,
        is_valid_repository: true,
        current_head_oid,
        expected_head_oid,
        matches_expected_head,
        is_worktree_clean,
        ready_for_local_features,
        message,
    })
}

fn prepare_local_repository_for_pull_request(
    _cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
    log: &CheckoutLog,
) -> Result<PathBuf, String> {
    ensure_managed_repository_for_pull_request(repository, pull_request_number, head_ref_oid, log)
}

fn load_or_prepare_managed_repository_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
    log: &CheckoutLog,
) -> Result<LocalRepositoryStatus, String> {
    let status = inspect_managed_worktree_candidate(repository, pull_request_number, head_ref_oid)?;
    log.event(format!(
        "managed worktree status before prepare: {}",
        summarize_status(&status)
    ));
    if status.ready_for_local_features {
        log.event("managed worktree is already ready");
        return Ok(status);
    }

    let root = prepare_local_repository_for_pull_request(
        cache,
        repository,
        pull_request_number,
        head_ref_oid,
        log,
    )?;
    inspect_repository_candidate(
        repository,
        root,
        "managed".to_string(),
        Some("Using the app-managed checkout.".to_string()),
        head_ref_oid,
    )
}

fn inspect_managed_repository_candidate(
    repository: &str,
    expected_head_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let managed_path = managed_repository_path(repository)?;

    inspect_repository_candidate(
        repository,
        managed_path,
        "managed".to_string(),
        Some("Using the app-managed checkout.".to_string()),
        expected_head_oid,
    )
}

fn inspect_managed_worktree_candidate(
    repository: &str,
    pull_request_number: i64,
    expected_head_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let managed_path =
        managed_repository_worktree_path(repository, pull_request_number, expected_head_oid)?;

    inspect_repository_candidate(
        repository,
        managed_path,
        "managed".to_string(),
        Some("Using the app-managed checkout.".to_string()),
        expected_head_oid,
    )
}

fn ensure_managed_repository(repository: &str, log: &CheckoutLog) -> Result<PathBuf, String> {
    let target = managed_repository_path(repository)?;
    log.event(format!(
        "ensuring base managed repository at {}",
        target.display()
    ));

    if target.exists() {
        log.event("base managed repository path exists; inspecting git root and remote");
        let Some(root) = resolve_git_root(&target)? else {
            log.event(format!(
                "base managed repository at {} has no git metadata; removing incomplete clone before retry",
                target.display()
            ));
            remove_incomplete_managed_repository(&target, log)?;
            return ensure_managed_repository(repository, log);
        };

        if current_head_oid(&root)?.is_some() {
            if !repository_matches_git_remote(repository, &root)? {
                return Err(format!(
                    "The app-managed checkout does not match {}. Remove it from app storage and try again.",
                    repository
                ));
            }

            configure_managed_repository_git_credentials(&root, log);
            log.event(format!(
                "base managed repository is valid at {}",
                root.display()
            ));
            return Ok(root);
        }

        log.event(format!(
            "base managed repository at {} has no HEAD; removing incomplete clone before retry",
            root.display()
        ));
        remove_incomplete_managed_repository(&target, log)?;
    }

    let Some(parent) = target.parent() else {
        return Err(
            "Failed to resolve the app-managed checkout folder inside app storage.".to_string(),
        );
    };

    fs::create_dir_all(parent).map_err(|error| {
        format!("Failed to create the app-managed checkout folder in app storage: {error}")
    })?;
    log.event(format!(
        "created base managed repository parent {}",
        parent.display()
    ));

    let output = run_gh_logged(
        log,
        vec![
            "repo".to_string(),
            "clone".to_string(),
            repository.to_string(),
            target.display().to_string(),
            "--".to_string(),
            "--filter=blob:none".to_string(),
            "--no-checkout".to_string(),
        ],
        None,
        "clone base managed repository",
    )?;

    if output.exit_code != Some(0) {
        let cleanup_error = remove_incomplete_managed_repository(&target, log).err();
        let mut error = combine_process_error(
            output,
            &format!("Failed to create the app-managed checkout for {repository}"),
        );
        if let Some(cleanup_error) = cleanup_error {
            error.push_str(&format!(
                " Also failed to clean up the incomplete clone: {cleanup_error}"
            ));
        }
        return Err(error);
    }

    let root = resolve_git_root(&target)?.ok_or_else(|| {
        "The app-managed checkout was created but is not a git repository.".to_string()
    })?;
    configure_managed_repository_git_credentials(&root, log);
    log.event(format!(
        "base managed repository clone resolved git root {}",
        root.display()
    ));
    if current_head_oid(&root)?.is_none() {
        remove_incomplete_managed_repository(&target, log)?;
        return Err(
            "The app-managed checkout was created but did not resolve a HEAD commit.".to_string(),
        );
    }

    if repository_matches_git_remote(repository, &root)? {
        Ok(root)
    } else {
        Err(format!(
            "The app-managed checkout does not match {}.",
            repository
        ))
    }
}

fn remove_incomplete_managed_repository(target: &Path, log: &CheckoutLog) -> Result<(), String> {
    if !target.exists() {
        return Ok(());
    }

    log.event(format!(
        "removing incomplete base managed repository at {}",
        target.display()
    ));
    fs::remove_dir_all(target).map_err(|error| {
        format!(
            "Failed to remove incomplete app-managed checkout '{}': {error}",
            target.display()
        )
    })?;
    log.event(format!(
        "removed incomplete base managed repository at {}",
        target.display()
    ));
    Ok(())
}

fn ensure_managed_repository_for_pull_request(
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
    log: &CheckoutLog,
) -> Result<PathBuf, String> {
    let root = ensure_managed_repository(repository, log)?;
    log.event(format!(
        "base managed repository ready at {}",
        root.display()
    ));
    if normalized_expected_head_oid(head_ref_oid).is_some() {
        let status =
            inspect_managed_worktree_candidate(repository, pull_request_number, head_ref_oid)?;
        log.event(format!(
            "existing per-PR worktree status: {}",
            summarize_status(&status)
        ));
        if status.ready_for_local_features {
            if let Some(path) = status.path {
                log.event(format!("using existing per-PR worktree at {path}"));
                return Ok(PathBuf::from(path));
            }
        }
    }

    sync_managed_repository_to_pull_request(
        &root,
        repository,
        pull_request_number,
        head_ref_oid,
        log,
    )?;
    ensure_managed_worktree_for_pull_request(
        &root,
        repository,
        pull_request_number,
        head_ref_oid,
        log,
    )
}

fn ensure_managed_worktree_for_pull_request(
    base_root: &Path,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
    log: &CheckoutLog,
) -> Result<PathBuf, String> {
    let expected_head = normalized_expected_head_oid(head_ref_oid);
    let worktree_path = managed_repository_worktree_path(
        repository,
        pull_request_number,
        expected_head.as_deref(),
    )?;
    log.event(format!(
        "ensuring per-PR managed worktree at {}; expected_head={}",
        worktree_path.display(),
        expected_head.as_deref().unwrap_or("<none>")
    ));
    remove_stale_managed_worktrees_for_pull_request(
        base_root,
        repository,
        pull_request_number,
        &worktree_path,
        log,
    )?;

    if worktree_path.exists() {
        log.event("per-PR worktree path exists; inspecting before reuse/removal");
        let status = inspect_repository_candidate(
            repository,
            worktree_path.clone(),
            "managed".to_string(),
            Some("Using the app-managed checkout.".to_string()),
            expected_head.as_deref(),
        )?;
        log.event(format!(
            "per-PR worktree status before reuse/removal: {}",
            summarize_status(&status)
        ));
        if expected_head.is_some() && status.ready_for_local_features {
            return Ok(PathBuf::from(
                status
                    .path
                    .unwrap_or_else(|| worktree_path.display().to_string()),
            ));
        }

        remove_managed_worktree(base_root, &worktree_path, log)?;
    }

    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("Failed to create the app-managed worktree folder in app storage: {error}")
        })?;
        log.event(format!(
            "ensured per-PR worktree parent {}",
            parent.display()
        ));
    }

    let target_ref = expected_head
        .clone()
        .or_else(|| current_head_oid(base_root).ok().flatten())
        .ok_or_else(|| {
            "The app-managed checkout could not resolve the pull request head commit.".to_string()
        })?;
    log.event(format!(
        "creating per-PR worktree from target_ref={target_ref}"
    ));

    let output = run_git_logged(
        log,
        "git worktree add",
        base_root,
        vec![
            "worktree".to_string(),
            "add".to_string(),
            "--force".to_string(),
            "--detach".to_string(),
            worktree_path.display().to_string(),
            target_ref.clone(),
        ],
    )?;

    if output.exit_code != Some(0) {
        return Err(combine_process_error(
            output,
            &format!(
                "Failed to create the app-managed worktree for pull request #{pull_request_number} in {repository}"
            ),
        ));
    }

    let status = inspect_repository_candidate(
        repository,
        worktree_path.clone(),
        "managed".to_string(),
        Some("Using the app-managed checkout.".to_string()),
        expected_head.as_deref(),
    )?;
    log.event(format!(
        "per-PR worktree status after creation: {}",
        summarize_status(&status)
    ));
    if status.ready_for_local_features {
        Ok(PathBuf::from(
            status
                .path
                .unwrap_or_else(|| worktree_path.display().to_string()),
        ))
    } else {
        Err(status.message)
    }
}

fn remove_stale_managed_worktrees_for_pull_request(
    base_root: &Path,
    repository: &str,
    pull_request_number: i64,
    keep_path: &Path,
    log: &CheckoutLog,
) -> Result<(), String> {
    let worktrees_root = managed_repository_worktrees_root(repository)?;
    if !worktrees_root.exists() {
        return Ok(());
    }

    let stale_prefix = format!("pr-{pull_request_number}-");
    let entries = fs::read_dir(&worktrees_root).map_err(|error| {
        format!(
            "Failed to inspect app-managed worktree folder '{}': {error}",
            worktrees_root.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to inspect app-managed worktree folder '{}': {error}",
                worktrees_root.display()
            )
        })?;
        let path = entry.path();
        if path == keep_path || !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(&stale_prefix) {
            continue;
        }

        log.event(format!(
            "removing stale per-PR managed worktree sibling {}; keeping {}",
            path.display(),
            keep_path.display()
        ));
        remove_managed_worktree(base_root, &path, log)?;
    }

    Ok(())
}

fn remove_managed_worktree(
    base_root: &Path,
    worktree_path: &Path,
    log: &CheckoutLog,
) -> Result<(), String> {
    log.event(format!(
        "removing stale per-PR worktree at {}",
        worktree_path.display()
    ));
    let _ = run_git_logged(
        log,
        "git worktree remove",
        base_root,
        vec![
            "worktree".to_string(),
            "remove".to_string(),
            "--force".to_string(),
            worktree_path.display().to_string(),
        ],
    );

    if worktree_path.exists() {
        fs::remove_dir_all(worktree_path).map_err(|error| {
            format!(
                "Failed to remove stale app-managed worktree '{}': {error}",
                worktree_path.display()
            )
        })?;
        log.event(format!(
            "removed stale per-PR worktree directory {}",
            worktree_path.display()
        ));
    }

    let _ = run_git_logged(log, "git worktree prune", base_root, ["worktree", "prune"]);
    Ok(())
}

fn sync_managed_repository_to_pull_request(
    root: &Path,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
    log: &CheckoutLog,
) -> Result<(), String> {
    let expected_head = head_ref_oid
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let current_head_before = current_head_oid(root)?;
    log.event(format!(
        "sync base managed repository to PR: root={}; current_head={}; expected_head={}; fetch_ref=refs/pull/{pull_request_number}/head",
        root.display(),
        current_head_before.as_deref().unwrap_or("<none>"),
        expected_head.as_deref().unwrap_or("<none>")
    ));

    if expected_head
        .as_deref()
        .is_some_and(|head| git_commit_exists(root, head).unwrap_or(false))
    {
        log.event("base managed repository already has expected PR head object");
        return Ok(());
    }

    let pr_ref = format!("refs/remotes/remiss/pr/{pull_request_number}");
    let output = run_git_logged(
        log,
        "fetch PR head in base managed repository",
        root,
        vec![
            "fetch".to_string(),
            "--force".to_string(),
            "--no-tags".to_string(),
            "--depth=1".to_string(),
            "--filter=blob:none".to_string(),
            "origin".to_string(),
            format!("+refs/pull/{pull_request_number}/head:{pr_ref}"),
        ],
    )?;

    if output.exit_code != Some(0) {
        return Err(combine_process_error(
            output,
            &format!(
                "Failed to update the app-managed checkout to pull request #{pull_request_number} for {repository}"
            ),
        ));
    }

    if let Some(expected_head) = expected_head {
        let has_expected_head = git_commit_exists(root, &expected_head)?;
        log.event(format!(
            "base managed repository expected PR head object after fetch: expected_head={expected_head}; exists={has_expected_head}"
        ));
        if !has_expected_head {
            return Err(format!(
                "The app-managed checkout did not fetch pull request #{pull_request_number}. Expected commit {expected_head} was not present after fetching refs/pull/{pull_request_number}/head.",
            ));
        }
    }

    Ok(())
}

fn git_commit_exists(path: &Path, oid: &str) -> Result<bool, String> {
    let output = run_git(path, ["cat-file", "-e", &format!("{oid}^{{commit}}")])?;
    Ok(output.exit_code == Some(0))
}

fn configure_managed_repository_git_credentials(root: &Path, log: &CheckoutLog) {
    configure_managed_repository_git_config(
        root,
        log,
        "configure managed git credential helper",
        [
            "config",
            "--local",
            "--replace-all",
            "credential.https://github.com.helper",
            "!gh auth git-credential",
        ],
    );
    configure_managed_repository_git_config(
        root,
        log,
        "configure managed git credential path scope",
        [
            "config",
            "--local",
            "--replace-all",
            "credential.https://github.com.useHttpPath",
            "true",
        ],
    );
}

fn configure_managed_repository_git_config<const N: usize>(
    root: &Path,
    log: &CheckoutLog,
    label: &str,
    args: [&str; N],
) {
    match run_git_logged(log, label, root, args) {
        Ok(output) if output.exit_code == Some(0) => {
            log.event(format!("{label}: ok"));
        }
        Ok(output) => {
            log.event(format!(
                "{label}: skipped; exit={:?}; stderr=\"{}\"; stdout=\"{}\"",
                output.exit_code,
                shorten_for_log(&output.stderr),
                shorten_for_log(&output.stdout),
            ));
        }
        Err(error) => {
            log.event(format!("{label}: skipped; error={error}"));
        }
    }
}

fn run_gh_logged(
    log: &CheckoutLog,
    args: Vec<String>,
    working_directory: Option<&Path>,
    label: &str,
) -> Result<gh::CommandOutput, String> {
    log.command_start(
        label,
        &format!("gh {}", format_command_args(&args)),
        working_directory,
    );

    let mut runner = CommandRunner::new("gh")
        .args(args)
        .timeout(Duration::from_secs(120));
    if let Some(path) = working_directory {
        runner = runner.current_dir(path);
    }

    let output = runner.run();
    match output {
        Ok(output) => {
            log.command_result(label, &output);
            if output.timed_out {
                Err("gh command timed out after 120 seconds.".to_string())
            } else {
                Ok(output)
            }
        }
        Err(error) => {
            log.event(format!(
                "command launch/poll failed: {label}; error={error}"
            ));
            Err(error)
        }
    }
}

fn run_git(
    path: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<gh::CommandOutput, String> {
    let mut command_args = vec!["-C".to_string(), path.display().to_string()];
    command_args.extend(args.into_iter().map(Into::into));
    let output = CommandRunner::new("git")
        .args(command_args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .run()?;
    if output.timed_out {
        return Err("git command timed out after 120 seconds.".to_string());
    }
    Ok(output)
}

fn run_git_logged(
    log: &CheckoutLog,
    label: &str,
    path: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<gh::CommandOutput, String> {
    let mut command_args = vec!["-C".to_string(), path.display().to_string()];
    command_args.extend(args.into_iter().map(Into::into));
    log.command_start(
        label,
        &format!("git {}", format_command_args(&command_args)),
        None,
    );

    let output = CommandRunner::new("git")
        .args(command_args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .run();
    match output {
        Ok(output) => {
            log.command_result(label, &output);
            if output.timed_out {
                Err("git command timed out after 120 seconds.".to_string())
            } else {
                Ok(output)
            }
        }
        Err(error) => {
            log.event(format!(
                "command launch/poll failed: {label}; error={error}"
            ));
            Err(error)
        }
    }
}

fn resolve_git_root(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let output = run_git(path, ["rev-parse", "--show-toplevel"])?;

    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let root = output.stdout.trim().to_string();
    if root.is_empty() {
        return Ok(None);
    }

    Ok(Some(PathBuf::from(root)))
}

fn current_head_oid(path: &Path) -> Result<Option<String>, String> {
    let output = run_git(path, ["rev-parse", "HEAD"])?;

    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let head = output.stdout.trim().to_string();
    if head.is_empty() {
        return Ok(None);
    }

    Ok(Some(head))
}

fn worktree_is_clean(path: &Path) -> Result<bool, String> {
    let output = run_git(path, ["status", "--porcelain", "--untracked-files=normal"])?;

    if output.exit_code != Some(0) {
        return Ok(false);
    }

    Ok(output.stdout.trim().is_empty())
}

fn repository_matches_git_remote(repository: &str, path: &Path) -> Result<bool, String> {
    let output = run_git(path, ["remote"])?;

    if output.exit_code != Some(0) {
        return Ok(false);
    }

    let target = repository.to_ascii_lowercase();
    let remote_names = output.stdout;

    for remote_name in remote_names.lines().filter(|line| !line.trim().is_empty()) {
        let remote_output = run_git(path, ["remote", "get-url", remote_name])?;

        if remote_output.exit_code != Some(0) {
            continue;
        }

        if normalized_remote_repository(&remote_output.stdout)
            .is_some_and(|normalized| normalized == target)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn managed_repository_path(repository: &str) -> Result<PathBuf, String> {
    Ok(
        app_storage::managed_repositories_root()
            .join(managed_repository_directory_name(repository)),
    )
}

fn managed_repository_worktree_path(
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<PathBuf, String> {
    let head = managed_worktree_head_component(head_ref_oid);
    Ok(managed_repository_worktrees_root(repository)?
        .join(format!("pr-{pull_request_number}-{head}")))
}

fn managed_repository_worktrees_root(repository: &str) -> Result<PathBuf, String> {
    let repository_dir = managed_repository_directory_name(repository);
    Ok(app_storage::managed_repositories_root().join(format!("{repository_dir}__worktrees")))
}

fn managed_repository_directory_name(repository: &str) -> String {
    let mut result = String::new();

    for character in repository.chars() {
        match character {
            'a'..='z' | '0'..='9' | '-' | '_' | '.' => result.push(character),
            'A'..='Z' => result.push(character.to_ascii_lowercase()),
            '/' | '\\' => result.push_str("__"),
            _ => result.push('-'),
        }
    }

    if result.is_empty() {
        "repository".to_string()
    } else {
        result
    }
}

fn managed_worktree_head_component(head_ref_oid: Option<&str>) -> String {
    let mut result = String::new();
    for character in head_ref_oid
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .chars()
        .take(16)
    {
        match character {
            'a'..='z' | '0'..='9' => result.push(character),
            'A'..='Z' => result.push(character.to_ascii_lowercase()),
            _ => result.push('-'),
        }
    }

    if result.is_empty() {
        "unknown".to_string()
    } else {
        result
    }
}

fn local_repo_link_key(repository: &str) -> String {
    format!("{LOCAL_REPO_LINK_KEY_PREFIX}{repository}")
}

fn normalized_expected_head_oid(head_ref_oid: Option<&str>) -> Option<String> {
    head_ref_oid
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn combine_process_error(output: gh::CommandOutput, prefix: &str) -> String {
    let detail = if !output.stderr.is_empty() {
        output.stderr
    } else if !output.stdout.is_empty() {
        output.stdout
    } else {
        String::new()
    };

    if git_output_requested_credentials(&detail) {
        let auth_message = "Git asked for GitHub credentials while preparing the app-managed checkout. Remiss tried to use your `gh` session, but background checkout updates cannot answer an interactive username/password prompt. Run `gh auth setup-git` once, or configure a Git credential helper or PAT for GitHub, then retry the checkout.";
        if detail.is_empty() {
            format!("{prefix}: {auth_message}")
        } else {
            format!("{prefix}: {auth_message} Original git output: {detail}")
        }
    } else if detail.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {detail}")
    }
}

fn git_output_requested_credentials(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("username for 'https://")
        || lower.contains("password for 'https://")
        || lower.contains("terminal prompts disabled")
        || lower.contains("authentication failed")
}

fn normalized_remote_repository(remote_url: &str) -> Option<String> {
    let trimmed = remote_url.trim().trim_end_matches(".git");

    let repository_path = if let Some((_, remainder)) = trimmed.split_once("://") {
        let (_, path) = remainder.split_once('/')?;
        path
    } else if let Some((_, path)) = trimmed.split_once(':') {
        path
    } else {
        return None;
    };

    let mut segments = repository_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty());

    let owner = segments.next()?;
    let name = segments.next()?;

    Some(format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        name.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::cache::CacheStore;

    use super::{
        combine_process_error, ensure_local_repository_for_pull_request,
        load_local_repository_status_for_pull_request, local_repo_link_key,
        managed_repository_directory_name, managed_repository_path,
        managed_repository_worktree_path, normalized_remote_repository, LocalRepositoryLink,
    };

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    struct GitTestRepository {
        root: PathBuf,
        _workspace: PathBuf,
    }

    impl GitTestRepository {
        fn new(remote_repository: &str) -> Self {
            let workspace = unique_test_directory("local-repo");
            let root = workspace.join("repo");
            fs::create_dir_all(&root).expect("failed to create repo directory");
            run_git(&root, ["init"]);
            run_git(&root, ["config", "user.name", "Remiss Tests"]);
            run_git(&root, ["config", "user.email", "remiss-tests@example.com"]);
            run_git(
                &root,
                [
                    "remote",
                    "add",
                    "origin",
                    &format!("git@github.com:{remote_repository}.git"),
                ],
            );
            Self {
                root,
                _workspace: workspace,
            }
        }

        fn write_file(&self, path: &str, contents: &str) {
            let full_path = self.root.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).expect("failed to create parent directory");
            }
            fs::write(full_path, contents).expect("failed to write test file");
        }

        fn commit_all(&self, message: &str) -> String {
            run_git(&self.root, ["add", "."]);
            run_git(&self.root, ["commit", "-m", message]);
            self.head_oid()
        }

        fn head_oid(&self) -> String {
            git_output(&self.root, ["rev-parse", "HEAD"])
        }
    }

    fn unique_test_directory(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "remiss-{prefix}-{nanos}-{test_id}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("failed to create temp directory");
        path
    }

    fn run_git<const N: usize>(path: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("failed to run git");

        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn git_output<const N: usize>(path: &Path, args: [&str; N]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("failed to run git");

        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn create_managed_clone(repository: &str, source: &Path) -> PathBuf {
        let managed_path = managed_repository_path(repository).expect("failed to resolve path");
        if managed_path.exists() {
            fs::remove_dir_all(&managed_path).expect("failed to remove existing managed repo");
        }
        if let Some(parent) = managed_path.parent() {
            fs::create_dir_all(parent).expect("failed to create managed repo parent");
        }

        let output = Command::new("git")
            .arg("clone")
            .arg(source)
            .arg(&managed_path)
            .output()
            .expect("failed to clone managed repo");

        if !output.status.success() {
            panic!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let remote_url = format!("git@github.com:{repository}.git");
        run_git(
            &managed_path,
            ["remote", "set-url", "origin", remote_url.as_str()],
        );
        managed_path
    }

    #[test]
    fn normalizes_https_remote_urls() {
        assert_eq!(
            normalized_remote_repository("https://github.com/openai/example.git"),
            Some("openai/example".to_string())
        );
    }

    #[test]
    fn normalizes_ssh_remote_urls() {
        assert_eq!(
            normalized_remote_repository("git@github.com:OpenAI/Example.git"),
            Some("openai/example".to_string())
        );
    }

    #[test]
    fn normalizes_enterprise_remote_urls() {
        assert_eq!(
            normalized_remote_repository("ssh://git@github.example.com/acme/widgets.git"),
            Some("acme/widgets".to_string())
        );
    }

    #[test]
    fn rejects_non_repository_urls() {
        assert_eq!(normalized_remote_repository("not-a-remote"), None);
    }

    #[test]
    fn checkout_process_error_explains_git_credential_prompts() {
        let message = combine_process_error(
            crate::gh::CommandOutput {
                exit_code: Some(128),
                stdout: String::new(),
                stderr: "fatal: could not read Username for 'https://github.com': terminal prompts disabled".to_string(),
                stdout_bytes: Vec::new(),
                stderr_bytes: Vec::new(),
                timed_out: false,
                duration_ms: 12,
                stdout_truncated: false,
                stderr_truncated: false,
            },
            "Failed to update the app-managed checkout",
        );

        assert!(message.contains("Git asked for GitHub credentials"));
        assert!(message.contains("gh auth setup-git"));
        assert!(message.contains("could not read Username"));
    }

    #[test]
    fn sanitizes_managed_repository_directory_names() {
        assert_eq!(
            managed_repository_directory_name("OpenAI/example.repo"),
            "openai__example.repo".to_string()
        );
    }

    #[test]
    fn linked_repository_status_requires_expected_head() {
        let repository = GitTestRepository::new("openai/example");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let initial_head = repository.commit_all("initial");

        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let current_head = repository.commit_all("second");

        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key("openai/example"),
                &LocalRepositoryLink {
                    path: repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status = load_local_repository_status_for_pull_request(
            &cache,
            "openai/example",
            Some(&initial_head),
        )
        .expect("failed to load status");

        assert!(status.is_valid_repository);
        assert_eq!(
            status.current_head_oid.as_deref(),
            Some(current_head.as_str())
        );
        assert_eq!(
            status.expected_head_oid.as_deref(),
            Some(initial_head.as_str())
        );
        assert!(!status.matches_expected_head);
        assert!(!status.ready_for_local_features);
        assert!(status.message.contains("expects"));
        assert!(!status
            .message
            .contains(&repository.root.display().to_string()));
    }

    #[test]
    fn linked_repository_status_requires_clean_worktree() {
        let repository = GitTestRepository::new("openai/example");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = repository.commit_all("initial");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 3 }\n");

        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key("openai/example"),
                &LocalRepositoryLink {
                    path: repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status =
            load_local_repository_status_for_pull_request(&cache, "openai/example", Some(&head))
                .expect("failed to load status");

        assert!(status.matches_expected_head);
        assert!(!status.is_worktree_clean);
        assert!(!status.ready_for_local_features);
        assert!(status.ready_for_snapshot_features());
        assert!(!status.should_prefer_worktree_contents());
        assert!(status.message.contains("local changes"));
        assert!(!status
            .message
            .contains(&repository.root.display().to_string()));
    }

    #[test]
    fn ensure_local_repository_uses_clean_linked_checkout_at_expected_head() {
        let repository = GitTestRepository::new("openai/example");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = repository.commit_all("initial");

        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key("openai/example"),
                &LocalRepositoryLink {
                    path: repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status =
            ensure_local_repository_for_pull_request(&cache, "openai/example", 42, Some(&head))
                .expect("failed to ensure repository");

        assert_eq!(status.source, "linked");
        assert!(status.ready_for_local_features);
        assert_eq!(
            PathBuf::from(status.path.expect("status path"))
                .canonicalize()
                .expect("canonical status path"),
            repository
                .root
                .canonicalize()
                .expect("canonical repository path")
        );
    }

    #[test]
    fn managed_worktree_paths_vary_by_pull_request_head() {
        let first =
            managed_repository_worktree_path("openai/example", 42, Some("aaaaaaaaaaaaaaaaaaaa"))
                .expect("first path");
        let second =
            managed_repository_worktree_path("openai/example", 42, Some("bbbbbbbbbbbbbbbbbbbb"))
                .expect("second path");

        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains("pr-42-aaaaaaaaaaaaaaaa"));
        assert!(second.to_string_lossy().contains("pr-42-bbbbbbbbbbbbbbbb"));
    }

    #[test]
    fn ensure_local_repository_removes_stale_worktree_for_updated_pr_head() {
        let repository_name = format!(
            "openai/example-stale-worktree-{}",
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        );
        let linked_repository = GitTestRepository::new(&repository_name);
        linked_repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let first_head = linked_repository.commit_all("initial");
        linked_repository.write_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let second_head = linked_repository.commit_all("second");

        let managed_path = create_managed_clone(&repository_name, &linked_repository.root);
        linked_repository.write_file("src/lib.rs", "pub fn value() -> i32 { 3 }\n");

        let stale_path = managed_repository_worktree_path(&repository_name, 42, Some(&first_head))
            .expect("stale worktree path");
        fs::create_dir_all(&stale_path).expect("failed to create stale worktree");
        fs::write(stale_path.join("stale.txt"), "stale").expect("failed to write stale marker");

        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key(&repository_name),
                &LocalRepositoryLink {
                    path: linked_repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status = ensure_local_repository_for_pull_request(
            &cache,
            &repository_name,
            42,
            Some(&second_head),
        )
        .expect("failed to ensure repository");

        let expected_path =
            managed_repository_worktree_path(&repository_name, 42, Some(&second_head))
                .expect("expected worktree path");
        assert_eq!(status.source, "managed");
        assert_eq!(
            status.current_head_oid.as_deref(),
            Some(second_head.as_str())
        );
        assert_eq!(
            status.path.as_deref(),
            Some(expected_path.to_string_lossy().as_ref())
        );
        assert!(!stale_path.exists());
        assert!(expected_path.exists());

        if let Some(parent) = expected_path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
        let _ = fs::remove_dir_all(managed_path);
    }

    #[test]
    fn ensure_local_repository_falls_back_to_managed_checkout_when_linked_repo_is_dirty() {
        let repository_name = format!(
            "openai/example-fallback-{}",
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        );
        let linked_repository = GitTestRepository::new(&repository_name);
        linked_repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = linked_repository.commit_all("initial");
        linked_repository.write_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");

        let managed_path = create_managed_clone(&repository_name, &linked_repository.root);
        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key(&repository_name),
                &LocalRepositoryLink {
                    path: linked_repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status =
            ensure_local_repository_for_pull_request(&cache, &repository_name, 42, Some(&head))
                .expect("failed to ensure repository");

        assert_eq!(status.source, "managed");
        assert!(status.ready_for_local_features);
        assert!(status.is_worktree_clean);
        assert_eq!(status.current_head_oid.as_deref(), Some(head.as_str()));
        assert_eq!(
            status.path.as_deref(),
            Some(
                managed_repository_worktree_path(&repository_name, 42, Some(&head))
                    .expect("worktree path")
                    .to_string_lossy()
                    .as_ref()
            )
        );

        let _ = fs::remove_dir_all(
            managed_repository_worktree_path(&repository_name, 42, Some(&head))
                .expect("worktree path"),
        );
        let _ = fs::remove_dir_all(managed_path);
    }
}
