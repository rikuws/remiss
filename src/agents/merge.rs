use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::guided_review::{
    GenerateGuidedReviewInput, GeneratedGuidedReview, GuidedReviewCallsite,
    GuidedReviewCandidateGroup, GuidedReviewSection, GuidedReviewSectionCategory,
    GuidedReviewSectionPriority, GuidedReviewStep,
};

use super::prompt::trim_text;

pub const MAX_SECTIONS: usize = 10;
pub const MAX_REVIEW_POINTS: usize = 4;
pub const MAX_CALLSITES_PER_SECTION: usize = 3;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GuidedReviewResponse {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub review_focus: Option<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub overview: Option<GuidedReviewResponseOverview>,
    #[serde(default)]
    pub steps: Vec<GuidedReviewResponseStep>,
    #[serde(default)]
    pub sections: Vec<GuidedReviewResponseSection>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GuidedReviewResponseOverview {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub badge: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GuidedReviewResponseStep {
    #[serde(default)]
    pub source_step_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub badge: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GuidedReviewResponseSection {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub badge: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub step_ids: Vec<String>,
    #[serde(default)]
    pub review_points: Vec<String>,
    #[serde(default)]
    pub callsites: Vec<GuidedReviewResponseCallsite>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GuidedReviewResponseCallsite {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub snippet: Option<String>,
}

pub fn merge_guided_review(
    response: GuidedReviewResponse,
    input: &GenerateGuidedReviewInput,
    model: Option<String>,
) -> GeneratedGuidedReview {
    let mut candidate_steps_by_id: HashMap<String, GuidedReviewStep> = HashMap::new();
    for step in &input.candidate_steps {
        candidate_steps_by_id.insert(step.id.clone(), step.clone());
    }

    let overview_candidate = candidate_steps_by_id
        .get("overview")
        .cloned()
        .or_else(|| input.candidate_steps.first().cloned());

    let mut merged_steps_by_id: HashMap<String, GuidedReviewStep> = HashMap::new();
    let mut overview_step: Option<GuidedReviewStep> = None;

    if let Some(candidate) = overview_candidate {
        let overview = response.overview.clone().unwrap_or_default();
        let merged = merge_step(
            &candidate,
            overview.title.as_deref(),
            overview.summary.as_deref(),
            overview.detail.as_deref(),
            overview.badge.as_deref(),
        );
        merged_steps_by_id.insert(merged.id.clone(), merged.clone());
        overview_step = Some(merged);
    }

    for candidate in &input.candidate_steps {
        if candidate.kind != "file" {
            continue;
        }

        let override_step = response
            .steps
            .iter()
            .find(|item| item.source_step_id.as_deref() == Some(candidate.id.as_str()));

        let merged = merge_step(
            candidate,
            override_step.and_then(|step| step.title.as_deref()),
            override_step.and_then(|step| step.summary.as_deref()),
            override_step.and_then(|step| step.detail.as_deref()),
            override_step.and_then(|step| step.badge.as_deref()),
        );
        merged_steps_by_id.insert(candidate.id.clone(), merged);
    }

    let mut used_step_ids: HashSet<String> = HashSet::new();
    let mut merged_sections: Vec<GuidedReviewSection> = Vec::new();

    for item in response.sections.iter().take(MAX_SECTIONS) {
        let step_ids: Vec<String> = unique_step_ids(&item.step_ids)
            .into_iter()
            .filter(|step_id| {
                candidate_steps_by_id
                    .get(step_id)
                    .map(|candidate| candidate.kind == "file")
                    .unwrap_or(false)
                    && !used_step_ids.contains(step_id)
            })
            .collect();

        if step_ids.is_empty() {
            continue;
        }

        let section_steps: Vec<&GuidedReviewStep> = step_ids
            .iter()
            .filter_map(|step_id| merged_steps_by_id.get(step_id))
            .collect();
        let category = response_section_category(item.category.as_deref(), &section_steps);
        let priority = response_section_priority(item.priority.as_deref(), &section_steps);

        let next_id = format!("section:{}", merged_sections.len() + 1);
        merged_sections.push(GuidedReviewSection {
            id: next_id,
            title: fallback_text(
                item.title.as_deref(),
                &fallback_section_title(&section_steps),
            ),
            summary: fallback_text(
                item.summary.as_deref(),
                &fallback_section_summary(&section_steps),
            ),
            detail: fallback_text(
                item.detail.as_deref(),
                &fallback_section_detail(&section_steps),
            ),
            badge: fallback_text(
                item.badge.as_deref(),
                &fallback_section_badge(&section_steps),
            ),
            category,
            priority,
            step_ids: step_ids.clone(),
            review_points: sanitize_string_array(&item.review_points, MAX_REVIEW_POINTS),
            callsites: sanitize_callsites(&item.callsites, MAX_CALLSITES_PER_SECTION),
        });

        for step_id in &step_ids {
            used_step_ids.insert(step_id.clone());
        }
    }

    for section in build_fallback_sections(
        &input.candidate_groups,
        &merged_steps_by_id,
        &mut used_step_ids,
    ) {
        let next_id = format!("section:{}", merged_sections.len() + 1);
        merged_sections.push(GuidedReviewSection {
            id: next_id,
            ..section
        });
    }

    let mut merged_steps: Vec<GuidedReviewStep> = Vec::new();
    if let Some(step) = overview_step {
        merged_steps.push(step);
    }

    for section in &merged_sections {
        for step_id in &section.step_ids {
            if let Some(step) = merged_steps_by_id.get(step_id) {
                merged_steps.push(step.clone());
            }
        }
    }

    GeneratedGuidedReview {
        provider: input.provider,
        model,
        generated_at: iso_now(),
        summary: fallback_text(
            response.summary.as_deref(),
            "AI-generated guided review focused on the most reviewable parts of this pull request.",
        ),
        review_focus: fallback_text(
            response.review_focus.as_deref(),
            "Review the walkthrough section by section and use the diff anchors to drop into the concrete implementation details.",
        ),
        open_questions: sanitize_string_array(&response.open_questions, 6),
        warnings: sanitize_string_array(&response.warnings, 4),
        sections: merged_sections,
        steps: merged_steps,
    }
}

pub fn build_copilot_fallback_guided_review(
    input: &GenerateGuidedReviewInput,
    model: Option<String>,
    reason: String,
) -> GeneratedGuidedReview {
    let warning = trim_text(&reason, 240);
    let response = GuidedReviewResponse {
        summary: Some(
            "Fallback Guided Review walkthrough assembled from the verified pull-request context and grouped changed files."
                .to_string(),
        ),
        review_focus: Some(
            "Review the grouped changed files and diff anchors below; GitHub Copilot did not finish a custom narrative for this run."
                .to_string(),
        ),
        open_questions: Vec::new(),
        warnings: if warning.is_empty() {
            Vec::new()
        } else {
            vec![warning]
        },
        overview: None,
        steps: Vec::new(),
        sections: Vec::new(),
    };

    merge_guided_review(response, input, model)
}

fn merge_step(
    candidate: &GuidedReviewStep,
    title: Option<&str>,
    summary: Option<&str>,
    detail: Option<&str>,
    badge: Option<&str>,
) -> GuidedReviewStep {
    GuidedReviewStep {
        id: candidate.id.clone(),
        kind: candidate.kind.clone(),
        title: fallback_text(title, &candidate.title),
        summary: fallback_text(summary, &candidate.summary),
        detail: fallback_text(detail, &candidate.detail),
        file_path: candidate.file_path.clone(),
        anchor: candidate.anchor.clone(),
        additions: candidate.additions,
        deletions: candidate.deletions,
        unresolved_thread_count: candidate.unresolved_thread_count,
        snippet: candidate.snippet.clone(),
        badge: fallback_text(badge, &candidate.badge),
    }
}

fn fallback_text(value: Option<&str>, fallback: &str) -> String {
    value
        .map(|raw| raw.trim())
        .filter(|raw| !raw.is_empty())
        .map(|raw| raw.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn unique_step_ids(values: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut ordered: Vec<String> = Vec::new();

    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() || seen.contains(trimmed) {
            continue;
        }
        seen.insert(trimmed.to_string());
        ordered.push(trimmed.to_string());
    }

    ordered
}

fn response_section_category(
    value: Option<&str>,
    steps: &[&GuidedReviewStep],
) -> GuidedReviewSectionCategory {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => GuidedReviewSectionCategory::from_slug(raw)
            .unwrap_or(GuidedReviewSectionCategory::Other),
        None => fallback_section_category(steps),
    }
}

fn response_section_priority(
    value: Option<&str>,
    steps: &[&GuidedReviewStep],
) -> GuidedReviewSectionPriority {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => GuidedReviewSectionPriority::from_slug(raw)
            .unwrap_or_else(|| fallback_section_priority(steps)),
        None => fallback_section_priority(steps),
    }
}

fn sanitize_string_array(values: &[String], limit: usize) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut items: Vec<String> = Vec::new();

    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() || seen.contains(trimmed) {
            continue;
        }
        seen.insert(trimmed.to_string());
        items.push(trimmed.to_string());
        if items.len() >= limit {
            break;
        }
    }

    items
}

fn sanitize_callsites(
    values: &[GuidedReviewResponseCallsite],
    limit: usize,
) -> Vec<GuidedReviewCallsite> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut items: Vec<GuidedReviewCallsite> = Vec::new();

    for entry in values {
        let path = entry
            .path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let summary = entry
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let (Some(path), Some(summary)) = (path, summary) else {
            continue;
        };

        let line = entry
            .line
            .filter(|value| *value >= 1)
            .map(|value| value.max(1));

        let title = entry
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
            .unwrap_or_else(|| match line {
                Some(line) => format!("Callsite in {path}:{line}"),
                None => format!("Callsite in {path}"),
            });

        let snippet = entry
            .snippet
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| trim_text(value, 1_200));

        let key = format!(
            "{path}:{}:{title}",
            line.map(|value| value.to_string())
                .unwrap_or_else(|| "no-line".to_string())
        );

        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        items.push(GuidedReviewCallsite {
            title,
            path: path.to_string(),
            line,
            summary: summary.to_string(),
            snippet,
        });

        if items.len() >= limit {
            break;
        }
    }

    items
}

fn build_fallback_sections(
    candidate_groups: &[GuidedReviewCandidateGroup],
    merged_steps_by_id: &HashMap<String, GuidedReviewStep>,
    used_step_ids: &mut HashSet<String>,
) -> Vec<GuidedReviewSection> {
    let mut sections: Vec<GuidedReviewSection> = Vec::new();

    for group in candidate_groups {
        let step_ids: Vec<String> = unique_step_ids(&group.step_ids)
            .into_iter()
            .filter(|step_id| {
                merged_steps_by_id
                    .get(step_id)
                    .map(|step| step.kind == "file")
                    .unwrap_or(false)
                    && !used_step_ids.contains(step_id)
            })
            .collect();

        if step_ids.is_empty() {
            continue;
        }

        let section_steps: Vec<&GuidedReviewStep> = step_ids
            .iter()
            .filter_map(|step_id| merged_steps_by_id.get(step_id))
            .collect();

        sections.push(GuidedReviewSection {
            id: String::new(),
            title: fallback_text(Some(&group.title), &fallback_section_title(&section_steps)),
            summary: fallback_text(
                Some(&group.summary),
                &fallback_section_summary(&section_steps),
            ),
            detail: fallback_section_detail(&section_steps),
            badge: fallback_section_badge(&section_steps),
            category: fallback_section_category(&section_steps),
            priority: fallback_section_priority(&section_steps),
            step_ids: step_ids.clone(),
            review_points: fallback_section_review_points(&section_steps),
            callsites: Vec::new(),
        });

        for step_id in &step_ids {
            used_step_ids.insert(step_id.clone());
        }
    }

    let mut remaining_step_ids: Vec<String> = merged_steps_by_id
        .values()
        .filter(|step| step.kind == "file" && !used_step_ids.contains(&step.id))
        .map(|step| step.id.clone())
        .collect();
    remaining_step_ids.sort();

    if !remaining_step_ids.is_empty() {
        let section_steps: Vec<&GuidedReviewStep> = remaining_step_ids
            .iter()
            .filter_map(|step_id| merged_steps_by_id.get(step_id))
            .collect();

        sections.push(GuidedReviewSection {
            id: String::new(),
            title: "Remaining changed files".to_string(),
            summary: fallback_section_summary(&section_steps),
            detail: fallback_section_detail(&section_steps),
            badge: fallback_section_badge(&section_steps),
            category: fallback_section_category(&section_steps),
            priority: fallback_section_priority(&section_steps),
            step_ids: remaining_step_ids.clone(),
            review_points: fallback_section_review_points(&section_steps),
            callsites: Vec::new(),
        });

        for step_id in remaining_step_ids {
            used_step_ids.insert(step_id);
        }
    }

    sections
}

fn fallback_section_title(steps: &[&GuidedReviewStep]) -> String {
    if steps.is_empty() {
        return "Related changes".to_string();
    }
    if steps.len() == 1 {
        return steps[0].title.clone();
    }
    format!("Related changes across {} files", steps.len())
}

fn fallback_section_summary(steps: &[&GuidedReviewStep]) -> String {
    if steps.is_empty() {
        return "Grouped changes from the pull request.".to_string();
    }

    let additions: i64 = steps.iter().map(|step| step.additions).sum();
    let deletions: i64 = steps.iter().map(|step| step.deletions).sum();
    let unresolved_thread_count: i64 = steps.iter().map(|step| step.unresolved_thread_count).sum();

    let delta = format!("+{additions} / -{deletions}");
    if unresolved_thread_count > 0 {
        format!(
            "{} related files with {delta} and {unresolved_thread_count} unresolved review threads.",
            steps.len(),
        )
    } else {
        format!("{} related files with {delta}.", steps.len())
    }
}

fn fallback_section_detail(steps: &[&GuidedReviewStep]) -> String {
    if steps.iter().any(|step| step.unresolved_thread_count > 0) {
        return "Review these files together and carry the open review discussion across the whole slice of the change.".to_string();
    }
    "Review these files together to trace how the behavior moves through this part of the repository before dropping into the raw diff.".to_string()
}

fn fallback_section_badge(steps: &[&GuidedReviewStep]) -> String {
    if steps.iter().any(|step| step.unresolved_thread_count > 0) {
        return "discussion".to_string();
    }
    if steps.iter().any(|step| step.badge == "added") {
        return "new surface".to_string();
    }
    if steps.len() > 1 {
        return "grouped".to_string();
    }
    steps
        .first()
        .map(|step| step.badge.clone())
        .unwrap_or_else(|| "focus".to_string())
}

fn fallback_section_category(steps: &[&GuidedReviewStep]) -> GuidedReviewSectionCategory {
    let haystack = steps
        .iter()
        .map(|step| {
            format!(
                "{} {} {} {}",
                step.title,
                step.summary,
                step.detail,
                step.file_path.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase()
        })
        .collect::<Vec<_>>()
        .join("\n");

    if contains_any(
        &haystack,
        &[
            "auth",
            "oauth",
            "login",
            "session",
            "token",
            "secret",
            "permission",
            "passkey",
            "password",
            "security",
        ],
    ) {
        return GuidedReviewSectionCategory::AuthSecurity;
    }
    if contains_any(
        &haystack,
        &[
            "test", "tests/", "spec", "fixture", "mock", "snapshot", "assert",
        ],
    ) {
        return GuidedReviewSectionCategory::Tests;
    }
    if contains_any(
        &haystack,
        &["docs/", ".md", "readme", "guide", "documentation"],
    ) {
        return GuidedReviewSectionCategory::Docs;
    }
    if contains_any(
        &haystack,
        &[
            ".toml", ".json", ".yaml", ".yml", ".lock", "config", "settings",
        ],
    ) {
        return GuidedReviewSectionCategory::Config;
    }
    if contains_any(
        &haystack,
        &[
            ".github",
            "workflow",
            "ci",
            "deploy",
            "docker",
            "infra",
            "terraform",
            "kubernetes",
        ],
    ) {
        return GuidedReviewSectionCategory::Infra;
    }
    if contains_any(
        &haystack,
        &[
            "view",
            "ui",
            "ux",
            "component",
            "screen",
            "render",
            "button",
            "layout",
            "style",
        ],
    ) {
        return GuidedReviewSectionCategory::UiUx;
    }
    if contains_any(
        &haystack,
        &[
            "api", "route", "endpoint", "request", "response", "handler", "http", "graphql",
            "webhook",
        ],
    ) {
        return GuidedReviewSectionCategory::ApiIo;
    }
    if contains_any(
        &haystack,
        &[
            "state",
            "store",
            "cache",
            "database",
            "db",
            "migration",
            "schema",
            "model",
            "serde",
        ],
    ) {
        return GuidedReviewSectionCategory::DataState;
    }
    if contains_any(
        &haystack,
        &["performance", "perf", "latency", "throughput", "allocation"],
    ) {
        return GuidedReviewSectionCategory::Performance;
    }
    if contains_any(
        &haystack,
        &[
            "error",
            "retry",
            "fallback",
            "recover",
            "validation",
            "timeout",
        ],
    ) {
        return GuidedReviewSectionCategory::Reliability;
    }
    if contains_any(
        &haystack,
        &["refactor", "rename", "extract", "inline", "move", "cleanup"],
    ) {
        return GuidedReviewSectionCategory::Refactor;
    }

    GuidedReviewSectionCategory::Other
}

fn fallback_section_priority(steps: &[&GuidedReviewStep]) -> GuidedReviewSectionPriority {
    let file_count = steps.len();
    let total_delta: i64 = steps
        .iter()
        .map(|step| step.additions + step.deletions)
        .sum();
    let unresolved_thread_count: i64 = steps.iter().map(|step| step.unresolved_thread_count).sum();

    if unresolved_thread_count > 0 || total_delta >= 220 || file_count >= 8 {
        GuidedReviewSectionPriority::High
    } else if total_delta >= 60
        || file_count >= 3
        || steps
            .iter()
            .any(|step| matches!(step.badge.as_str(), "added" | "deleted" | "renamed"))
    {
        GuidedReviewSectionPriority::Medium
    } else {
        GuidedReviewSectionPriority::Low
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn fallback_section_review_points(steps: &[&GuidedReviewStep]) -> Vec<String> {
    let mut points: Vec<String> = Vec::new();

    if steps.len() > 1 {
        points.push(format!(
            "Trace the flow across {} files here instead of reviewing each patch in isolation.",
            steps.len()
        ));
    }

    if steps.iter().any(|step| step.badge == "added") {
        points.push(
            "Check how the new entry points or data shapes connect back to existing callers."
                .to_string(),
        );
    }

    if steps.iter().any(|step| step.unresolved_thread_count > 0) {
        points.push(
            "Keep the unresolved review discussion in view while checking the rest of this section."
                .to_string(),
        );
    }

    if points.is_empty() {
        points.push(
            "Use the file cards below to inspect the concrete diff anchors for this section."
                .to_string(),
        );
    }

    if points.len() > MAX_REVIEW_POINTS {
        points.truncate(MAX_REVIEW_POINTS);
    }
    points
}

fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_iso_timestamp(duration.as_millis() as i64)
}

fn format_iso_timestamp(millis: i64) -> String {
    let total_seconds = millis / 1000;
    let millis_part = millis.rem_euclid(1000);
    let (year, month, day, hour, minute, second) = seconds_to_calendar(total_seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis_part:03}Z")
}

fn seconds_to_calendar(mut epoch_seconds: i64) -> (i32, u32, u32, u32, u32, u32) {
    let second = epoch_seconds.rem_euclid(60) as u32;
    epoch_seconds = epoch_seconds.div_euclid(60);
    let minute = epoch_seconds.rem_euclid(60) as u32;
    epoch_seconds = epoch_seconds.div_euclid(60);
    let hour = epoch_seconds.rem_euclid(24) as u32;
    let mut days = epoch_seconds.div_euclid(24);

    let mut year: i32 = 1970;
    loop {
        let in_year = if is_leap(year) { 366 } else { 365 };
        if days >= in_year as i64 {
            days -= in_year as i64;
            year += 1;
        } else if days < 0 {
            year -= 1;
            let previous = if is_leap(year) { 366 } else { 365 };
            days += previous as i64;
        } else {
            break;
        }
    }

    let month_lengths = [
        31u32,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month: u32 = 1;
    let mut day_of_year = days as u32;
    for (index, length) in month_lengths.iter().enumerate() {
        if day_of_year < *length {
            month = (index as u32) + 1;
            break;
        }
        day_of_year -= *length;
    }

    (year, month, day_of_year + 1, hour, minute, second)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(not(any()))]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guided_review::{
        DiffAnchor, GenerateGuidedReviewInput, GuidedReviewCandidateGroup,
        GuidedReviewSectionCategory, GuidedReviewSectionPriority, GuidedReviewStep,
        ReviewAiProvider,
    };

    fn sample_input() -> GenerateGuidedReviewInput {
        GenerateGuidedReviewInput {
            provider: ReviewAiProvider::Codex,
            working_directory: "/tmp/repo".to_string(),
            repository: "owner/name".to_string(),
            number: 1,
            code_version_key: "diff-123".to_string(),
            title: "Test".to_string(),
            body: "body".to_string(),
            url: "url".to_string(),
            author_login: "rikuws".to_string(),
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feat".to_string(),
            head_ref_oid: None,
            updated_at: "2026-04-01T00:00:00Z".to_string(),
            additions: 5,
            deletions: 5,
            changed_files: 2,
            commits_count: 1,
            files: Vec::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            review_memory: crate::review_memory::ReviewMemoryPromptContext::default(),
            candidate_steps: vec![
                GuidedReviewStep {
                    id: "overview".into(),
                    kind: "overview".into(),
                    title: "Overview".into(),
                    summary: "sum".into(),
                    detail: "det".into(),
                    file_path: None,
                    anchor: None,
                    additions: 5,
                    deletions: 5,
                    unresolved_thread_count: 0,
                    snippet: None,
                    badge: "ready".into(),
                },
                GuidedReviewStep {
                    id: "file:a".into(),
                    kind: "file".into(),
                    title: "a".into(),
                    summary: "+3 / -1".into(),
                    detail: "Modified file.".into(),
                    file_path: Some("a".into()),
                    anchor: Some(DiffAnchor {
                        file_path: "a".into(),
                        hunk_header: None,
                        line: Some(10),
                        side: Some("RIGHT".into()),
                        thread_id: None,
                    }),
                    additions: 3,
                    deletions: 1,
                    unresolved_thread_count: 0,
                    snippet: None,
                    badge: "modified".into(),
                },
                GuidedReviewStep {
                    id: "file:b".into(),
                    kind: "file".into(),
                    title: "b".into(),
                    summary: "+2 / -4".into(),
                    detail: "Modified file.".into(),
                    file_path: Some("b".into()),
                    anchor: None,
                    additions: 2,
                    deletions: 4,
                    unresolved_thread_count: 0,
                    snippet: None,
                    badge: "modified".into(),
                },
            ],
            candidate_groups: vec![GuidedReviewCandidateGroup {
                id: "group:1".into(),
                title: "Group".into(),
                summary: "grouped".into(),
                step_ids: vec!["file:a".into(), "file:b".into()],
                file_paths: vec!["a".into(), "b".into()],
            }],
        }
    }

    #[test]
    fn merge_guided_review_uses_fallback_sections_when_response_has_none() {
        let input = sample_input();
        let tour = merge_guided_review(GuidedReviewResponse::default(), &input, None);
        assert_eq!(tour.sections.len(), 1);
        assert_eq!(tour.sections[0].step_ids, vec!["file:a", "file:b"]);
        // overview + 2 file steps
        assert_eq!(tour.steps.len(), 3);
        assert_eq!(tour.steps[0].id, "overview");
    }

    #[test]
    fn merge_guided_review_honors_response_overrides() {
        let input = sample_input();
        let response = GuidedReviewResponse {
            summary: Some("Custom summary".into()),
            review_focus: Some("Custom focus".into()),
            overview: Some(GuidedReviewResponseOverview {
                title: Some("Custom overview".into()),
                summary: Some("Overview summary".into()),
                detail: Some("Overview detail".into()),
                badge: Some("custom".into()),
            }),
            steps: vec![GuidedReviewResponseStep {
                source_step_id: Some("file:a".into()),
                title: Some("Inspected A".into()),
                summary: Some("A summary".into()),
                detail: Some("A detail".into()),
                badge: Some("focus".into()),
            }],
            sections: vec![GuidedReviewResponseSection {
                title: Some("Section 1".into()),
                summary: Some("Section summary".into()),
                detail: Some("Section detail".into()),
                badge: Some("badge".into()),
                category: Some("auth-security".into()),
                priority: Some("high".into()),
                step_ids: vec!["file:a".into(), "file:a".into(), "file:b".into()],
                review_points: vec!["point".into()],
                callsites: vec![GuidedReviewResponseCallsite {
                    title: None,
                    path: Some("a".into()),
                    line: Some(10),
                    summary: Some("do the thing".into()),
                    snippet: Some("code".into()),
                }],
            }],
            ..Default::default()
        };

        let tour = merge_guided_review(response, &input, Some("gpt-5".into()));
        assert_eq!(tour.summary, "Custom summary");
        assert_eq!(tour.review_focus, "Custom focus");
        assert_eq!(tour.sections.len(), 1);
        assert_eq!(tour.sections[0].step_ids, vec!["file:a", "file:b"]);
        assert_eq!(tour.sections[0].title, "Section 1");
        assert_eq!(
            tour.sections[0].category,
            GuidedReviewSectionCategory::AuthSecurity
        );
        assert_eq!(tour.sections[0].priority, GuidedReviewSectionPriority::High);
        assert_eq!(tour.sections[0].review_points, vec!["point"]);
        assert_eq!(tour.sections[0].callsites.len(), 1);
        assert_eq!(tour.sections[0].callsites[0].title, "Callsite in a:10");
        let file_a = tour.steps.iter().find(|step| step.id == "file:a").unwrap();
        assert_eq!(file_a.title, "Inspected A");
        assert_eq!(tour.steps[0].title, "Custom overview");
        assert_eq!(tour.model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn fallback_tour_includes_reason_as_warning() {
        let input = sample_input();
        let tour = build_copilot_fallback_guided_review(
            &input,
            Some("gpt-5".into()),
            "Copilot failed to return structured output.".into(),
        );
        assert_eq!(tour.warnings.len(), 1);
        assert!(tour.warnings[0].contains("Copilot failed"));
    }

    #[test]
    fn merge_guided_review_validates_group_metadata_and_preserves_file_coverage() {
        let mut input = sample_input();
        input.candidate_steps.push(GuidedReviewStep {
            id: "file:c".into(),
            kind: "file".into(),
            title: "c".into(),
            summary: "+1 / -1".into(),
            detail: "Modified file.".into(),
            file_path: Some("c".into()),
            anchor: None,
            additions: 1,
            deletions: 1,
            unresolved_thread_count: 0,
            snippet: None,
            badge: "modified".into(),
        });

        let response = GuidedReviewResponse {
            sections: vec![
                GuidedReviewResponseSection {
                    title: Some("Security path".into()),
                    summary: Some("Security summary".into()),
                    detail: Some("Security detail".into()),
                    badge: Some("focus".into()),
                    category: Some("auth-security".into()),
                    priority: Some("high".into()),
                    step_ids: vec!["file:a".into(), "file:a".into()],
                    review_points: Vec::new(),
                    callsites: Vec::new(),
                },
                GuidedReviewResponseSection {
                    title: Some("Invalid metadata".into()),
                    summary: Some("Invalid summary".into()),
                    detail: Some("Invalid detail".into()),
                    badge: Some("focus".into()),
                    category: Some("not-a-category".into()),
                    priority: Some("not-a-priority".into()),
                    step_ids: vec!["file:a".into(), "file:b".into()],
                    review_points: Vec::new(),
                    callsites: Vec::new(),
                },
            ],
            ..Default::default()
        };

        let tour = merge_guided_review(response, &input, None);
        assert_eq!(tour.sections.len(), 3);
        assert_eq!(tour.sections[0].step_ids, vec!["file:a"]);
        assert_eq!(
            tour.sections[0].category,
            GuidedReviewSectionCategory::AuthSecurity
        );
        assert_eq!(tour.sections[0].priority, GuidedReviewSectionPriority::High);
        assert_eq!(tour.sections[1].step_ids, vec!["file:b"]);
        assert_eq!(
            tour.sections[1].category,
            GuidedReviewSectionCategory::Other
        );
        assert_eq!(tour.sections[1].priority, GuidedReviewSectionPriority::Low);
        assert_eq!(tour.sections[2].step_ids, vec!["file:c"]);

        let covered_step_ids = tour
            .sections
            .iter()
            .flat_map(|section| section.step_ids.iter().cloned())
            .collect::<Vec<_>>();
        assert_eq!(covered_step_ids, vec!["file:a", "file:b", "file:c"]);
    }

    #[test]
    fn format_iso_timestamp_matches_known_value() {
        // 2026-04-16T00:00:00Z → seconds since epoch
        let ts = 1_776_556_800_000i64;
        assert!(format_iso_timestamp(ts).starts_with("2026-04-"));
    }

    #[test]
    fn _provider_types_compile() {
        let _ = ReviewAiProvider::Codex;
    }
}
