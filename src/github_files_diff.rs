use serde::Deserialize;

use crate::{diff::ParsedDiffFile, gh};

#[derive(Debug, Clone)]
pub struct PullRequestFilesDiffFetch {
    pub raw_diff: String,
    pub parsed_diff: Vec<ParsedDiffFile>,
    pub loaded_files: usize,
}

#[derive(Debug, Deserialize)]
struct RestPullRequestFile {
    filename: String,
    status: String,
    #[serde(default)]
    patch: Option<String>,
    #[serde(default)]
    previous_filename: Option<String>,
}

pub fn fetch_pull_request_diff_from_files_endpoint(
    repository: &str,
    number: i64,
) -> Result<PullRequestFilesDiffFetch, String> {
    let (owner, name) = split_repository(repository)?;
    let mut files = Vec::<RestPullRequestFile>::new();

    for page in 1..=100 {
        let endpoint = format!(
            "repos/{}/{}/pulls/{number}/files?per_page=100&page={page}",
            encode_uri_component(owner),
            encode_uri_component(name)
        );
        let value = gh::run_json_owned(vec!["api".to_string(), endpoint])?;
        let mut page_files = serde_json::from_value::<Vec<RestPullRequestFile>>(value)
            .map_err(|error| format!("Failed to parse pull request files response: {error}"))?;
        let page_len = page_files.len();
        files.append(&mut page_files);
        if page_len < 100 {
            break;
        }
    }

    let raw_diff = synthesize_unified_diff_from_rest_files(&files);
    let parsed_diff = crate::diff::parse_unified_diff(&raw_diff);
    Ok(PullRequestFilesDiffFetch {
        raw_diff,
        parsed_diff,
        loaded_files: files.len(),
    })
}

fn synthesize_unified_diff_from_rest_files(files: &[RestPullRequestFile]) -> String {
    let mut raw = String::new();
    for file in files {
        let Some(diff) = synthesize_unified_diff_for_rest_file(file) else {
            continue;
        };
        if !raw.is_empty() && !raw.ends_with('\n') {
            raw.push('\n');
        }
        raw.push_str(&diff);
        if !raw.ends_with('\n') {
            raw.push('\n');
        }
    }
    raw
}

fn synthesize_unified_diff_for_rest_file(file: &RestPullRequestFile) -> Option<String> {
    let path = file.filename.trim();
    if path.is_empty() {
        return None;
    }

    let previous_path = file
        .previous_filename
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(path);
    let status = file.status.as_str();
    let old_path = if status == "added" {
        "/dev/null".to_string()
    } else {
        format!("a/{previous_path}")
    };
    let new_path = if status == "removed" {
        "/dev/null".to_string()
    } else {
        format!("b/{path}")
    };

    let mut diff = String::new();
    diff.push_str(&format!("diff --git a/{previous_path} b/{path}\n"));
    if status == "renamed" && previous_path != path {
        diff.push_str(&format!("rename from {previous_path}\n"));
        diff.push_str(&format!("rename to {path}\n"));
    }
    diff.push_str(&format!("--- {old_path}\n"));
    diff.push_str(&format!("+++ {new_path}\n"));

    if let Some(patch) = file
        .patch
        .as_deref()
        .filter(|patch| !patch.trim().is_empty())
    {
        diff.push_str(patch.trim_end());
        diff.push('\n');
    }

    Some(diff)
}

fn split_repository(repository: &str) -> Result<(&str, &str), String> {
    repository
        .split_once('/')
        .ok_or_else(|| format!("Invalid repository name '{repository}'. Expected owner/name."))
}

fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesizes_parseable_unified_diff_from_pull_request_files() {
        let files = vec![
            RestPullRequestFile {
                filename: "src/lib.rs".to_string(),
                status: "modified".to_string(),
                patch: Some(
                    "@@ -1,2 +1,2 @@\n pub fn answer() -> i32 {\n-    41\n+    42".to_string(),
                ),
                previous_filename: None,
            },
            RestPullRequestFile {
                filename: "src/new.rs".to_string(),
                status: "added".to_string(),
                patch: Some("@@ -0,0 +1 @@\n+pub fn new() {}".to_string()),
                previous_filename: None,
            },
            RestPullRequestFile {
                filename: "src/new_name.rs".to_string(),
                status: "renamed".to_string(),
                patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
                previous_filename: Some("src/old_name.rs".to_string()),
            },
        ];

        let raw = synthesize_unified_diff_from_rest_files(&files);
        let parsed = crate::diff::parse_unified_diff(&raw);

        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].path, "src/lib.rs");
        assert_eq!(parsed[0].previous_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(parsed[0].hunks[0].lines.len(), 3);
        assert_eq!(parsed[1].path, "src/new.rs");
        assert_eq!(parsed[1].previous_path.as_deref(), None);
        assert_eq!(parsed[2].path, "src/new_name.rs");
        assert_eq!(parsed[2].previous_path.as_deref(), Some("src/old_name.rs"));
    }
}
