use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::{json, Value};

use super::*;
use crate::app_storage;

pub(super) const REVIEW_PARTNER_CONTEXT_BUNDLE_VERSION: &str = "review-partner-context-bundle-v1";

const REVIEW_PARTNER_WORKSPACE_DIR: &str = "review-partner";
const CHECKOUT_LINK_NAME: &str = "checkout";

#[derive(Clone, Debug)]
pub(super) struct ReviewPartnerContextBundle {
    pub workspace_root: PathBuf,
    pub manifest_json: String,
}

impl ReviewPartnerContextBundle {
    pub(super) fn agent_working_directory(&self) -> &Path {
        &self.workspace_root
    }
}

pub(super) fn write_review_partner_context_bundle(
    input: &GenerateReviewPartnerInput,
) -> Result<ReviewPartnerContextBundle, String> {
    write_review_partner_context_bundle_at(
        input,
        &app_storage::agent_workspaces_root().join(REVIEW_PARTNER_WORKSPACE_DIR),
    )
}

pub(super) fn write_review_partner_context_bundle_at(
    input: &GenerateReviewPartnerInput,
    root: &Path,
) -> Result<ReviewPartnerContextBundle, String> {
    let data = ReviewPartnerBundleData {
        task: ReviewPartnerBundleTask::FullContext,
        provider: input.provider,
        checkout_root: Path::new(&input.working_directory),
        repository: &input.repository,
        number: input.number,
        code_version_key: &input.code_version_key,
        stack: &input.stack,
        structural_evidence: &input.structural_evidence,
        semantic_review: input.semantic_review.as_ref(),
        review_memory: &input.review_memory,
        context: &input.context,
        focus_targets: &input.focus_targets,
        target: None,
    };
    write_bundle(data, root)
}

pub(super) fn write_review_partner_focus_context_bundle(
    document: &GeneratedReviewPartnerContext,
    target: &ReviewPartnerFocusTarget,
    working_directory: &str,
) -> Result<ReviewPartnerContextBundle, String> {
    write_review_partner_focus_context_bundle_at(
        document,
        target,
        working_directory,
        &app_storage::agent_workspaces_root().join(REVIEW_PARTNER_WORKSPACE_DIR),
    )
}

pub(super) fn write_review_partner_focus_context_bundle_at(
    document: &GeneratedReviewPartnerContext,
    target: &ReviewPartnerFocusTarget,
    working_directory: &str,
    root: &Path,
) -> Result<ReviewPartnerContextBundle, String> {
    let data = ReviewPartnerBundleData {
        task: ReviewPartnerBundleTask::FocusRecord,
        provider: document.provider,
        checkout_root: Path::new(working_directory),
        repository: &document.stack.repository,
        number: document.stack.selected_pr_number,
        code_version_key: &document.code_version_key,
        stack: &document.stack,
        structural_evidence: &document.structural_evidence,
        semantic_review: document.semantic_review.as_ref(),
        review_memory: &document.review_memory,
        context: &document.context,
        focus_targets: &document.focus_targets,
        target: Some(target),
    };
    write_bundle(data, root)
}

#[derive(Clone, Copy, Debug)]
enum ReviewPartnerBundleTask {
    FullContext,
    FocusRecord,
}

impl ReviewPartnerBundleTask {
    fn label(self) -> &'static str {
        match self {
            Self::FullContext => "reviewPartnerContext",
            Self::FocusRecord => "reviewPartnerFocusRecord",
        }
    }
}

struct ReviewPartnerBundleData<'a> {
    task: ReviewPartnerBundleTask,
    provider: ReviewAiProvider,
    checkout_root: &'a Path,
    repository: &'a str,
    number: i64,
    code_version_key: &'a str,
    stack: &'a ReviewStack,
    structural_evidence: &'a StructuralEvidencePack,
    semantic_review: Option<&'a RemissSemanticReviewSummary>,
    review_memory: &'a ReviewMemoryPromptContext,
    context: &'a ReviewPartnerContextPack,
    focus_targets: &'a [ReviewPartnerFocusTarget],
    target: Option<&'a ReviewPartnerFocusTarget>,
}

fn write_bundle(
    data: ReviewPartnerBundleData<'_>,
    root: &Path,
) -> Result<ReviewPartnerContextBundle, String> {
    let workspace_root = root.join(bundle_workspace_name(&data));
    recreate_directory(&workspace_root)?;
    fs::create_dir_all(workspace_root.join("layers")).map_err(|error| {
        format!(
            "Failed to create Review Partner layer context folder '{}': {error}",
            workspace_root.join("layers").display()
        )
    })?;

    let mut warnings = Vec::new();
    let checkout_link_available =
        create_checkout_link(&workspace_root, data.checkout_root, &mut warnings);
    write_text_file(
        &workspace_root.join("checkout-path.txt"),
        &data.checkout_root.display().to_string(),
    )?;

    let layer_entries = write_layer_files(&workspace_root, &data)?;

    write_json_file(&workspace_root.join("stack.json"), data.stack)?;
    write_json_file(
        &workspace_root.join("focus-targets.json"),
        data.focus_targets,
    )?;
    write_json_file(
        &workspace_root.join("review-memory.json"),
        data.review_memory,
    )?;
    write_json_file(
        &workspace_root.join("structural-evidence.json"),
        data.structural_evidence,
    )?;
    write_json_file(
        &workspace_root.join("semantic-evidence.json"),
        &data.semantic_review,
    )?;
    write_json_file(&workspace_root.join("collected-context.json"), data.context)?;
    if let Some(target) = data.target {
        write_json_file(&workspace_root.join("target.json"), target)?;
    }

    let focus_target_keys = data
        .focus_targets
        .iter()
        .map(|target| target.key.as_str())
        .collect::<Vec<_>>();
    let manifest = json!({
        "bundleVersion": REVIEW_PARTNER_CONTEXT_BUNDLE_VERSION,
        "task": data.task.label(),
        "provider": data.provider.slug(),
        "repository": data.repository,
        "pullRequestNumber": data.number,
        "codeVersionKey": data.code_version_key,
        "generatorVersion": REVIEW_PARTNER_GENERATOR_VERSION,
        "contextVersion": REVIEW_PARTNER_CONTEXT_VERSION,
        "stackGeneratorVersion": data.stack.generator_version,
        "checkout": {
            "path": CHECKOUT_LINK_NAME,
            "absolutePath": data.checkout_root.display().to_string(),
            "linkAvailable": checkout_link_available,
            "pathFile": "checkout-path.txt",
        },
        "files": {
            "stack": "stack.json",
            "focusTargets": "focus-targets.json",
            "reviewMemory": "review-memory.json",
            "structuralEvidence": "structural-evidence.json",
            "semanticEvidence": "semantic-evidence.json",
            "collectedContext": "collected-context.json",
            "target": data.target.map(|_| "target.json"),
        },
        "focusTargetKeys": focus_target_keys,
        "target": data.target.map(summarize_focus_target),
        "layers": layer_entries,
        "warnings": warnings,
    });
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .expect("Review Partner context manifest must serialize");
    write_text_file(&workspace_root.join("manifest.json"), &manifest_json)?;

    Ok(ReviewPartnerContextBundle {
        workspace_root,
        manifest_json,
    })
}

fn write_layer_files(
    workspace_root: &Path,
    data: &ReviewPartnerBundleData<'_>,
) -> Result<Vec<Value>, String> {
    data.stack
        .layers
        .iter()
        .take(MAX_PARTNER_LAYERS)
        .map(|layer| {
            let relative_path = layer_context_relative_path(layer);
            let layer_path = workspace_root.join(&relative_path);
            let atoms = layer
                .atom_ids
                .iter()
                .filter_map(|atom_id| data.stack.atom(atom_id))
                .collect::<Vec<_>>();
            let focus_targets = data
                .focus_targets
                .iter()
                .filter(|target| target.layer_id.as_deref() == Some(layer.id.as_str()))
                .collect::<Vec<_>>();
            let layer_context = json!({
                "layer": layer,
                "atoms": atoms,
                "focusTargets": focus_targets,
                "collectedContext": data.context.layer(&layer.id),
            });
            write_json_file(&layer_path, &layer_context)?;
            Ok(json!({
                "layerId": layer.id,
                "index": layer.index,
                "title": layer.title,
                "path": relative_path,
                "focusTargetKeys": focus_targets
                    .iter()
                    .map(|target| target.key.as_str())
                    .collect::<Vec<_>>(),
            }))
        })
        .collect()
}

fn bundle_workspace_name(data: &ReviewPartnerBundleData<'_>) -> String {
    let raw_key = format!(
        "{}:{}:{}:{}:{}:{}:{}:{}",
        data.task.label(),
        data.provider.slug(),
        data.repository,
        data.number,
        data.code_version_key,
        REVIEW_PARTNER_GENERATOR_VERSION,
        REVIEW_PARTNER_CONTEXT_VERSION,
        data.target.map(|target| target.key.as_str()).unwrap_or("")
    );
    format!(
        "{}-{}-{}",
        safe_path_component(data.repository, "repository"),
        data.number,
        short_hash(&raw_key)
    )
}

fn layer_context_relative_path(layer: &ReviewStackLayer) -> String {
    format!(
        "layers/{:02}-{}-{}.json",
        layer.index,
        safe_path_component(&layer.title, "layer"),
        short_hash(&layer.id)
    )
}

fn safe_path_component(value: &str, fallback: &str) -> String {
    let mut output = String::new();
    let mut last_was_dash = false;
    for ch in value.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if matches!(ch, '-' | '_' | '.') {
            ch
        } else {
            '-'
        };
        if mapped == '-' {
            if last_was_dash {
                continue;
            }
            last_was_dash = true;
        } else {
            last_was_dash = false;
        }
        output.push(mapped);
        if output.len() >= 72 {
            break;
        }
    }
    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        fallback.to_string()
    } else {
        output
    }
}

fn recreate_directory(path: &Path) -> Result<(), String> {
    if path.exists() {
        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|error| {
                format!(
                    "Failed to remove stale Review Partner context workspace '{}': {error}",
                    path.display()
                )
            })?;
        } else {
            fs::remove_file(path).map_err(|error| {
                format!(
                    "Failed to remove stale Review Partner context workspace '{}': {error}",
                    path.display()
                )
            })?;
        }
    }
    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Failed to create Review Partner context workspace '{}': {error}",
            path.display()
        )
    })
}

fn write_json_file<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize '{}': {error}", path.display()))?;
    write_text_file(path, &text)
}

fn write_text_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create Review Partner context folder '{}': {error}",
                parent.display()
            )
        })?;
    }
    fs::write(path, text).map_err(|error| format!("Failed to write '{}': {error}", path.display()))
}

fn create_checkout_link(
    workspace_root: &Path,
    checkout_root: &Path,
    warnings: &mut Vec<String>,
) -> bool {
    let link_path = workspace_root.join(CHECKOUT_LINK_NAME);
    let target = checkout_root
        .canonicalize()
        .unwrap_or_else(|_| checkout_root.to_path_buf());
    match create_dir_symlink(&target, &link_path) {
        Ok(()) => true,
        Err(error) => {
            warnings.push(format!(
                "Could not create checkout symlink '{}': {error}. Use checkout.absolutePath if the provider cannot inspect checkout/.",
                link_path.display()
            ));
            false
        }
    }
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}
