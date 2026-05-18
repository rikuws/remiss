use std::{
    collections::BTreeMap,
    path::{Component, Path},
    time::Duration,
};

use crate::{
    app_storage,
    command_runner::{CommandOutput, CommandRunner},
};

const LOCAL_GIT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Default)]
pub struct LocalChangeStatus {
    pub branch: String,
    pub upstream: Option<String>,
    pub push_target: Option<String>,
    pub committed_ahead_count: i64,
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub untracked_count: usize,
    pub has_conflicts: bool,
    pub files: Vec<LocalChangeFileStatus>,
}

impl LocalChangeStatus {
    pub fn file(&self, path: &str) -> Option<&LocalChangeFileStatus> {
        self.files.iter().find(|file| file.path == path)
    }

    pub fn has_stageable_files(&self) -> bool {
        self.files.iter().any(LocalChangeFileStatus::is_stageable)
    }

    pub fn has_unstageable_files(&self) -> bool {
        self.files.iter().any(|file| file.staged)
    }

    pub fn can_commit(&self) -> bool {
        self.staged_count > 0 && !self.has_conflicts
    }

    pub fn can_push(&self) -> bool {
        self.committed_ahead_count > 0
            && self.staged_count == 0
            && !self.has_conflicts
            && self.push_target.is_some()
    }

    pub fn summary_label(&self) -> String {
        let mut parts = Vec::new();
        if self.staged_count > 0 {
            parts.push(format!("{} staged", self.staged_count));
        }
        if self.unstaged_count > 0 {
            parts.push(format!("{} unstaged", self.unstaged_count));
        }
        if self.untracked_count > 0 {
            parts.push(format!("{} untracked", self.untracked_count));
        }
        if self.committed_ahead_count > 0 {
            parts.push(format!("{} ahead", self.committed_ahead_count));
        }
        if parts.is_empty() {
            "No local changes".to_string()
        } else {
            parts.join(" / ")
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LocalChangeFileStatus {
    pub path: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

impl LocalChangeFileStatus {
    pub fn is_stageable(&self) -> bool {
        self.unstaged || self.untracked || self.conflicted && !self.staged
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalGitOperationKind {
    Stage,
    Unstage,
    Commit,
    Push,
}

impl LocalGitOperationKind {
    pub fn progress_label(self) -> &'static str {
        match self {
            Self::Stage => "Staging changes...",
            Self::Unstage => "Unstaging changes...",
            Self::Commit => "Committing staged changes...",
            Self::Push => "Pushing branch...",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LocalGitOperationState {
    pub running: Option<LocalGitOperationKind>,
    pub message: Option<String>,
    pub error: Option<String>,
}

impl LocalGitOperationState {
    pub fn running(kind: LocalGitOperationKind) -> Self {
        Self {
            running: Some(kind),
            message: Some(kind.progress_label().to_string()),
            error: None,
        }
    }
}

pub fn inspect_status(
    root: &Path,
    branch: &str,
    committed_ahead_count: i64,
) -> Result<LocalChangeStatus, String> {
    reject_app_managed_checkout(root)?;

    let upstream = upstream_ref(root)?;
    let push_target = upstream.clone().or_else(|| {
        remote_exists(root, "origin")
            .ok()
            .filter(|exists| *exists)
            .map(|_| format!("origin/{branch}"))
    });
    let ahead_count = if let Some(upstream) = upstream.as_deref() {
        rev_list_count(root, upstream, "HEAD").unwrap_or(committed_ahead_count)
    } else {
        committed_ahead_count
    };
    let files = status_files(root)?;
    let staged_count = files.iter().filter(|file| file.staged).count();
    let unstaged_count = files.iter().filter(|file| file.unstaged).count();
    let untracked_count = files.iter().filter(|file| file.untracked).count();
    let has_conflicts = files.iter().any(|file| file.conflicted);

    Ok(LocalChangeStatus {
        branch: branch.to_string(),
        upstream,
        push_target,
        committed_ahead_count: ahead_count,
        staged_count,
        unstaged_count,
        untracked_count,
        has_conflicts,
        files,
    })
}

pub fn stage_file(root: &Path, path: &str) -> Result<(), String> {
    reject_app_managed_checkout(root)?;
    validate_relative_path(path)?;
    expect_success(run_git(root, ["add", "--", path])?, "Failed to stage file")
}

pub fn unstage_file(root: &Path, path: &str) -> Result<(), String> {
    reject_app_managed_checkout(root)?;
    validate_relative_path(path)?;
    expect_success(
        run_git(root, ["restore", "--staged", "--", path])?,
        "Failed to unstage file",
    )
}

pub fn stage_all(root: &Path) -> Result<(), String> {
    reject_app_managed_checkout(root)?;
    expect_success(run_git(root, ["add", "-A"])?, "Failed to stage changes")
}

pub fn unstage_all(root: &Path) -> Result<(), String> {
    reject_app_managed_checkout(root)?;
    expect_success(
        run_git(root, ["restore", "--staged", "--", "."])?,
        "Failed to unstage changes",
    )
}

pub fn commit_staged(root: &Path, message: &str) -> Result<(), String> {
    reject_app_managed_checkout(root)?;
    let message = message.trim();
    if message.is_empty() {
        return Err("Enter a commit message before committing.".to_string());
    }
    expect_success(
        run_git(root, ["commit", "-m", message])?,
        "Failed to commit staged changes",
    )
}

pub fn push_current_branch(root: &Path) -> Result<(), String> {
    reject_app_managed_checkout(root)?;
    let branch = current_branch(root)?.ok_or_else(|| {
        "This checkout is detached. Check out a branch before pushing.".to_string()
    })?;

    if upstream_ref(root)?.is_some() {
        return expect_success(run_git(root, ["push"])?, "Failed to push branch");
    }

    if !remote_exists(root, "origin")? {
        return Err(
            "No upstream branch or origin remote is configured for this branch.".to_string(),
        );
    }

    expect_success(
        run_git(root, ["push", "-u", "origin", &branch])?,
        "Failed to push branch and set upstream",
    )
}

fn status_files(root: &Path) -> Result<Vec<LocalChangeFileStatus>, String> {
    let output = run_git(
        root,
        ["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )?;
    if output.exit_code != Some(0) {
        return Err(process_error(output, "Failed to inspect local Git status"));
    }
    parse_status_entries(&output.stdout_bytes)
}

fn parse_status_entries(bytes: &[u8]) -> Result<Vec<LocalChangeFileStatus>, String> {
    let entries = bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    let mut files = BTreeMap::<String, LocalChangeFileStatus>::new();
    let mut index = 0;

    while let Some(entry) = entries.get(index) {
        if entry.len() < 4 {
            index += 1;
            continue;
        }

        let x = entry[0] as char;
        let y = entry[1] as char;
        let path_bytes = &entry[3..];
        let path = String::from_utf8(path_bytes.to_vec())
            .map_err(|_| "Git returned a non-UTF-8 path in local status.".to_string())?;
        let conflicted = is_conflict_status(x, y);
        let untracked = x == '?' && y == '?';
        let staged = !untracked && x != ' ' && x != '!' && !conflicted;
        let unstaged = !untracked && y != ' ' && y != '!';

        let file = files
            .entry(path.clone())
            .or_insert_with(|| LocalChangeFileStatus {
                path,
                ..LocalChangeFileStatus::default()
            });
        file.staged |= staged;
        file.unstaged |= unstaged;
        file.untracked |= untracked;
        file.conflicted |= conflicted;

        if matches!(x, 'R' | 'C') {
            index += 2;
        } else {
            index += 1;
        }
    }

    Ok(files.into_values().collect())
}

fn is_conflict_status(x: char, y: char) -> bool {
    matches!(
        (x, y),
        ('D', 'D') | ('A', 'U') | ('U', 'D') | ('U', 'A') | ('D', 'U') | ('A', 'A') | ('U', 'U')
    )
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

fn remote_exists(path: &Path, remote: &str) -> Result<bool, String> {
    let output = run_git(path, ["remote"])?;
    if output.exit_code != Some(0) {
        return Ok(false);
    }
    Ok(output.stdout.lines().any(|line| line.trim() == remote))
}

fn rev_list_count(path: &Path, base: &str, head: &str) -> Result<i64, String> {
    let range = format!("{base}..{head}");
    let output = run_git(path, ["rev-list", "--count", &range])?;
    if output.exit_code != Some(0) {
        return Err(process_error(output, "Failed to count commits ahead"));
    }
    output
        .stdout
        .trim()
        .parse::<i64>()
        .map_err(|error| format!("Failed to parse commits ahead: {error}"))
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    let value = Path::new(path);
    if value.is_absolute() {
        return Err("Git file operations require a repository-relative path.".to_string());
    }
    if value
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("Git file operations require a safe repository-relative path.".to_string());
    }
    Ok(())
}

fn reject_app_managed_checkout(root: &Path) -> Result<(), String> {
    let managed_root = app_storage::managed_repositories_root();
    if root.starts_with(&managed_root) {
        return Err(
            "Remiss will not run mutable Git commands in app-managed checkouts.".to_string(),
        );
    }
    Ok(())
}

fn run_git<const N: usize>(path: &Path, args: [&str; N]) -> Result<CommandOutput, String> {
    let mut command_args = vec!["-C".to_string(), path.display().to_string()];
    command_args.extend(args.into_iter().map(str::to_string));
    let output = CommandRunner::new("git")
        .args(command_args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .timeout(LOCAL_GIT_TIMEOUT)
        .run()?;
    if output.timed_out {
        return Err("git command timed out after 120 seconds.".to_string());
    }
    Ok(output)
}

fn expect_success(output: CommandOutput, prefix: &str) -> Result<(), String> {
    if output.exit_code == Some(0) {
        Ok(())
    } else {
        Err(process_error(output, prefix))
    }
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        commit_staged, inspect_status, parse_status_entries, push_current_branch, stage_file,
        unstage_file,
    };

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    struct GitFixture {
        root: PathBuf,
        remote: PathBuf,
        _workspace: PathBuf,
    }

    impl GitFixture {
        fn new() -> Self {
            let workspace = unique_test_directory("local-git");
            let remote = workspace.join("remote.git");
            fs::create_dir_all(&remote).expect("remote directory");
            run_git(&remote, ["init", "--bare"]);

            let root = workspace.join("repo");
            fs::create_dir_all(&root).expect("repo directory");
            run_git(&root, ["init"]);
            run_git(&root, ["config", "user.name", "Remiss Tests"]);
            run_git(&root, ["config", "user.email", "remiss-tests@example.com"]);
            run_git(&root, ["remote", "add", "origin", remote.to_str().unwrap()]);
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

        fn commit_all(&self, message: &str) {
            run_git(&self.root, ["add", "."]);
            run_git(&self.root, ["commit", "-m", message]);
        }

        fn push_main(&self) {
            run_git(&self.root, ["push", "-u", "origin", "main"]);
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
    fn parses_staged_unstaged_and_untracked_status_entries() {
        let status = parse_status_entries(b"M  staged.rs\0 M unstaged.rs\0?? new.rs\0")
            .expect("status should parse");

        let staged = status.iter().find(|file| file.path == "staged.rs").unwrap();
        assert!(staged.staged);
        assert!(!staged.unstaged);

        let unstaged = status
            .iter()
            .find(|file| file.path == "unstaged.rs")
            .unwrap();
        assert!(!unstaged.staged);
        assert!(unstaged.unstaged);

        let untracked = status.iter().find(|file| file.path == "new.rs").unwrap();
        assert!(untracked.untracked);
    }

    #[test]
    fn stages_and_unstages_one_file() {
        let fixture = GitFixture::new();
        fixture.set_file("README.md", "initial\n");
        fixture.commit_all("initial");
        fixture.set_file("README.md", "changed\n");

        stage_file(&fixture.root, "README.md").expect("stage file");
        let status = inspect_status(&fixture.root, "main", 0).expect("inspect status");
        assert_eq!(status.staged_count, 1);
        assert_eq!(status.unstaged_count, 0);

        unstage_file(&fixture.root, "README.md").expect("unstage file");
        let status = inspect_status(&fixture.root, "main", 0).expect("inspect status");
        assert_eq!(status.staged_count, 0);
        assert_eq!(status.unstaged_count, 1);
    }

    #[test]
    fn commits_staged_changes_without_committing_unstaged_changes() {
        let fixture = GitFixture::new();
        fixture.set_file("a.txt", "a\n");
        fixture.set_file("b.txt", "b\n");
        fixture.commit_all("initial");
        fixture.set_file("a.txt", "a changed\n");
        fixture.set_file("b.txt", "b changed\n");

        stage_file(&fixture.root, "a.txt").expect("stage file");
        commit_staged(&fixture.root, "commit a").expect("commit staged file");

        assert_eq!(
            git_output(&fixture.root, ["show", "--name-only", "--format=", "HEAD"]),
            "a.txt"
        );
        let status = inspect_status(&fixture.root, "main", 1).expect("inspect status");
        assert_eq!(status.staged_count, 0);
        assert_eq!(status.unstaged_count, 1);
    }

    #[test]
    fn pushes_current_branch_to_upstream() {
        let fixture = GitFixture::new();
        fixture.set_file("README.md", "initial\n");
        fixture.commit_all("initial");
        fixture.push_main();
        fixture.set_file("README.md", "changed\n");
        fixture.commit_all("change");

        let status = inspect_status(&fixture.root, "main", 1).expect("inspect status");
        assert_eq!(status.committed_ahead_count, 1);
        assert_eq!(status.push_target.as_deref(), Some("origin/main"));

        push_current_branch(&fixture.root).expect("push current branch");
        assert_eq!(
            git_output(&fixture.remote, ["rev-parse", "refs/heads/main"]),
            git_output(&fixture.root, ["rev-parse", "HEAD"])
        );
    }
}
