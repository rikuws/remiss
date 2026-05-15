use std::path::Path;

use crate::{
    cache::CacheStore,
    diff::ParsedDiffFile,
    difftastic::{
        adapt_difftastic_file, run_difftastic_for_texts, AdaptedDifftasticDiffFile,
        DifftasticAdaptOptions,
    },
    github::{PullRequestDetail, PullRequestFile},
    local_documents, local_repo,
    structural_diff_cache::{
        save_cached_structural_diff, structural_diff_cache_key, CachedStructuralDiffResult,
    },
};

#[derive(Clone)]
pub struct StructuralDiffRequest {
    pub path: String,
    pub previous_path: Option<String>,
    pub old_side: StructuralDiffSideRequest,
    pub new_side: StructuralDiffSideRequest,
    pub request_key: String,
    pub cache_key: String,
}

#[derive(Clone)]
pub struct StructuralDiffSideRequest {
    pub path: String,
    pub reference: String,
    pub fetch: bool,
    pub prefer_worktree: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralDiffTerminalStatus {
    Ready,
    Error,
}

#[derive(Clone, Debug)]
pub enum StructuralDiffBuildResult {
    Ready(AdaptedDifftasticDiffFile),
    TerminalError(String),
    TransientError(String),
}

impl StructuralDiffBuildResult {
    pub fn cached_result(&self) -> Option<CachedStructuralDiffResult> {
        match self {
            StructuralDiffBuildResult::Ready(diff) => {
                Some(CachedStructuralDiffResult::Ready { diff: diff.clone() })
            }
            StructuralDiffBuildResult::TerminalError(message) => {
                Some(CachedStructuralDiffResult::TerminalError {
                    message: message.clone(),
                })
            }
            StructuralDiffBuildResult::TransientError(_) => None,
        }
    }
}

#[derive(Debug)]
enum StructuralDiffBuildError {
    Terminal(String),
    Transient(String),
}

pub fn build_structural_diff_request(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    head_oid: &str,
) -> Option<StructuralDiffRequest> {
    if file.path.is_empty() {
        return None;
    }

    let base_reference = detail.base_ref_oid.clone()?;
    let head_reference = head_oid.trim().to_string();
    if base_reference.is_empty() || head_reference.is_empty() {
        return None;
    }

    let previous_path = parsed
        .and_then(|parsed| parsed.previous_path.clone())
        .filter(|path| !path.is_empty());
    let old_path = previous_path.clone().unwrap_or_else(|| file.path.clone());
    let old_fetch = file.change_type != "ADDED";
    let new_fetch = file.change_type != "DELETED";
    let is_local_review = crate::local_review::is_local_review_detail(detail);
    let cache_head_reference = if is_local_review {
        detail.id.clone()
    } else {
        head_reference.clone()
    };
    let cache_key = structural_diff_cache_key(
        detail,
        &cache_head_reference,
        file,
        previous_path.as_deref(),
    );

    Some(StructuralDiffRequest {
        path: file.path.clone(),
        previous_path,
        old_side: StructuralDiffSideRequest {
            path: old_path,
            reference: base_reference,
            fetch: old_fetch,
            prefer_worktree: false,
        },
        new_side: StructuralDiffSideRequest {
            path: file.path.clone(),
            reference: head_reference,
            fetch: new_fetch,
            prefer_worktree: is_local_review && new_fetch,
        },
        request_key: cache_key.clone(),
        cache_key,
    })
}

pub fn checkout_head_oid(status: &local_repo::LocalRepositoryStatus) -> Option<String> {
    status
        .ready_for_snapshot_features()
        .then(|| status.current_head_oid.as_deref())
        .flatten()
        .map(str::trim)
        .filter(|head| !head.is_empty())
        .map(str::to_string)
}

pub fn structural_diff_warmup_request_key(detail: &PullRequestDetail, head_oid: &str) -> String {
    let identity = if crate::local_review::is_local_review_detail(detail) {
        detail.id.as_str()
    } else {
        head_oid
    };
    format!(
        "structural-diff-warmup-v1:{}:{}:{}",
        detail.repository, detail.number, identity
    )
}

pub fn structural_result_from_cached(
    cached: CachedStructuralDiffResult,
) -> StructuralDiffBuildResult {
    match cached {
        CachedStructuralDiffResult::Ready { diff } => StructuralDiffBuildResult::Ready(diff),
        CachedStructuralDiffResult::TerminalError { message } => {
            StructuralDiffBuildResult::TerminalError(message)
        }
    }
}

pub fn build_and_cache_structural_diff(
    cache: &CacheStore,
    repository: &str,
    checkout_root: &Path,
    request: &StructuralDiffRequest,
) -> StructuralDiffBuildResult {
    let result = build_structural_diff_from_local(cache, repository, checkout_root, request);
    if let Some(cached) = result.cached_result() {
        let _ = save_cached_structural_diff(cache, &request.cache_key, &cached);
    }
    result
}

fn build_structural_diff_from_local(
    cache: &CacheStore,
    repository: &str,
    checkout_root: &Path,
    request: &StructuralDiffRequest,
) -> StructuralDiffBuildResult {
    let old_text =
        match load_structural_side_text(cache, repository, checkout_root, &request.old_side) {
            Ok(text) => text,
            Err(StructuralDiffBuildError::Terminal(error)) => {
                return StructuralDiffBuildResult::TerminalError(error);
            }
            Err(StructuralDiffBuildError::Transient(error)) => {
                return StructuralDiffBuildResult::TransientError(error);
            }
        };
    let new_text =
        match load_structural_side_text(cache, repository, checkout_root, &request.new_side) {
            Ok(text) => text,
            Err(StructuralDiffBuildError::Terminal(error)) => {
                return StructuralDiffBuildResult::TerminalError(error);
            }
            Err(StructuralDiffBuildError::Transient(error)) => {
                return StructuralDiffBuildResult::TransientError(error);
            }
        };
    let file = match run_difftastic_for_texts(
        request.old_side.path.as_str(),
        old_text.as_str(),
        request.new_side.path.as_str(),
        new_text.as_str(),
    ) {
        Ok(file) => file,
        Err(error) => return StructuralDiffBuildResult::TerminalError(error),
    };

    StructuralDiffBuildResult::Ready(adapt_difftastic_file(
        &file,
        old_text.as_str(),
        new_text.as_str(),
        request.path.clone(),
        request.previous_path.clone(),
        &DifftasticAdaptOptions { context_lines: 3 },
    ))
}

fn load_structural_side_text(
    cache: &CacheStore,
    repository: &str,
    checkout_root: &Path,
    side: &StructuralDiffSideRequest,
) -> Result<String, StructuralDiffBuildError> {
    if !side.fetch {
        return Ok(String::new());
    }

    let document = local_documents::load_local_repository_file_content(
        cache,
        repository,
        checkout_root,
        &side.reference,
        &side.path,
        side.prefer_worktree,
    )
    .map_err(StructuralDiffBuildError::Transient)?;
    if document.is_binary {
        return Err(StructuralDiffBuildError::Terminal(format!(
            "Structural diff is not available for binary file {}.",
            side.path
        )));
    }

    Ok(document.content.unwrap_or_default())
}
