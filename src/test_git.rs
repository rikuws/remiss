use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

pub fn unique_test_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos}-{test_id}-{}", std::process::id())
}

pub fn unique_test_directory(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("remiss-{}", unique_test_name(prefix)));
    fs::create_dir_all(&path).expect("failed to create temp directory");
    path
}

pub fn run_git<I, S>(path: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(&args)
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

pub fn git_output<I, S>(path: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(&args)
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
