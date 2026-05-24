use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryTrustIndex {
    pub repository: String,
    #[serde(default)]
    pub source_paths: Vec<String>,
    #[serde(default)]
    pub records: BTreeMap<String, TrustRecord>,
    #[serde(default)]
    pub state: RepositoryTrustState,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryTrustState {
    Loaded,
    #[default]
    Missing,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustRecord {
    pub status: TrustRecordStatus,
    #[serde(default)]
    pub reason: Option<String>,
    pub source_path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustRecordStatus {
    Vouched,
    Denounced,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestTriageSignal {
    pub kind: PullRequestTriageSignalKind,
    pub label: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PullRequestTriageSignalKind {
    Vouched,
    Denounced,
    Trusted,
    PriorContributor,
    FirstTimeContributor,
    TrustUnknown,
    NoTrustList,
    TrustListError,
}

impl PullRequestTriageSignal {
    fn new(kind: PullRequestTriageSignalKind, label: &str, detail: Option<String>) -> Self {
        Self {
            kind,
            label: label.to_string(),
            detail,
        }
    }
}

pub fn parse_trustdown_file(
    path: &str,
    content: &str,
) -> (BTreeMap<String, TrustRecord>, Vec<String>) {
    let mut records = BTreeMap::new();
    let mut warnings = Vec::new();

    for (line_ix, raw_line) in content.lines().enumerate() {
        let line_number = line_ix + 1;
        let line = raw_line
            .split_once('#')
            .map(|(value, _)| value)
            .unwrap_or(raw_line)
            .trim();
        if line.is_empty() {
            continue;
        }

        let token_end = line.find(char::is_whitespace).unwrap_or(line.len());
        let token = &line[..token_end];
        let reason = line[token_end..].trim();
        let denounced = token.starts_with('-');
        let handle = token.trim_start_matches('-').trim_start_matches('@');
        let (platform, username) = handle
            .split_once(':')
            .map(|(platform, username)| (Some(platform), username))
            .unwrap_or((None, handle));

        if !matches!(
            platform.map(str::to_ascii_lowercase).as_deref(),
            None | Some("github")
        ) {
            continue;
        }

        let username = username.trim_start_matches('@').trim();
        if username.is_empty() {
            warnings.push(format!("{path}:{line_number} has an empty trust handle."));
            continue;
        }
        if username.chars().any(char::is_whitespace) {
            warnings.push(format!(
                "{path}:{line_number} has whitespace inside the trust handle."
            ));
            continue;
        }

        let key = username.to_ascii_lowercase();
        let status = if denounced {
            TrustRecordStatus::Denounced
        } else {
            TrustRecordStatus::Vouched
        };
        let reason = reason.trim();
        let record = TrustRecord {
            status,
            reason: (!reason.is_empty()).then(|| reason.to_string()),
            source_path: path.to_string(),
        };

        let should_replace = records
            .get(&key)
            .map(|existing: &TrustRecord| {
                record.status == TrustRecordStatus::Denounced
                    || existing.status != TrustRecordStatus::Denounced
            })
            .unwrap_or(true);
        if should_replace {
            records.insert(key, record);
        }
    }

    (records, warnings)
}

pub fn triage_signals_for_author(
    author_login: &str,
    author_association: &str,
    trust_index: Option<&RepositoryTrustIndex>,
) -> Vec<PullRequestTriageSignal> {
    let mut signals = Vec::new();
    let author_key = author_login.to_ascii_lowercase();

    if let Some(index) = trust_index {
        if let Some(record) = index.records.get(&author_key) {
            match record.status {
                TrustRecordStatus::Denounced => {
                    signals.push(PullRequestTriageSignal::new(
                        PullRequestTriageSignalKind::Denounced,
                        "denounced",
                        record.reason.clone(),
                    ));
                    return signals;
                }
                TrustRecordStatus::Vouched => {
                    signals.push(PullRequestTriageSignal::new(
                        PullRequestTriageSignalKind::Vouched,
                        "vouched",
                        Some(record.source_path.clone()),
                    ));
                    return signals;
                }
            }
        }
    }

    let association = author_association.trim().to_ascii_uppercase();
    match association.as_str() {
        "OWNER" | "MEMBER" | "COLLABORATOR" => {
            signals.push(PullRequestTriageSignal::new(
                PullRequestTriageSignalKind::Trusted,
                "trusted",
                (!association.is_empty()).then_some(association.to_ascii_lowercase()),
            ));
        }
        "CONTRIBUTOR" => {
            signals.push(PullRequestTriageSignal::new(
                PullRequestTriageSignalKind::PriorContributor,
                "prior contributor",
                None,
            ));
        }
        "FIRST_TIMER" | "FIRST_TIME_CONTRIBUTOR" => {
            signals.push(PullRequestTriageSignal::new(
                PullRequestTriageSignalKind::FirstTimeContributor,
                "first-time contributor",
                None,
            ));
        }
        _ => {}
    }

    if signals.is_empty() {
        match trust_index
            .map(|index| index.state)
            .unwrap_or(RepositoryTrustState::Missing)
        {
            RepositoryTrustState::Loaded => signals.push(PullRequestTriageSignal::new(
                PullRequestTriageSignalKind::TrustUnknown,
                "trust unknown",
                None,
            )),
            RepositoryTrustState::Missing => signals.push(PullRequestTriageSignal::new(
                PullRequestTriageSignalKind::NoTrustList,
                "no trust list",
                None,
            )),
            RepositoryTrustState::Error => signals.push(PullRequestTriageSignal::new(
                PullRequestTriageSignalKind::TrustListError,
                "trust warning",
                trust_index.and_then(|index| index.message.clone()),
            )),
        }
    }

    signals
}

pub fn has_signal(signals: &[PullRequestTriageSignal], kind: PullRequestTriageSignalKind) -> bool {
    signals.iter().any(|signal| signal.kind == kind)
}

pub fn has_trusted_signal(signals: &[PullRequestTriageSignal]) -> bool {
    has_signal(signals, PullRequestTriageSignalKind::Vouched)
        || has_signal(signals, PullRequestTriageSignalKind::Trusted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(records: BTreeMap<String, TrustRecord>) -> RepositoryTrustIndex {
        RepositoryTrustIndex {
            repository: "owner/repo".to_string(),
            source_paths: vec!["VOUCHED.td".to_string()],
            records,
            state: RepositoryTrustState::Loaded,
            message: None,
        }
    }

    #[test]
    fn parses_vouch_compatible_trustdown() {
        let content = r#"
            # Comments are ignored.
            mitchellh
            github:alice
            gitlab:ignored
            -github:badguy Submitted repeated spam
            -slopmaster3000
        "#;

        let (records, warnings) = parse_trustdown_file("VOUCHED.td", content);

        assert!(warnings.is_empty());
        assert_eq!(records["mitchellh"].status, TrustRecordStatus::Vouched);
        assert_eq!(records["alice"].status, TrustRecordStatus::Vouched);
        assert_eq!(records["badguy"].status, TrustRecordStatus::Denounced);
        assert_eq!(
            records["badguy"].reason.as_deref(),
            Some("Submitted repeated spam")
        );
        assert_eq!(
            records["slopmaster3000"].status,
            TrustRecordStatus::Denounced
        );
        assert!(!records.contains_key("ignored"));
    }

    #[test]
    fn denounced_record_overrides_vouched_record() {
        let (records, _) =
            parse_trustdown_file("VOUCHED.td", "alice\n-github:alice reason\ngithub:alice\n");

        assert_eq!(records["alice"].status, TrustRecordStatus::Denounced);
        assert_eq!(records["alice"].reason.as_deref(), Some("reason"));
    }

    #[test]
    fn triage_signals_prioritize_explicit_trust() {
        let (records, _) = parse_trustdown_file("VOUCHED.td", "-alice no\nbob\n");
        let index = index(records);

        assert_eq!(
            triage_signals_for_author("alice", "OWNER", Some(&index))[0].kind,
            PullRequestTriageSignalKind::Denounced
        );
        assert_eq!(
            triage_signals_for_author("bob", "NONE", Some(&index))[0].kind,
            PullRequestTriageSignalKind::Vouched
        );
    }

    #[test]
    fn triage_signals_use_github_association_without_vouch_record() {
        assert_eq!(
            triage_signals_for_author("alice", "COLLABORATOR", None)[0].kind,
            PullRequestTriageSignalKind::Trusted
        );
        assert_eq!(
            triage_signals_for_author("alice", "CONTRIBUTOR", None)[0].kind,
            PullRequestTriageSignalKind::PriorContributor
        );
        assert_eq!(
            triage_signals_for_author("alice", "FIRST_TIME_CONTRIBUTOR", None)[0].kind,
            PullRequestTriageSignalKind::FirstTimeContributor
        );
    }

    #[test]
    fn triage_signals_explain_missing_or_loaded_trust_list() {
        assert_eq!(
            triage_signals_for_author("alice", "NONE", None)[0].kind,
            PullRequestTriageSignalKind::NoTrustList
        );

        let loaded = RepositoryTrustIndex {
            repository: "owner/repo".to_string(),
            state: RepositoryTrustState::Loaded,
            ..RepositoryTrustIndex::default()
        };
        assert_eq!(
            triage_signals_for_author("alice", "NONE", Some(&loaded))[0].kind,
            PullRequestTriageSignalKind::TrustUnknown
        );
    }
}
