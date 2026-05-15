use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const COMMON_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
];
const SYSTEM_BIN_DIRS: &[&str] = &["/usr/bin", "/bin", "/usr/sbin", "/sbin"];
const NVM_NODE_VERSIONS_RELATIVE_PATH: &str = ".nvm/versions/node";
const NODE_HOSTED_TOOL_NAMES: &[&str] = &["codex", "copilot"];

#[derive(Clone, Copy)]
struct ToolBinarySpec<'a> {
    name: &'a str,
    env_vars: &'a [&'a str],
    well_known_paths: &'a [&'a str],
    home_relative_paths: &'a [&'a str],
}

pub fn repair_process_path_for_cli_tools() {
    let home_dir = dirs::home_dir();
    let Some(path) = augmented_path(env::var_os("PATH"), home_dir.as_deref(), |path| {
        path.is_dir()
    }) else {
        return;
    };

    env::set_var("PATH", path);
}

pub fn find_gh_binary() -> Option<String> {
    find_tool_binary(ToolBinarySpec {
        name: "gh",
        env_vars: &["REMISS_GH_BINARY", "GH_UI_TOOL_GH_BINARY"],
        well_known_paths: &["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"],
        home_relative_paths: &[],
    })
}

pub fn find_codex_binary() -> Option<String> {
    find_tool_binary(ToolBinarySpec {
        name: "codex",
        env_vars: &["REMISS_CODEX_BINARY", "GH_UI_TOOL_CODEX_BINARY"],
        well_known_paths: &[
            "/opt/homebrew/bin/codex",
            "/usr/local/bin/codex",
            "/usr/bin/codex",
        ],
        home_relative_paths: &[".codex/bin/codex", ".local/bin/codex", ".cargo/bin/codex"],
    })
}

pub fn find_copilot_binary() -> Option<String> {
    find_tool_binary(ToolBinarySpec {
        name: "copilot",
        env_vars: &["REMISS_COPILOT_BINARY", "GH_UI_TOOL_COPILOT_BINARY"],
        well_known_paths: &[
            "/opt/homebrew/bin/copilot",
            "/usr/local/bin/copilot",
            "/usr/bin/copilot",
        ],
        home_relative_paths: &[".local/bin/copilot", ".cargo/bin/copilot"],
    })
}

pub fn prepend_binary_parent_to_command_path(command: &mut Command, binary: &str) {
    let binary_path = Path::new(binary);
    if binary_path.components().count() <= 1 {
        return;
    }

    let Some(parent) = binary_path.parent() else {
        return;
    };
    let Some(path) = path_with_prepended_dir(env::var_os("PATH"), parent) else {
        return;
    };

    command.env("PATH", path);
}

fn find_tool_binary(spec: ToolBinarySpec<'_>) -> Option<String> {
    let home_dir = dirs::home_dir();
    find_tool_binary_from_env(
        spec,
        |key| env::var_os(key),
        home_dir.as_deref(),
        |path| path.is_file(),
    )
    .or_else(|| command_is_available(spec.name).then(|| spec.name.to_string()))
}

fn find_tool_binary_from_env(
    spec: ToolBinarySpec<'_>,
    env_value: impl Fn(&str) -> Option<OsString>,
    home_dir: Option<&Path>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<String> {
    for env_var in spec.env_vars {
        let Some(candidate) = env_path_value(&env_value, env_var) else {
            continue;
        };
        if is_file(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }

    if let Some(path_value) = env_value("PATH") {
        for directory in env::split_paths(&path_value) {
            let candidate = directory.join(spec.name);
            if is_file(&candidate) {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    for candidate in spec.well_known_paths {
        let candidate = Path::new(candidate);
        if is_file(candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }

    let Some(home_dir) = home_dir else {
        return None;
    };
    for relative_path in spec.home_relative_paths {
        let candidate = home_dir.join(relative_path);
        if is_file(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }

    if NODE_HOSTED_TOOL_NAMES.contains(&spec.name) {
        for directory in nvm_node_bin_dirs(home_dir) {
            let candidate = directory.join(spec.name);
            if is_file(&candidate) {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    None
}

fn env_path_value(env_value: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<PathBuf> {
    let value = env_value(key)?;
    let text = value.to_string_lossy();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn command_is_available(name: &str) -> bool {
    matches!(
        Command::new(name)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
        Ok(status) if status.success()
    )
}

fn augmented_path(
    current_path: Option<OsString>,
    home_dir: Option<&Path>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<OsString> {
    let mut segments = current_path
        .as_ref()
        .map(env::split_paths)
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();

    for directory in COMMON_BIN_DIRS {
        push_unique_dir(&mut segments, PathBuf::from(directory), &is_dir);
    }

    if let Some(home_dir) = home_dir {
        push_unique_dir(&mut segments, home_dir.join(".local/bin"), &is_dir);
        push_unique_dir(&mut segments, home_dir.join(".cargo/bin"), &is_dir);
        push_unique_dir(&mut segments, home_dir.join(".codex/bin"), &is_dir);
        for directory in nvm_node_bin_dirs(home_dir) {
            if nvm_bin_dir_has_node_hosted_tool(&directory) {
                push_unique_dir(&mut segments, directory, &is_dir);
            }
        }
    }

    for directory in SYSTEM_BIN_DIRS {
        push_unique_dir(&mut segments, PathBuf::from(directory), &is_dir);
    }

    env::join_paths(segments).ok()
}

fn push_unique_dir(
    segments: &mut Vec<PathBuf>,
    candidate: PathBuf,
    is_dir: &impl Fn(&Path) -> bool,
) {
    if candidate.as_os_str().is_empty()
        || !is_dir(&candidate)
        || segments.iter().any(|segment| segment == &candidate)
    {
        return;
    }

    segments.push(candidate);
}

fn nvm_node_bin_dirs(home_dir: &Path) -> Vec<PathBuf> {
    let versions_dir = home_dir.join(NVM_NODE_VERSIONS_RELATIVE_PATH);
    let Ok(entries) = fs::read_dir(versions_dir) else {
        return Vec::new();
    };

    let mut candidates = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let version_name = entry.file_name().to_string_lossy().into_owned();
            let bin_dir = entry.path().join("bin");
            bin_dir
                .is_dir()
                .then_some((parse_node_version(&version_name), version_name, bin_dir))
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    candidates
        .into_iter()
        .map(|(_, _, bin_dir)| bin_dir)
        .collect()
}

fn nvm_bin_dir_has_node_hosted_tool(directory: &Path) -> bool {
    NODE_HOSTED_TOOL_NAMES
        .iter()
        .any(|tool_name| directory.join(tool_name).is_file())
}

fn parse_node_version(name: &str) -> Option<(u64, u64, u64)> {
    let version = name.strip_prefix('v').unwrap_or(name);
    let mut parts = version.split('.');
    Some((
        parse_version_component(parts.next()?)?,
        parse_version_component(parts.next()?)?,
        parse_version_component(parts.next()?)?,
    ))
}

fn parse_version_component(component: &str) -> Option<u64> {
    let digits = component
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn path_with_prepended_dir(current_path: Option<OsString>, directory: &Path) -> Option<OsString> {
    if directory.as_os_str().is_empty() {
        return None;
    }

    let mut segments = vec![directory.to_path_buf()];
    if let Some(existing) = current_path {
        segments
            .extend(env::split_paths(&existing).filter(|segment| segment.as_path() != directory));
    }

    env::join_paths(segments).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_homebrew_gh_when_launch_path_is_minimal() {
        let spec = ToolBinarySpec {
            name: "gh",
            env_vars: &["REMISS_GH_BINARY"],
            well_known_paths: &["/opt/homebrew/bin/gh"],
            home_relative_paths: &[],
        };

        let result = find_tool_binary_from_env(
            spec,
            |key| match key {
                "PATH" => Some(OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")),
                _ => None,
            },
            None,
            |path| path == Path::new("/opt/homebrew/bin/gh"),
        );

        assert_eq!(result.as_deref(), Some("/opt/homebrew/bin/gh"));
    }

    #[test]
    fn explicit_env_override_wins_before_path() {
        let spec = ToolBinarySpec {
            name: "codex",
            env_vars: &["REMISS_CODEX_BINARY"],
            well_known_paths: &[],
            home_relative_paths: &[],
        };

        let result = find_tool_binary_from_env(
            spec,
            |key| match key {
                "REMISS_CODEX_BINARY" => Some(OsString::from("/custom/codex")),
                "PATH" => Some(OsString::from("/opt/homebrew/bin")),
                _ => None,
            },
            None,
            |path| {
                path == Path::new("/custom/codex") || path == Path::new("/opt/homebrew/bin/codex")
            },
        );

        assert_eq!(result.as_deref(), Some("/custom/codex"));
    }

    #[test]
    fn resolves_home_relative_agent_tool_candidate() {
        let spec = ToolBinarySpec {
            name: "codex",
            env_vars: &["REMISS_CODEX_BINARY"],
            well_known_paths: &[],
            home_relative_paths: &[".codex/bin/codex"],
        };

        let result = find_tool_binary_from_env(
            spec,
            |key| match key {
                "PATH" => Some(OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")),
                _ => None,
            },
            Some(Path::new("/Users/example")),
            |path| path == Path::new("/Users/example/.codex/bin/codex"),
        );

        assert_eq!(result.as_deref(), Some("/Users/example/.codex/bin/codex"));
    }

    #[test]
    fn augmented_path_adds_common_and_home_tool_directories_once() {
        let result = augmented_path(
            Some(OsString::from("/usr/bin:/bin")),
            Some(Path::new("/Users/example")),
            |path| {
                matches!(
                    path.to_string_lossy().as_ref(),
                    "/opt/homebrew/bin" | "/Users/example/.codex/bin" | "/usr/bin" | "/bin"
                )
            },
        )
        .expect("path should join");

        let segments = env::split_paths(&result).collect::<Vec<_>>();
        let result_text = result.to_string_lossy();
        assert!(result_text.contains("/opt/homebrew/bin"));
        assert!(result_text.contains("/Users/example/.codex/bin"));
        assert_eq!(
            segments
                .iter()
                .filter(|segment| segment.as_path() == Path::new("/usr/bin"))
                .count(),
            1
        );
        assert_eq!(
            segments
                .iter()
                .filter(|segment| segment.as_path() == Path::new("/bin"))
                .count(),
            1
        );
    }

    #[test]
    fn resolves_copilot_from_nvm_versioned_node_install() {
        let home_dir = unique_test_home("nvm-copilot");
        let copilot = home_dir.join(".nvm/versions/node/v22.17.0/bin/copilot");
        write_test_file(&copilot);

        let spec = ToolBinarySpec {
            name: "copilot",
            env_vars: &["REMISS_COPILOT_BINARY"],
            well_known_paths: &[],
            home_relative_paths: &[],
        };

        let result = find_tool_binary_from_env(
            spec,
            |key| match key {
                "PATH" => Some(OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")),
                _ => None,
            },
            Some(&home_dir),
            |path| path.is_file(),
        );

        assert_eq!(result.as_deref(), Some(copilot.to_string_lossy().as_ref()));
        let _ = fs::remove_dir_all(home_dir);
    }

    #[test]
    fn prefers_newest_nvm_node_version_with_requested_tool() {
        let home_dir = unique_test_home("nvm-newest-tool");
        let older = home_dir.join(".nvm/versions/node/v18.20.3/bin/copilot");
        let newer_without_tool = home_dir.join(".nvm/versions/node/v23.0.0/bin/node");
        let newer = home_dir.join(".nvm/versions/node/v22.17.0/bin/copilot");
        write_test_file(&older);
        write_test_file(&newer_without_tool);
        write_test_file(&newer);

        let spec = ToolBinarySpec {
            name: "copilot",
            env_vars: &[],
            well_known_paths: &[],
            home_relative_paths: &[],
        };

        let result =
            find_tool_binary_from_env(spec, |_| None, Some(&home_dir), |path| path.is_file());

        assert_eq!(result.as_deref(), Some(newer.to_string_lossy().as_ref()));
        let _ = fs::remove_dir_all(home_dir);
    }

    #[test]
    fn augmented_path_adds_nvm_dirs_that_host_agent_tools() {
        let home_dir = unique_test_home("nvm-path");
        let copilot_dir = home_dir.join(".nvm/versions/node/v22.17.0/bin");
        let node_only_dir = home_dir.join(".nvm/versions/node/v23.0.0/bin");
        write_test_file(&copilot_dir.join("copilot"));
        write_test_file(&node_only_dir.join("node"));

        let result = augmented_path(
            Some(OsString::from("/usr/bin:/bin")),
            Some(&home_dir),
            |path| path.is_dir(),
        )
        .expect("path should join");
        let segments = env::split_paths(&result).collect::<Vec<_>>();

        assert!(segments.iter().any(|segment| segment == &copilot_dir));
        assert!(!segments.iter().any(|segment| segment == &node_only_dir));
        let _ = fs::remove_dir_all(home_dir);
    }

    #[test]
    fn prepends_binary_parent_to_child_command_path() {
        let tool_dir = Path::new("/Users/example/.nvm/versions/node/v22.17.0/bin");
        let path =
            path_with_prepended_dir(Some(OsString::from("/usr/bin:/bin")), tool_dir).unwrap();
        let segments = env::split_paths(&path).collect::<Vec<_>>();

        assert_eq!(segments.first().map(PathBuf::as_path), Some(tool_dir));
        assert_eq!(
            segments
                .iter()
                .filter(|segment| segment.as_path() == tool_dir)
                .count(),
            1
        );
    }

    fn unique_test_home(label: &str) -> PathBuf {
        let home_dir =
            env::temp_dir().join(format!("gh-ui-cli-binary-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home_dir);
        home_dir
    }

    fn write_test_file(path: &Path) {
        fs::create_dir_all(path.parent().expect("test path should have parent")).unwrap();
        fs::write(path, b"test").unwrap();
    }
}
