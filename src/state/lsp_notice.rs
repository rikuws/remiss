use std::path::Path;

use crate::lsp::{self, LspServerState, LspServerStatus};
use crate::managed_lsp::{self, ManagedServerKind};

use super::DetailState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspStatusNotice {
    pub title: String,
    pub detail: String,
    pub install_kind: Option<ManagedServerKind>,
    pub busy: bool,
    pub dismissal_key: String,
}

impl DetailState {
    pub fn lsp_status_notice_for_path(&self, path: &str) -> Option<LspStatusNotice> {
        if self.lsp_loading_paths.contains(path) {
            let server_label = lsp::preferred_server_label_for_file(path)
                .unwrap_or_else(|| "Language server".to_string());
            return Some(LspStatusNotice {
                title: "Language server starting".to_string(),
                detail: format!(
                    "{server_label} is initializing for {}.",
                    lsp_notice_file_label(path)
                ),
                install_kind: None,
                busy: true,
                dismissal_key: format!("starting:{path}"),
            });
        }

        let status = self.lsp_statuses.get(path)?;
        if status.is_ready() {
            return None;
        }

        let install_kind = lsp::managed_server_kind_for_file(path);
        let language_label = lsp::language_label_for_file(path).unwrap_or("this");
        let dismissal_key = lsp_status_notice_dismissal_key(path, status, install_kind);
        match status.state {
            LspServerState::MissingServer => Some(LspStatusNotice {
                title: if install_kind.is_some() {
                    "Language server not installed".to_string()
                } else {
                    "Language server unavailable".to_string()
                },
                detail: if let Some(kind) = install_kind {
                    format!(
                        "Install {} to enable hover details for {language_label} files.",
                        managed_lsp::managed_server_display_name(kind)
                    )
                } else {
                    status.message.clone()
                },
                install_kind,
                busy: false,
                dismissal_key,
            }),
            LspServerState::CheckoutUnavailable => Some(LspStatusNotice {
                title: "Language server waiting on checkout".to_string(),
                detail: status.message.clone(),
                install_kind: None,
                busy: false,
                dismissal_key,
            }),
            LspServerState::Error => Some(LspStatusNotice {
                title: if install_kind.is_some() {
                    "Language server needs setup".to_string()
                } else {
                    "Language server unavailable".to_string()
                },
                detail: status.message.clone(),
                install_kind,
                busy: false,
                dismissal_key,
            }),
            LspServerState::UnsupportedLanguage | LspServerState::Ready => None,
        }
    }

    pub fn begin_lsp_symbol_loading(&mut self, path: &str) {
        *self
            .lsp_symbol_loading_paths
            .entry(path.to_string())
            .or_default() += 1;
    }

    pub fn finish_lsp_symbol_loading(&mut self, path: &str) {
        match self.lsp_symbol_loading_paths.get_mut(path) {
            Some(count) if *count > 1 => {
                *count -= 1;
            }
            Some(_) => {
                self.lsp_symbol_loading_paths.remove(path);
            }
            None => {}
        }
    }
}

fn lsp_notice_file_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn lsp_status_notice_dismissal_key(
    path: &str,
    status: &LspServerStatus,
    install_kind: Option<ManagedServerKind>,
) -> String {
    if let Some(kind) = install_kind {
        match status.state {
            LspServerState::MissingServer => return format!("managed-missing:{kind:?}"),
            LspServerState::Error => {
                return format!("managed-error:{kind:?}:{}", status.message.as_str());
            }
            _ => {}
        }
    }

    match status.state {
        LspServerState::MissingServer => format!(
            "missing:{}:{}",
            status.language_id.as_deref().unwrap_or_default(),
            status.command.as_deref().unwrap_or_default()
        ),
        LspServerState::CheckoutUnavailable => {
            format!("checkout:{}:{}", path, status.message.as_str())
        }
        LspServerState::Error => format!(
            "error:{}:{}:{}",
            path,
            status.command.as_deref().unwrap_or_default(),
            status.message.as_str()
        ),
        LspServerState::UnsupportedLanguage | LspServerState::Ready => {
            format!("inactive:{path}:{:?}", status.state)
        }
    }
}
