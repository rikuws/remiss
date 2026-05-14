use std::{
    env,
    ffi::OsString,
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
}
