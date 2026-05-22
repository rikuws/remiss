use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::{agents, cache::CacheStore, github::PullRequestDetail};

const REVIEW_AI_SETTINGS_CACHE_KEY: &str = "review-ai-settings-v1";

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewAiProvider {
    #[default]
    Codex,
    Copilot,
}

impl ReviewAiProvider {
    pub fn slug(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Copilot => "copilot",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Copilot => "Copilot",
        }
    }

    pub fn all() -> &'static [ReviewAiProvider] {
        &[ReviewAiProvider::Codex, ReviewAiProvider::Copilot]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewAiSettings {
    #[serde(default)]
    pub provider: ReviewAiProvider,
    #[serde(default)]
    pub experimental_features_enabled: bool,
    #[serde(default)]
    pub background_jobs_enabled: bool,
    #[serde(default)]
    pub automatic_repositories: BTreeSet<String>,
}

impl ReviewAiSettings {
    pub fn experimental_features_enabled(&self) -> bool {
        self.experimental_features_enabled
    }

    pub fn background_jobs_enabled(&self) -> bool {
        self.experimental_features_enabled && self.background_jobs_enabled
    }

    pub fn automatically_generates_for(&self, repository: &str) -> bool {
        self.background_jobs_enabled() && self.repository_background_enabled(repository)
    }

    pub fn repository_background_enabled(&self, repository: &str) -> bool {
        self.automatic_repositories.contains(repository)
    }

    pub fn set_automatic_generation_for(&mut self, repository: &str, enabled: bool) {
        if enabled {
            self.automatic_repositories.insert(repository.to_string());
        } else {
            self.automatic_repositories.remove(repository);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewAiProviderStatus {
    pub provider: ReviewAiProvider,
    pub label: String,
    pub available: bool,
    pub authenticated: bool,
    pub message: String,
    pub detail: String,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffAnchor {
    pub file_path: String,
    pub hunk_header: Option<String>,
    pub line: Option<i64>,
    pub side: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewAiProgressUpdate {
    pub stage: String,
    pub summary: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub log_file_path: Option<String>,
}

pub fn load_review_ai_provider_statuses() -> Result<Vec<ReviewAiProviderStatus>, String> {
    Ok(agents::load_all_statuses())
}

pub fn load_review_ai_settings(cache: &CacheStore) -> Result<ReviewAiSettings, String> {
    Ok(cache
        .get::<ReviewAiSettings>(REVIEW_AI_SETTINGS_CACHE_KEY)?
        .map(|document| document.value)
        .unwrap_or_default())
}

pub fn save_review_ai_settings(
    cache: &CacheStore,
    settings: &ReviewAiSettings,
) -> Result<(), String> {
    cache.put(REVIEW_AI_SETTINGS_CACHE_KEY, settings, now_ms())
}

pub fn review_code_version_key(detail: &PullRequestDetail) -> String {
    if crate::local_review::is_local_review_detail(detail) {
        return format!(
            "local-base-{}-head-{}-diff-{}",
            detail.base_ref_oid.as_deref().unwrap_or_default(),
            detail.head_ref_oid.as_deref().unwrap_or_default(),
            hash_text(&detail.raw_diff)
        );
    }

    detail
        .head_ref_oid
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("head-{value}"))
        .unwrap_or_else(|| format!("diff-{}", hash_text(&detail.raw_diff)))
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_review_ai_settings_keep_experimental_features_off() {
        let settings = ReviewAiSettings::default();

        assert!(!settings.experimental_features_enabled());
        assert!(!settings.background_jobs_enabled());
        assert!(!settings.automatically_generates_for("acme/api"));
    }

    #[test]
    fn legacy_review_ai_settings_do_not_enable_agent_jobs() {
        let settings: ReviewAiSettings = serde_json::from_value(json!({
            "provider": "copilot",
            "automaticRepositories": ["acme/api"]
        }))
        .expect("legacy settings should deserialize");

        assert_eq!(settings.provider, ReviewAiProvider::Copilot);
        assert!(!settings.experimental_features_enabled());
        assert!(!settings.background_jobs_enabled());
        assert!(settings.repository_background_enabled("acme/api"));
        assert!(!settings.automatically_generates_for("acme/api"));
    }

    #[test]
    fn background_jobs_require_the_experimental_feature_gate() {
        let mut settings = ReviewAiSettings {
            background_jobs_enabled: true,
            ..ReviewAiSettings::default()
        };
        settings.set_automatic_generation_for("acme/api", true);

        assert!(!settings.background_jobs_enabled());
        assert!(!settings.automatically_generates_for("acme/api"));

        settings.experimental_features_enabled = true;

        assert!(settings.background_jobs_enabled());
        assert!(settings.automatically_generates_for("acme/api"));
    }
}
