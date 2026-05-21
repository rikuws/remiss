use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{cache::CacheStore, github::PullRequestSummary};

pub(crate) const MUTED_REPOSITORIES_CACHE_KEY: &str = "muted-repositories-v1";
const PULL_REQUEST_FILTER_SETTINGS_CACHE_KEY: &str = "pull-request-filters-v1";
const FRESH_WINDOW_DAYS: i64 = 3;
const STALE_AFTER_DAYS: i64 = 14;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MutedRepositoriesSettings {
    #[serde(default)]
    repositories: Vec<String>,
}

impl MutedRepositoriesSettings {
    fn from_repositories(repositories: &HashSet<String>) -> Self {
        let mut repositories = repositories.iter().cloned().collect::<Vec<_>>();
        repositories.sort();
        repositories.dedup();
        Self { repositories }
    }
}

pub(crate) fn load_muted_repositories(cache: &CacheStore) -> Result<HashSet<String>, String> {
    Ok(cache
        .get::<MutedRepositoriesSettings>(MUTED_REPOSITORIES_CACHE_KEY)?
        .map(|document| {
            document
                .value
                .repositories
                .into_iter()
                .filter(|repository| !repository.trim().is_empty())
                .collect()
        })
        .unwrap_or_default())
}

pub(crate) fn save_muted_repositories(
    cache: &CacheStore,
    repositories: &HashSet<String>,
) -> Result<(), String> {
    cache.put(
        MUTED_REPOSITORIES_CACHE_KEY,
        &MutedRepositoriesSettings::from_repositories(repositories),
        now_ms(),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullRequestFilterScope {
    Overview,
    Pulls,
    Reviews,
}

impl PullRequestFilterScope {
    pub fn all() -> &'static [Self] {
        &[Self::Overview, Self::Pulls, Self::Reviews]
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Pulls => "pulls",
            Self::Reviews => "reviews",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullRequestFilterToggle {
    Unread,
    Ready,
    Draft,
    Fresh,
    Stale,
    Large,
    NeedsReview,
    IncludeMuted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFilterSettings {
    #[serde(default)]
    scopes: BTreeMap<String, PullRequestFilterScopeSettings>,
}

impl Default for PullRequestFilterSettings {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl PullRequestFilterSettings {
    pub fn with_defaults() -> Self {
        let mut settings = Self {
            scopes: BTreeMap::new(),
        };
        for scope in PullRequestFilterScope::all() {
            settings.scopes.insert(
                scope.key().to_string(),
                PullRequestFilterScopeSettings::default_for_scope(*scope),
            );
        }
        settings
    }

    pub fn normalize(mut self) -> Self {
        for scope in PullRequestFilterScope::all() {
            let entry = self
                .scopes
                .entry(scope.key().to_string())
                .or_insert_with(|| PullRequestFilterScopeSettings::default_for_scope(*scope));
            entry.normalize(*scope);
        }
        self
    }

    pub fn current_filter(&self, scope: PullRequestFilterScope) -> PullRequestFilter {
        self.scopes
            .get(scope.key())
            .map(|scope| scope.current.clone())
            .unwrap_or_default()
    }

    pub fn active_preset_id(&self, scope: PullRequestFilterScope) -> Option<&str> {
        self.scopes
            .get(scope.key())
            .and_then(|scope| scope.active_preset_id.as_deref())
    }

    pub fn presets(&self, scope: PullRequestFilterScope) -> Vec<PullRequestFilterPreset> {
        self.scopes
            .get(scope.key())
            .map(|scope| scope.presets.clone())
            .unwrap_or_else(|| default_presets(scope))
    }

    pub fn set_active_preset(&mut self, scope: PullRequestFilterScope, preset_id: &str) {
        let entry = self
            .scopes
            .entry(scope.key().to_string())
            .or_insert_with(|| PullRequestFilterScopeSettings::default_for_scope(scope));
        entry.set_active_preset(preset_id, scope);
    }

    pub fn toggle(&mut self, scope: PullRequestFilterScope, toggle: PullRequestFilterToggle) {
        let entry = self
            .scopes
            .entry(scope.key().to_string())
            .or_insert_with(|| PullRequestFilterScopeSettings::default_for_scope(scope));
        entry.current.toggle(toggle);
        entry.active_preset_id = None;
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFilterScopeSettings {
    #[serde(default)]
    current: PullRequestFilter,
    #[serde(default)]
    active_preset_id: Option<String>,
    #[serde(default)]
    presets: Vec<PullRequestFilterPreset>,
}

impl PullRequestFilterScopeSettings {
    fn default_for_scope(scope: PullRequestFilterScope) -> Self {
        let presets = default_presets(scope);
        let current = presets
            .first()
            .map(|preset| preset.filter.clone())
            .unwrap_or_default();
        let active_preset_id = presets.first().map(|preset| preset.id.clone());
        Self {
            current,
            active_preset_id,
            presets,
        }
    }

    fn normalize(&mut self, scope: PullRequestFilterScope) {
        if self.presets.is_empty() {
            self.presets = default_presets(scope);
        }
        self.presets.sort_by_key(|preset| preset.order);
        self.presets.dedup_by(|left, right| left.id == right.id);
        if self
            .active_preset_id
            .as_deref()
            .map(|id| self.presets.iter().any(|preset| preset.id == id))
            .unwrap_or(false)
        {
            return;
        }
        if self.current == PullRequestFilter::default() {
            self.active_preset_id = self.presets.first().map(|preset| preset.id.clone());
        } else {
            self.active_preset_id = None;
        }
    }

    fn set_active_preset(&mut self, preset_id: &str, scope: PullRequestFilterScope) {
        self.normalize(scope);
        if let Some(preset) = self.presets.iter().find(|preset| preset.id == preset_id) {
            self.current = preset.filter.clone();
            self.active_preset_id = Some(preset.id.clone());
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFilterPreset {
    pub id: String,
    pub label: String,
    pub order: usize,
    pub filter: PullRequestFilter,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFilter {
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub draft: DraftFilter,
    #[serde(default)]
    pub activity: ActivityFilter,
    #[serde(default)]
    pub freshness: FreshnessFilter,
    #[serde(default)]
    pub size: SizeFilter,
    #[serde(default)]
    pub review_decision: ReviewDecisionFilter,
    #[serde(default)]
    pub include_muted: bool,
}

impl Default for PullRequestFilter {
    fn default() -> Self {
        Self {
            repository: None,
            author: None,
            draft: DraftFilter::Any,
            activity: ActivityFilter::Any,
            freshness: FreshnessFilter::Any,
            size: SizeFilter::Any,
            review_decision: ReviewDecisionFilter::Any,
            include_muted: false,
        }
    }
}

impl PullRequestFilter {
    pub fn needs_unread_context(&self) -> bool {
        self.activity == ActivityFilter::Unread
    }

    pub fn matches(
        &self,
        summary: &PullRequestSummary,
        context: &PullRequestFilterContext,
    ) -> bool {
        if !self.include_muted && context.muted_repositories.contains(&summary.repository) {
            return false;
        }
        if self
            .repository
            .as_deref()
            .map(|repository| repository != summary.repository)
            .unwrap_or(false)
        {
            return false;
        }
        if self
            .author
            .as_deref()
            .map(|author| author != summary.author_login)
            .unwrap_or(false)
        {
            return false;
        }
        if !self.draft.matches(summary.is_draft) {
            return false;
        }
        if self.activity == ActivityFilter::Unread
            && !context.unread_pr_keys.contains(&summary_cache_key(summary))
        {
            return false;
        }
        if self.freshness != FreshnessFilter::Any
            && !self
                .freshness
                .matches(summary.updated_at.as_str(), context.now_epoch_days)
        {
            return false;
        }
        if self.size != SizeFilter::Any && !self.size.matches(summary) {
            return false;
        }
        self.review_decision
            .matches(summary.review_decision.as_deref())
    }

    pub fn active_labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        if let Some(repository) = self.repository.as_ref() {
            labels.push(format!("repo:{repository}"));
        }
        if let Some(author) = self.author.as_ref() {
            labels.push(format!("author:{author}"));
        }
        labels.extend(self.draft.active_label().map(str::to_string));
        labels.extend(self.activity.active_label().map(str::to_string));
        labels.extend(self.freshness.active_label().map(str::to_string));
        labels.extend(self.size.active_label().map(str::to_string));
        labels.extend(self.review_decision.active_label().map(str::to_string));
        if self.include_muted {
            labels.push("muted included".to_string());
        }
        labels
    }

    fn toggle(&mut self, toggle: PullRequestFilterToggle) {
        match toggle {
            PullRequestFilterToggle::Unread => {
                self.activity = if self.activity == ActivityFilter::Unread {
                    ActivityFilter::Any
                } else {
                    ActivityFilter::Unread
                };
            }
            PullRequestFilterToggle::Ready => {
                self.draft = if self.draft == DraftFilter::Ready {
                    DraftFilter::Any
                } else {
                    DraftFilter::Ready
                };
            }
            PullRequestFilterToggle::Draft => {
                self.draft = if self.draft == DraftFilter::Draft {
                    DraftFilter::Any
                } else {
                    DraftFilter::Draft
                };
            }
            PullRequestFilterToggle::Fresh => {
                self.freshness = if self.freshness == FreshnessFilter::Fresh {
                    FreshnessFilter::Any
                } else {
                    FreshnessFilter::Fresh
                };
            }
            PullRequestFilterToggle::Stale => {
                self.freshness = if self.freshness == FreshnessFilter::Stale {
                    FreshnessFilter::Any
                } else {
                    FreshnessFilter::Stale
                };
            }
            PullRequestFilterToggle::Large => {
                self.size = if self.size == SizeFilter::Large {
                    SizeFilter::Any
                } else {
                    SizeFilter::Large
                };
            }
            PullRequestFilterToggle::NeedsReview => {
                self.review_decision =
                    if self.review_decision == ReviewDecisionFilter::ReviewRequired {
                        ReviewDecisionFilter::Any
                    } else {
                        ReviewDecisionFilter::ReviewRequired
                    };
            }
            PullRequestFilterToggle::IncludeMuted => {
                self.include_muted = !self.include_muted;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DraftFilter {
    #[default]
    Any,
    Ready,
    Draft,
}

impl DraftFilter {
    fn matches(self, is_draft: bool) -> bool {
        match self {
            Self::Any => true,
            Self::Ready => !is_draft,
            Self::Draft => is_draft,
        }
    }

    fn active_label(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Ready => Some("ready"),
            Self::Draft => Some("draft"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityFilter {
    #[default]
    Any,
    Unread,
}

impl ActivityFilter {
    fn active_label(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Unread => Some("unread"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FreshnessFilter {
    #[default]
    Any,
    Fresh,
    Stale,
}

impl FreshnessFilter {
    fn matches(self, updated_at: &str, now_epoch_days: i64) -> bool {
        let Some(updated_days) = parse_iso_date_to_epoch_days(updated_at) else {
            return self == Self::Any;
        };
        let age_days = now_epoch_days.saturating_sub(updated_days);
        match self {
            Self::Any => true,
            Self::Fresh => age_days <= FRESH_WINDOW_DAYS,
            Self::Stale => age_days >= STALE_AFTER_DAYS,
        }
    }

    fn active_label(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Fresh => Some("fresh"),
            Self::Stale => Some("stale"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SizeFilter {
    #[default]
    Any,
    Small,
    Medium,
    Large,
}

impl SizeFilter {
    fn matches(self, summary: &PullRequestSummary) -> bool {
        let size = summary_size(summary);
        match self {
            Self::Any => true,
            Self::Small => size == Self::Small,
            Self::Medium => size == Self::Medium,
            Self::Large => size == Self::Large,
        }
    }

    fn active_label(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Small => Some("small"),
            Self::Medium => Some("medium"),
            Self::Large => Some("large"),
        }
    }
}

fn summary_size(summary: &PullRequestSummary) -> SizeFilter {
    let changed_lines = summary.additions.saturating_add(summary.deletions);
    if summary.changed_files >= 30 || changed_lines >= 1_000 {
        SizeFilter::Large
    } else if summary.changed_files <= 8 && changed_lines <= 250 {
        SizeFilter::Small
    } else {
        SizeFilter::Medium
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewDecisionFilter {
    #[default]
    Any,
    NoDecision,
    Approved,
    ChangesRequested,
    ReviewRequired,
    Commented,
}

impl ReviewDecisionFilter {
    fn matches(self, review_decision: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::NoDecision => review_decision.is_none(),
            Self::Approved => review_decision == Some("APPROVED"),
            Self::ChangesRequested => review_decision == Some("CHANGES_REQUESTED"),
            Self::ReviewRequired => review_decision == Some("REVIEW_REQUIRED"),
            Self::Commented => review_decision == Some("COMMENTED"),
        }
    }

    fn active_label(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::NoDecision => Some("no decision"),
            Self::Approved => Some("approved"),
            Self::ChangesRequested => Some("changes requested"),
            Self::ReviewRequired => Some("needs review"),
            Self::Commented => Some("commented"),
        }
    }
}

pub struct PullRequestFilterContext<'a> {
    pub muted_repositories: &'a HashSet<String>,
    pub unread_pr_keys: &'a BTreeSet<String>,
    pub now_epoch_days: i64,
}

pub fn filter_pull_requests(
    items: &[PullRequestSummary],
    filter: &PullRequestFilter,
    context: &PullRequestFilterContext,
) -> Vec<PullRequestSummary> {
    items
        .iter()
        .filter(|item| filter.matches(item, context))
        .cloned()
        .collect()
}

pub fn load_pull_request_filter_settings(
    cache: &CacheStore,
) -> Result<PullRequestFilterSettings, String> {
    Ok(cache
        .get::<PullRequestFilterSettings>(PULL_REQUEST_FILTER_SETTINGS_CACHE_KEY)?
        .map(|document| document.value.normalize())
        .unwrap_or_default())
}

pub fn save_pull_request_filter_settings(
    cache: &CacheStore,
    settings: &PullRequestFilterSettings,
) -> Result<(), String> {
    cache.put(
        PULL_REQUEST_FILTER_SETTINGS_CACHE_KEY,
        &settings.clone().normalize(),
        now_ms(),
    )
}

pub fn current_epoch_days() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_secs() / 86_400) as i64)
        .unwrap_or_default()
}

fn default_presets(scope: PullRequestFilterScope) -> Vec<PullRequestFilterPreset> {
    let mut presets = vec![preset("all", "All", 0, PullRequestFilter::default())];
    match scope {
        PullRequestFilterScope::Overview => {
            presets.push(preset("unread", "Unread", 1, unread_filter()));
            presets.push(preset(
                "attention",
                "Needs Review",
                2,
                needs_review_filter(),
            ));
            presets.push(preset("large", "Large", 3, large_ready_filter()));
        }
        PullRequestFilterScope::Pulls => {
            presets.push(preset("ready", "Ready", 1, ready_filter()));
            presets.push(preset("drafts", "Drafts", 2, draft_filter()));
            presets.push(preset("large", "Large", 3, large_ready_filter()));
        }
        PullRequestFilterScope::Reviews => {
            presets.push(preset("unread", "Unread", 1, unread_filter()));
            presets.push(preset(
                "attention",
                "Needs Review",
                2,
                needs_review_filter(),
            ));
            presets.push(preset("stale", "Stale", 3, stale_filter()));
        }
    }
    presets
}

fn preset(
    id: &str,
    label: &str,
    order: usize,
    filter: PullRequestFilter,
) -> PullRequestFilterPreset {
    PullRequestFilterPreset {
        id: id.to_string(),
        label: label.to_string(),
        order,
        filter,
    }
}

fn ready_filter() -> PullRequestFilter {
    PullRequestFilter {
        draft: DraftFilter::Ready,
        ..PullRequestFilter::default()
    }
}

fn draft_filter() -> PullRequestFilter {
    PullRequestFilter {
        draft: DraftFilter::Draft,
        ..PullRequestFilter::default()
    }
}

fn unread_filter() -> PullRequestFilter {
    PullRequestFilter {
        activity: ActivityFilter::Unread,
        ..PullRequestFilter::default()
    }
}

fn needs_review_filter() -> PullRequestFilter {
    PullRequestFilter {
        draft: DraftFilter::Ready,
        review_decision: ReviewDecisionFilter::ReviewRequired,
        ..PullRequestFilter::default()
    }
}

fn stale_filter() -> PullRequestFilter {
    PullRequestFilter {
        freshness: FreshnessFilter::Stale,
        ..PullRequestFilter::default()
    }
}

fn large_ready_filter() -> PullRequestFilter {
    PullRequestFilter {
        draft: DraftFilter::Ready,
        size: SizeFilter::Large,
        ..PullRequestFilter::default()
    }
}

fn summary_cache_key(summary: &PullRequestSummary) -> String {
    summary
        .local_key
        .clone()
        .unwrap_or_else(|| format!("{}#{}", summary.repository, summary.number))
}

fn parse_iso_date_to_epoch_days(value: &str) -> Option<i64> {
    let date = value.get(0..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day = day as i64;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp_cache_store(name: &str) -> CacheStore {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/remiss-filter-test-{name}-{suffix}/cache.sqlite"
        ));
        CacheStore::new(path).expect("cache")
    }

    fn summary(
        repository: &str,
        number: i64,
        author: &str,
        updated_at: &str,
    ) -> PullRequestSummary {
        PullRequestSummary {
            repository: repository.to_string(),
            number,
            title: format!("PR {number}"),
            author_login: author.to_string(),
            author_avatar_url: None,
            is_draft: false,
            comments_count: 0,
            additions: 20,
            deletions: 10,
            changed_files: 2,
            state: "OPEN".to_string(),
            review_decision: None,
            updated_at: updated_at.to_string(),
            url: String::new(),
            local_key: None,
        }
    }

    fn context<'a>(
        muted_repositories: &'a HashSet<String>,
        unread_pr_keys: &'a BTreeSet<String>,
    ) -> PullRequestFilterContext<'a> {
        PullRequestFilterContext {
            muted_repositories,
            unread_pr_keys,
            now_epoch_days: parse_iso_date_to_epoch_days("2026-05-21T00:00:00Z").unwrap(),
        }
    }

    #[test]
    fn local_filter_matches_all_facets() {
        let mut item = summary("owner/repo", 7, "alice", "2026-05-20T12:00:00Z");
        item.review_decision = Some("REVIEW_REQUIRED".to_string());
        item.additions = 900;
        item.deletions = 200;
        item.changed_files = 12;

        let muted = HashSet::new();
        let unread = BTreeSet::from(["owner/repo#7".to_string()]);
        let ctx = context(&muted, &unread);
        let filter = PullRequestFilter {
            repository: Some("owner/repo".to_string()),
            author: Some("alice".to_string()),
            draft: DraftFilter::Ready,
            activity: ActivityFilter::Unread,
            freshness: FreshnessFilter::Fresh,
            size: SizeFilter::Large,
            review_decision: ReviewDecisionFilter::ReviewRequired,
            include_muted: false,
        };

        assert!(filter.matches(&item, &ctx));

        item.is_draft = true;
        assert!(!filter.matches(&item, &ctx));
    }

    #[test]
    fn filter_preserves_fetched_queue_order() {
        let items = vec![
            summary("owner/repo", 1, "alice", "2026-05-20T00:00:00Z"),
            summary("owner/repo", 2, "alice", "2026-05-19T00:00:00Z"),
            summary("owner/repo", 3, "bob", "2026-05-18T00:00:00Z"),
        ];
        let filter = PullRequestFilter {
            author: Some("alice".to_string()),
            ..PullRequestFilter::default()
        };
        let muted = HashSet::new();
        let unread = BTreeSet::new();
        let filtered = filter_pull_requests(&items, &filter, &context(&muted, &unread));

        assert_eq!(
            filtered
                .into_iter()
                .map(|item| item.number)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn muted_repositories_are_hidden_unless_filter_includes_them() {
        let item = summary("owner/muted", 1, "alice", "2026-05-20T00:00:00Z");
        let muted = HashSet::from(["owner/muted".to_string()]);
        let unread = BTreeSet::new();
        let ctx = context(&muted, &unread);

        assert!(!PullRequestFilter::default().matches(&item, &ctx));

        let filter = PullRequestFilter {
            include_muted: true,
            ..PullRequestFilter::default()
        };
        assert!(filter.matches(&item, &ctx));
    }

    #[test]
    fn filter_settings_persist_active_preset_and_current_filter() {
        let cache = temp_cache_store("settings");
        let mut settings = PullRequestFilterSettings::default();
        settings.set_active_preset(PullRequestFilterScope::Reviews, "stale");
        settings.toggle(
            PullRequestFilterScope::Reviews,
            PullRequestFilterToggle::Unread,
        );

        save_pull_request_filter_settings(&cache, &settings).expect("save filters");
        let loaded = load_pull_request_filter_settings(&cache).expect("load filters");
        let filter = loaded.current_filter(PullRequestFilterScope::Reviews);

        assert_eq!(
            loaded.active_preset_id(PullRequestFilterScope::Reviews),
            None
        );
        assert_eq!(filter.freshness, FreshnessFilter::Stale);
        assert_eq!(filter.activity, ActivityFilter::Unread);
        assert_eq!(
            loaded
                .presets(PullRequestFilterScope::Reviews)
                .into_iter()
                .map(|preset| preset.id)
                .collect::<Vec<_>>(),
            vec!["all", "unread", "attention", "stale"]
        );
    }

    #[test]
    fn muted_repository_settings_round_trip_in_stable_order() {
        let cache = temp_cache_store("muted-repositories");
        let repositories = HashSet::from([
            "zeta/project".to_string(),
            "alpha/project".to_string(),
            "alpha/project".to_string(),
        ]);

        save_muted_repositories(&cache, &repositories).expect("save muted repositories");

        let loaded = load_muted_repositories(&cache).expect("load muted repositories");
        assert_eq!(loaded, repositories);

        let document = cache
            .get::<MutedRepositoriesSettings>(MUTED_REPOSITORIES_CACHE_KEY)
            .expect("read muted repositories document")
            .expect("muted repositories document exists");
        assert_eq!(
            document.value.repositories,
            vec!["alpha/project".to_string(), "zeta/project".to_string()]
        );
    }
}
