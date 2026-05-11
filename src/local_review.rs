use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::{
    app_storage,
    cache::CacheStore,
    command_runner::{CommandOutput, CommandRunner},
    diff::{parse_unified_diff, DiffLineKind, ParsedDiffFile},
    github::{
        AuthState, PullRequestDataCompleteness, PullRequestDetail, PullRequestDetailSnapshot,
        PullRequestFile, PullRequestSummary,
    },
    local_repo::LocalRepositoryStatus,
};

const REMEMBERED_REPOSITORIES_KEY: &str = "local-review-repositories-v1";
const LOCAL_REVIEW_GIT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalReviewStatusKind {
    #[default]
    Unknown,
    Inspecting,
    Ready,
    NoDiff,
    Blocked,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RememberedLocalRepository {
    pub repository: String,
    pub path: String,
    #[serde(default)]
    pub last_branch: Option<String>,
    #[serde(default)]
    pub last_status: LocalReviewStatusKind,
    #[serde(default)]
    pub last_message: Option<String>,
    #[serde(default)]
    pub last_base_oid: Option<String>,
    #[serde(default)]
    pub last_head_oid: Option<String>,
    #[serde(default)]
    pub last_inspected_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RememberedLocalRepositoriesDocument {
    repositories: Vec<RememberedLocalRepository>,
}

#[derive(Clone, Debug)]
pub struct LocalReviewInspection {
    pub repository: String,
    pub root: PathBuf,
    pub branch: String,
    pub base_ref_name: String,
    pub base_oid: String,
    pub head_oid: String,
    pub commits_count: i64,
    pub status: LocalReviewStatusKind,
    pub message: String,
    pub local_repository_status: LocalRepositoryStatus,
    pub detail: PullRequestDetail,
    pub summary: PullRequestSummary,
    pub key: String,
}

pub fn is_local_review_key(key: &str) -> bool {
    key.starts_with("local:")
}

pub fn is_local_review_detail(detail: &PullRequestDetail) -> bool {
    is_local_review_key(&detail.id)
}

pub fn load_remembered_repositories(
    cache: &CacheStore,
) -> Result<Vec<RememberedLocalRepository>, String> {
    Ok(cache
        .get::<RememberedLocalRepositoriesDocument>(REMEMBERED_REPOSITORIES_KEY)?
        .map(|document| document.value.repositories)
        .unwrap_or_default())
}

pub fn save_remembered_repositories(
    cache: &CacheStore,
    repositories: &[RememberedLocalRepository],
) -> Result<(), String> {
    cache.put(
        REMEMBERED_REPOSITORIES_KEY,
        &RememberedLocalRepositoriesDocument {
            repositories: repositories.to_vec(),
        },
        now_ms(),
    )
}

pub fn mark_repository_inspecting(repository: &mut RememberedLocalRepository) {
    repository.last_status = LocalReviewStatusKind::Inspecting;
    repository.last_message = Some("Inspecting working checkout...".to_string());
}

pub fn remembered_from_inspection(inspection: &LocalReviewInspection) -> RememberedLocalRepository {
    RememberedLocalRepository {
        repository: inspection.repository.clone(),
        path: inspection.root.display().to_string(),
        last_branch: Some(inspection.branch.clone()),
        last_status: inspection.status,
        last_message: Some(inspection.message.clone()),
        last_base_oid: Some(inspection.base_oid.clone()),
        last_head_oid: Some(inspection.head_oid.clone()),
        last_inspected_at_ms: Some(now_ms()),
    }
}

pub fn upsert_remembered_repository(
    repositories: &mut Vec<RememberedLocalRepository>,
    repository: RememberedLocalRepository,
) {
    repositories.retain(|item| item.repository != repository.repository);
    repositories.insert(0, repository);
}

pub fn remember_repository_path(
    cache: &CacheStore,
    repositories: &mut Vec<RememberedLocalRepository>,
    path: &Path,
) -> Result<LocalReviewInspection, String> {
    let inspection = inspect_working_checkout(path, false)?;
    let remembered = remembered_from_inspection(&inspection);
    upsert_remembered_repository(repositories, remembered);
    save_remembered_repositories(cache, repositories)?;
    Ok(inspection)
}

pub fn inspect_working_checkout(path: &Path, fetch: bool) -> Result<LocalReviewInspection, String> {
    let root = resolve_git_root(path)?.ok_or_else(|| {
        "The selected folder is not inside a git checkout. Pick a repository folder.".to_string()
    })?;
    reject_app_managed_checkout(&root)?;

    let repository = resolve_repository_identity(&root)?.ok_or_else(|| {
        "The checkout does not have a GitHub-style remote. Add an origin remote that points at owner/repo."
            .to_string()
    })?;

    if fetch {
        let output = run_git(&root, ["fetch", "--all", "--prune", "--no-tags"])?;
        if output.exit_code != Some(0) {
            return Err(process_error(output, "Failed to fetch remotes"));
        }
    }

    let branch = current_branch(&root)?.ok_or_else(|| {
        "This checkout is detached. Check out a branch before starting a local review.".to_string()
    })?;
    let head_oid = current_head_oid(&root)?.ok_or_else(|| {
        "This checkout does not have a HEAD commit yet. Make an initial commit first.".to_string()
    })?;
    let clean = worktree_is_clean(&root)?;
    let (base_ref_name, base_oid) = resolve_review_base(&root)?;
    let commits_count = rev_list_count(&root, &base_oid, &head_oid)?;
    let raw_diff = if clean {
        if commits_count == 0 {
            String::new()
        } else {
            diff_between(&root, &base_oid, &head_oid)?
        }
    } else {
        diff_worktree(&root, &base_oid)?
    };
    let key = local_review_key(
        &repository,
        &branch,
        &base_oid,
        &local_review_head_identity(&head_oid, clean, &raw_diff),
    );

    let (status, message, parsed_diff, files) = if raw_diff.trim().is_empty() {
        (
            LocalReviewStatusKind::NoDiff,
            if clean {
                "No local changes are ahead of the selected base.".to_string()
            } else {
                "No reviewable local changes were found after comparing the working tree to the selected base."
                    .to_string()
            },
            Vec::new(),
            Vec::new(),
        )
    } else {
        let parsed_diff = parse_unified_diff(&raw_diff);
        let files = files_from_diff(&raw_diff, &parsed_diff);
        (
            LocalReviewStatusKind::Ready,
            local_review_ready_message(clean, commits_count),
            parsed_diff,
            files,
        )
    };

    let additions = files.iter().map(|file| file.additions).sum::<i64>();
    let deletions = files.iter().map(|file| file.deletions).sum::<i64>();
    let local_repository_status = LocalRepositoryStatus {
        repository: repository.clone(),
        path: Some(root.display().to_string()),
        source: "local-review".to_string(),
        exists: true,
        is_valid_repository: true,
        current_head_oid: Some(head_oid.clone()),
        expected_head_oid: Some(head_oid.clone()),
        matches_expected_head: true,
        is_worktree_clean: clean,
        ready_for_local_features: true,
        message: message.clone(),
    };
    let detail = synthetic_detail(SyntheticDetailInput {
        key: key.clone(),
        repository: repository.clone(),
        branch: branch.clone(),
        base_ref_name: base_ref_name.clone(),
        base_oid: base_oid.clone(),
        head_oid: head_oid.clone(),
        commits_count,
        status,
        message: message.clone(),
        additions,
        deletions,
        files,
        raw_diff,
        parsed_diff,
    });
    let summary = summary_from_detail(&detail, &key);

    Ok(LocalReviewInspection {
        repository,
        root,
        branch,
        base_ref_name,
        base_oid,
        head_oid,
        commits_count,
        status,
        message,
        local_repository_status,
        detail,
        summary,
        key,
    })
}

pub fn detail_snapshot_from_inspection(
    inspection: &LocalReviewInspection,
) -> PullRequestDetailSnapshot {
    PullRequestDetailSnapshot {
        auth: AuthState {
            is_authenticated: false,
            active_login: None,
            active_hostname: None,
            message: "Local review".to_string(),
        },
        loaded_from_cache: false,
        fetched_at_ms: Some(now_ms()),
        detail: Some(inspection.detail.clone()),
    }
}

fn synthetic_detail(input: SyntheticDetailInput) -> PullRequestDetail {
    let title = match input.status {
        LocalReviewStatusKind::NoDiff => {
            format!("Local review: {} has no local changes", input.branch)
        }
        LocalReviewStatusKind::Blocked => {
            format!("Local review blocked: {}", input.branch)
        }
        _ => format!("Local review: {}", input.branch),
    };

    PullRequestDetail {
        id: input.key.clone(),
        repository: input.repository,
        number: 0,
        title,
        body: input.message,
        url: String::new(),
        author_login: std::env::var("USER").unwrap_or_else(|_| "local".to_string()),
        author_avatar_url: None,
        state: "LOCAL".to_string(),
        is_draft: false,
        review_decision: None,
        base_ref_name: input.base_ref_name,
        head_ref_name: input.branch,
        base_ref_oid: Some(input.base_oid),
        head_ref_oid: Some(input.head_oid),
        additions: input.additions,
        deletions: input.deletions,
        changed_files: input.files.len() as i64,
        comments_count: 0,
        commits_count: input.commits_count,
        created_at: input.key.clone(),
        updated_at: input.key,
        labels: Vec::new(),
        reviewers: Vec::new(),
        reviewer_avatar_urls: Default::default(),
        comments: Vec::new(),
        latest_reviews: Vec::new(),
        review_threads: Vec::new(),
        files: input.files,
        raw_diff: input.raw_diff,
        parsed_diff: input.parsed_diff,
        data_completeness: PullRequestDataCompleteness::default(),
    }
}

struct SyntheticDetailInput {
    key: String,
    repository: String,
    branch: String,
    base_ref_name: String,
    base_oid: String,
    head_oid: String,
    commits_count: i64,
    status: LocalReviewStatusKind,
    message: String,
    additions: i64,
    deletions: i64,
    files: Vec<PullRequestFile>,
    raw_diff: String,
    parsed_diff: Vec<ParsedDiffFile>,
}

fn summary_from_detail(detail: &PullRequestDetail, key: &str) -> PullRequestSummary {
    PullRequestSummary {
        local_key: Some(key.to_string()),
        repository: detail.repository.clone(),
        number: detail.number,
        title: detail.title.clone(),
        author_login: detail.author_login.clone(),
        author_avatar_url: None,
        is_draft: false,
        comments_count: 0,
        additions: detail.additions,
        deletions: detail.deletions,
        changed_files: detail.changed_files,
        state: detail.state.clone(),
        review_decision: None,
        updated_at: detail.updated_at.clone(),
        url: String::new(),
    }
}

fn local_review_key(repository: &str, branch: &str, base_oid: &str, head_oid: &str) -> String {
    format!("local:{repository}:{branch}:{base_oid}:{head_oid}")
}

fn resolve_git_root(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Err(format!(
            "The selected path '{}' does not exist.",
            path.display()
        ));
    }

    let output = run_git(path, ["rev-parse", "--show-toplevel"])?;
    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let root = output.stdout.trim();
    if root.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(root)))
    }
}

fn reject_app_managed_checkout(root: &Path) -> Result<(), String> {
    let managed_root = app_storage::managed_repositories_root();
    if root.starts_with(&managed_root) {
        return Err(
            "Local Review uses your working checkouts, not Remiss-managed pull request checkouts."
                .to_string(),
        );
    }
    Ok(())
}

pub fn resolve_repository_identity(path: &Path) -> Result<Option<String>, String> {
    let output = run_git(path, ["remote"])?;
    if output.exit_code != Some(0) {
        return Ok(None);
    }

    for remote in output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let remote_output = run_git(path, ["remote", "get-url", remote])?;
        if remote_output.exit_code != Some(0) {
            continue;
        }

        if let Some(repository) = normalized_remote_repository(&remote_output.stdout) {
            return Ok(Some(repository));
        }
    }

    Ok(None)
}

pub fn normalized_remote_repository(remote_url: &str) -> Option<String> {
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

fn current_branch(path: &Path) -> Result<Option<String>, String> {
    let output = run_git(path, ["branch", "--show-current"])?;
    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let branch = output.stdout.trim();
    if branch.is_empty() {
        Ok(None)
    } else {
        Ok(Some(branch.to_string()))
    }
}

fn current_head_oid(path: &Path) -> Result<Option<String>, String> {
    let output = run_git(path, ["rev-parse", "HEAD"])?;
    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let head = output.stdout.trim();
    if head.is_empty() {
        Ok(None)
    } else {
        Ok(Some(head.to_string()))
    }
}

fn worktree_is_clean(path: &Path) -> Result<bool, String> {
    let output = run_git(path, ["status", "--porcelain", "--untracked-files=normal"])?;
    if output.exit_code != Some(0) {
        return Ok(false);
    }
    Ok(output.stdout.trim().is_empty())
}

fn resolve_review_base(path: &Path) -> Result<(String, String), String> {
    if let Some(upstream) = upstream_ref(path)? {
        if let Some(base_oid) = merge_base(path, &upstream, "HEAD")? {
            return Ok((upstream, base_oid));
        }
    }

    for candidate in default_branch_candidates(path)? {
        if verify_commit(path, &candidate)? {
            if let Some(base_oid) = merge_base(path, &candidate, "HEAD")? {
                return Ok((candidate, base_oid));
            }
        }
    }

    Err("Could not resolve a review base. Set a branch upstream or fetch the default branch remote ref."
        .to_string())
}

fn upstream_ref(path: &Path) -> Result<Option<String>, String> {
    let output = run_git(
        path,
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let upstream = output.stdout.trim();
    if upstream.is_empty() {
        Ok(None)
    } else {
        Ok(Some(upstream.to_string()))
    }
}

fn default_branch_candidates(path: &Path) -> Result<Vec<String>, String> {
    let mut candidates = Vec::new();
    let origin_head = run_git(
        path,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )?;
    if origin_head.exit_code == Some(0) {
        let value = origin_head.stdout.trim();
        if !value.is_empty() {
            candidates.push(value.to_string());
        }
    }

    let remotes = run_git(path, ["remote"])?;
    if remotes.exit_code == Some(0) {
        for remote in remotes
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            candidates.push(format!("{remote}/main"));
            candidates.push(format!("{remote}/master"));
        }
    }

    candidates.dedup();
    Ok(candidates)
}

fn verify_commit(path: &Path, reference: &str) -> Result<bool, String> {
    let output = run_git(
        path,
        ["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    Ok(output.exit_code == Some(0))
}

fn merge_base(path: &Path, left: &str, right: &str) -> Result<Option<String>, String> {
    let output = run_git(path, ["merge-base", left, right])?;
    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let base = output.stdout.trim();
    if base.is_empty() {
        Ok(None)
    } else {
        Ok(Some(base.to_string()))
    }
}

fn rev_list_count(path: &Path, base_oid: &str, head_oid: &str) -> Result<i64, String> {
    let range = format!("{base_oid}..{head_oid}");
    let output = run_git(path, ["rev-list", "--count", &range])?;
    if output.exit_code != Some(0) {
        return Err(process_error(output, "Failed to count unpushed commits"));
    }

    output
        .stdout
        .trim()
        .parse::<i64>()
        .map_err(|error| format!("Failed to parse unpushed commit count: {error}"))
}

fn diff_between(path: &Path, base_oid: &str, head_oid: &str) -> Result<String, String> {
    let output = run_git(
        path,
        [
            "diff",
            "--binary",
            "--find-renames",
            "--find-copies",
            base_oid,
            head_oid,
        ],
    )?;
    if output.exit_code != Some(0) {
        return Err(process_error(output, "Failed to build local review diff"));
    }

    Ok(command_stdout_text(&output))
}

fn diff_worktree(path: &Path, base_oid: &str) -> Result<String, String> {
    let output = run_git(
        path,
        [
            "diff",
            "--binary",
            "--find-renames",
            "--find-copies",
            base_oid,
            "--",
        ],
    )?;
    if output.exit_code != Some(0) {
        return Err(process_error(
            output,
            "Failed to build local review working tree diff",
        ));
    }

    let mut raw_diff = command_stdout_text(&output);
    for untracked_path in untracked_paths(path)? {
        append_diff(&mut raw_diff, &diff_untracked_file(path, &untracked_path)?);
    }

    Ok(raw_diff)
}

fn untracked_paths(path: &Path) -> Result<Vec<String>, String> {
    let output = run_git(path, ["ls-files", "--others", "--exclude-standard", "-z"])?;
    if output.exit_code != Some(0) {
        return Err(process_error(
            output,
            "Failed to list untracked files for local review",
        ));
    }

    output
        .stdout_bytes
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| {
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                "Git returned a non-UTF-8 untracked path for local review.".to_string()
            })
        })
        .collect()
}

fn diff_untracked_file(root: &Path, relative_path: &str) -> Result<String, String> {
    let full_path = root.join(relative_path);
    if fs::metadata(&full_path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        return Ok(String::new());
    }

    let output = run_git(
        root,
        [
            "diff",
            "--no-index",
            "--binary",
            "--",
            "/dev/null",
            relative_path,
        ],
    )?;
    match output.exit_code {
        Some(0) | Some(1) => Ok(command_stdout_text(&output)),
        _ => Err(process_error(
            output,
            "Failed to build local review diff for untracked file",
        )),
    }
}

fn append_diff(raw_diff: &mut String, addition: &str) {
    if addition.trim().is_empty() {
        return;
    }
    if !raw_diff.is_empty() && !raw_diff.ends_with('\n') {
        raw_diff.push('\n');
    }
    raw_diff.push_str(addition);
    if !raw_diff.ends_with('\n') {
        raw_diff.push('\n');
    }
}

fn local_review_head_identity(head_oid: &str, clean: bool, raw_diff: &str) -> String {
    if clean {
        return head_oid.to_string();
    }

    format!("{head_oid}:worktree-{}", short_hash(raw_diff))
}

fn local_review_ready_message(clean: bool, commits_count: i64) -> String {
    if clean {
        return format!(
            "{} committed change{} ready for local review.",
            commits_count,
            if commits_count == 1 { "" } else { "s" }
        );
    }

    if commits_count == 0 {
        "Working tree changes ready for local review.".to_string()
    } else {
        format!(
            "Working tree changes and {} committed change{} ready for local review.",
            commits_count,
            if commits_count == 1 { "" } else { "s" }
        )
    }
}

fn short_hash(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
        .chars()
        .take(16)
        .collect()
}

fn command_stdout_text(output: &CommandOutput) -> String {
    String::from_utf8_lossy(&output.stdout_bytes).to_string()
}

fn files_from_diff(raw_diff: &str, parsed_diff: &[ParsedDiffFile]) -> Vec<PullRequestFile> {
    let metadata = diff_file_metadata(raw_diff);
    parsed_diff
        .iter()
        .map(|parsed| {
            let additions = parsed
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == DiffLineKind::Addition)
                .count() as i64;
            let deletions = parsed
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == DiffLineKind::Deletion)
                .count() as i64;
            let change_type = metadata
                .get(&parsed.path)
                .map(|item| item.change_type.clone())
                .unwrap_or_else(|| {
                    if parsed
                        .previous_path
                        .as_deref()
                        .is_some_and(|previous| previous != parsed.path)
                    {
                        "RENAMED".to_string()
                    } else {
                        "MODIFIED".to_string()
                    }
                });

            PullRequestFile {
                path: parsed.path.clone(),
                additions,
                deletions,
                change_type,
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
struct DiffFileMetadata {
    change_type: String,
}

fn diff_file_metadata(raw_diff: &str) -> HashMap<String, DiffFileMetadata> {
    let mut metadata = HashMap::new();
    let mut current_path = None::<String>;
    let mut current_type = "MODIFIED".to_string();

    for line in raw_diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(path) = current_path.take() {
                metadata.insert(
                    path,
                    DiffFileMetadata {
                        change_type: current_type.clone(),
                    },
                );
            }

            let mut parts = rest.split_whitespace();
            let _previous = parts.next();
            current_path = parts.next().map(normalize_diff_path);
            current_type = "MODIFIED".to_string();
            continue;
        }

        if line.starts_with("new file mode ") {
            current_type = "ADDED".to_string();
        } else if line.starts_with("deleted file mode ") {
            current_type = "DELETED".to_string();
        } else if let Some(path) = line.strip_prefix("rename to ") {
            current_path = Some(path.to_string());
            current_type = "RENAMED".to_string();
        } else if let Some(path) = line.strip_prefix("copy to ") {
            current_path = Some(path.to_string());
            current_type = "COPIED".to_string();
        }
    }

    if let Some(path) = current_path.take() {
        metadata.insert(
            path,
            DiffFileMetadata {
                change_type: current_type,
            },
        );
    }

    metadata
}

fn normalize_diff_path(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .strip_prefix("a/")
        .or_else(|| path.trim().trim_matches('"').strip_prefix("b/"))
        .unwrap_or_else(|| path.trim().trim_matches('"'))
        .to_string()
}

fn run_git<const N: usize>(path: &Path, args: [&str; N]) -> Result<CommandOutput, String> {
    let mut command_args = vec!["-C".to_string(), path.display().to_string()];
    command_args.extend(args.into_iter().map(str::to_string));
    let output = CommandRunner::new("git")
        .args(command_args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .timeout(LOCAL_REVIEW_GIT_TIMEOUT)
        .run()?;
    if output.timed_out {
        return Err("git command timed out after 120 seconds.".to_string());
    }
    Ok(output)
}

fn process_error(output: CommandOutput, prefix: &str) -> String {
    if !output.stderr.is_empty() {
        format!("{prefix}: {}", output.stderr)
    } else if !output.stdout.is_empty() {
        format!("{prefix}: {}", output.stdout)
    } else {
        prefix.to_string()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
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
        inspect_working_checkout, normalized_remote_repository, upsert_remembered_repository,
        LocalReviewStatusKind, RememberedLocalRepository,
    };

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    struct GitFixture {
        root: PathBuf,
        remote: PathBuf,
        _workspace: PathBuf,
    }

    impl GitFixture {
        fn new(remote_repository: &str) -> Self {
            let workspace = unique_test_directory("local-review");
            let remote = workspace.join("remote.git");
            fs::create_dir_all(&remote).expect("remote directory");
            run_git(&remote, ["init", "--bare"]);

            let root = workspace.join("repo");
            fs::create_dir_all(&root).expect("repo directory");
            run_git(&root, ["init"]);
            run_git(&root, ["config", "user.name", "Remiss Tests"]);
            run_git(&root, ["config", "user.email", "remiss-tests@example.com"]);
            run_git(&root, ["remote", "add", "origin", remote.to_str().unwrap()]);
            run_git(
                &root,
                [
                    "remote",
                    "set-url",
                    "origin",
                    &format!("git@github.com:{remote_repository}.git"),
                ],
            );
            run_git(&root, ["branch", "-M", "main"]);

            Self {
                root,
                remote,
                _workspace: workspace,
            }
        }

        fn set_file(&self, path: &str, contents: &str) {
            let full_path = self.root.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).expect("parent directory");
            }
            fs::write(full_path, contents).expect("write file");
        }

        fn commit_all(&self, message: &str) -> String {
            run_git(&self.root, ["add", "."]);
            run_git(&self.root, ["commit", "-m", message]);
            git_output(&self.root, ["rev-parse", "HEAD"])
        }

        fn push_main(&self) {
            run_git(
                &self.root,
                ["remote", "set-url", "origin", self.remote.to_str().unwrap()],
            );
            run_git(&self.root, ["push", "-u", "origin", "main"]);
            run_git(
                &self.root,
                [
                    "remote",
                    "set-url",
                    "origin",
                    "git@github.com:openai/example.git",
                ],
            );
            run_git(
                &self.root,
                ["update-ref", "refs/remotes/origin/main", "HEAD"],
            );
        }

        fn checkout_branch(&self, branch: &str) {
            run_git(&self.root, ["checkout", "-b", branch]);
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
        fs::create_dir_all(&path).expect("temp directory");
        path
    }

    fn run_git<const N: usize>(path: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("run git");
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
            .expect("run git");
        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn resolves_repository_identity_from_github_remotes() {
        assert_eq!(
            normalized_remote_repository("https://github.com/OpenAI/Example.git"),
            Some("openai/example".to_string())
        );
        assert_eq!(
            normalized_remote_repository("git@github.com:OpenAI/Example.git"),
            Some("openai/example".to_string())
        );
    }

    #[test]
    fn remembered_checkout_replaces_same_repository_identity() {
        let mut repositories = vec![RememberedLocalRepository {
            repository: "openai/example".to_string(),
            path: "/first".to_string(),
            last_branch: Some("main".to_string()),
            last_status: LocalReviewStatusKind::NoDiff,
            last_message: None,
            last_base_oid: None,
            last_head_oid: None,
            last_inspected_at_ms: None,
        }];

        upsert_remembered_repository(
            &mut repositories,
            RememberedLocalRepository {
                repository: "openai/example".to_string(),
                path: "/second".to_string(),
                last_branch: Some("feature".to_string()),
                last_status: LocalReviewStatusKind::Ready,
                last_message: None,
                last_base_oid: None,
                last_head_oid: None,
                last_inspected_at_ms: None,
            },
        );

        assert_eq!(repositories.len(), 1);
        assert_eq!(repositories[0].path, "/second");
        assert_eq!(repositories[0].last_branch.as_deref(), Some("feature"));
    }

    #[test]
    fn inspects_clean_branch_ahead_of_upstream() {
        let fixture = GitFixture::new("openai/example");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        fixture.commit_all("initial");
        fixture.push_main();
        fixture.checkout_branch("feature");
        run_git(
            &fixture.root,
            ["branch", "--set-upstream-to", "origin/main"],
        );
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let head = fixture.commit_all("feature");

        let inspection = inspect_working_checkout(&fixture.root, false).expect("inspection");

        assert_eq!(inspection.repository, "openai/example");
        assert_eq!(inspection.branch, "feature");
        assert_eq!(inspection.head_oid, head);
        assert_eq!(inspection.commits_count, 1);
        assert_eq!(inspection.status, LocalReviewStatusKind::Ready);
        assert_eq!(inspection.detail.files.len(), 1);
        assert_eq!(inspection.detail.files[0].path, "src/lib.rs");
        assert_eq!(inspection.detail.files[0].additions, 1);
        assert_eq!(inspection.detail.files[0].deletions, 1);
        assert!(inspection.key.starts_with("local:openai/example:feature:"));
    }

    #[test]
    fn inspects_no_unpushed_commits_as_empty_review() {
        let fixture = GitFixture::new("openai/example");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = fixture.commit_all("initial");
        fixture.push_main();

        let inspection = inspect_working_checkout(&fixture.root, false).expect("inspection");

        assert_eq!(inspection.status, LocalReviewStatusKind::NoDiff);
        assert_eq!(inspection.base_oid, head);
        assert_eq!(inspection.head_oid, head);
        assert!(inspection.detail.files.is_empty());
        assert!(inspection.detail.raw_diff.is_empty());
    }

    #[test]
    fn falls_back_to_default_branch_remote_when_upstream_is_missing() {
        let fixture = GitFixture::new("openai/example");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let base = fixture.commit_all("initial");
        fixture.push_main();
        fixture.checkout_branch("feature");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        fixture.commit_all("feature");

        let inspection = inspect_working_checkout(&fixture.root, false).expect("inspection");

        assert_eq!(inspection.status, LocalReviewStatusKind::Ready);
        assert_eq!(inspection.base_ref_name, "origin/main");
        assert_eq!(inspection.base_oid, base);
    }

    #[test]
    fn dirty_worktree_reviews_uncommitted_tracked_changes() {
        let fixture = GitFixture::new("openai/example");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        fixture.commit_all("initial");
        fixture.push_main();
        fixture.checkout_branch("feature");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");

        let inspection = inspect_working_checkout(&fixture.root, false).expect("inspection");

        assert_eq!(inspection.status, LocalReviewStatusKind::Ready);
        assert!(!inspection.local_repository_status.is_worktree_clean);
        assert!(inspection.local_repository_status.ready_for_local_features);
        assert_eq!(inspection.detail.files.len(), 1);
        assert_eq!(inspection.detail.files[0].path, "src/lib.rs");
        assert_eq!(inspection.detail.files[0].additions, 1);
        assert_eq!(inspection.detail.files[0].deletions, 1);
        assert!(inspection.message.contains("Working tree changes"));
        assert!(inspection.key.contains(":worktree-"));
    }

    #[test]
    fn dirty_worktree_reviews_untracked_files() {
        let fixture = GitFixture::new("openai/example");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        fixture.commit_all("initial");
        fixture.push_main();
        fixture.checkout_branch("feature");
        fixture.set_file("src/new.rs", "pub fn new_value() -> i32 { 2 }\n");

        let inspection = inspect_working_checkout(&fixture.root, false).expect("inspection");

        assert_eq!(inspection.status, LocalReviewStatusKind::Ready);
        assert!(!inspection.local_repository_status.is_worktree_clean);
        assert_eq!(inspection.detail.files.len(), 1);
        assert_eq!(inspection.detail.files[0].path, "src/new.rs");
        assert_eq!(inspection.detail.files[0].change_type, "ADDED");
        assert_eq!(inspection.detail.files[0].additions, 1);
        assert_eq!(inspection.detail.files[0].deletions, 0);
    }

    #[test]
    fn dirty_worktree_key_changes_when_diff_changes() {
        let fixture = GitFixture::new("openai/example");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        fixture.commit_all("initial");
        fixture.push_main();
        fixture.checkout_branch("feature");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let first = inspect_working_checkout(&fixture.root, false).expect("first inspection");

        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 3 }\n");
        let second = inspect_working_checkout(&fixture.root, false).expect("second inspection");

        assert_ne!(first.key, second.key);
        assert_eq!(first.head_oid, second.head_oid);
    }

    #[test]
    fn fetch_failure_is_reported() {
        let fixture = GitFixture::new("openai/example");
        fixture.set_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        fixture.commit_all("initial");
        fixture.push_main();
        run_git(
            &fixture.root,
            [
                "remote",
                "set-url",
                "origin",
                "file:///tmp/remiss-missing-remote.git",
            ],
        );

        let error = inspect_working_checkout(&fixture.root, true).expect_err("fetch failure");

        assert!(error.contains("Failed to fetch remotes"));
    }

    #[test]
    fn remembered_repositories_roundtrip_through_cache() {
        let cache =
            CacheStore::new(unique_test_directory("local-review-cache").join("cache.sqlite3"))
                .expect("cache");
        let repositories = vec![RememberedLocalRepository {
            repository: "openai/example".to_string(),
            path: "/repo".to_string(),
            last_branch: Some("main".to_string()),
            last_status: LocalReviewStatusKind::NoDiff,
            last_message: Some("No diff".to_string()),
            last_base_oid: Some("base".to_string()),
            last_head_oid: Some("head".to_string()),
            last_inspected_at_ms: Some(10),
        }];

        super::save_remembered_repositories(&cache, &repositories).expect("save");
        let loaded = super::load_remembered_repositories(&cache).expect("load");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].repository, "openai/example");
        assert_eq!(loaded[0].last_status, LocalReviewStatusKind::NoDiff);
    }
}
