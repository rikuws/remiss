use crate::code_tour::{CodeTourProvider, CodeTourProviderStatus};

use super::tour_view::resolve_preferred_provider;

fn provider_status(
    provider: CodeTourProvider,
    available: bool,
    authenticated: bool,
) -> CodeTourProviderStatus {
    CodeTourProviderStatus {
        provider,
        label: provider.label().to_string(),
        available,
        authenticated,
        message: String::new(),
        detail: String::new(),
        default_model: None,
    }
}

#[test]
fn auto_selects_the_only_ready_provider() {
    let statuses = vec![
        provider_status(CodeTourProvider::Codex, true, false),
        provider_status(CodeTourProvider::Copilot, true, true),
    ];

    assert_eq!(
        resolve_preferred_provider(&statuses, None, false),
        Some(CodeTourProvider::Copilot)
    );
}

#[test]
fn auto_selects_the_only_available_provider() {
    let statuses = vec![
        provider_status(CodeTourProvider::Codex, false, false),
        provider_status(CodeTourProvider::Copilot, true, false),
    ];

    assert_eq!(
        resolve_preferred_provider(&statuses, None, false),
        Some(CodeTourProvider::Copilot)
    );
}

#[test]
fn leaves_selection_empty_when_both_ready_and_not_manually_chosen() {
    let statuses = vec![
        provider_status(CodeTourProvider::Codex, true, true),
        provider_status(CodeTourProvider::Copilot, true, true),
    ];

    assert_eq!(resolve_preferred_provider(&statuses, None, false), None);
}

#[test]
fn preserves_manual_selection_when_multiple_providers_are_ready() {
    let statuses = vec![
        provider_status(CodeTourProvider::Codex, true, true),
        provider_status(CodeTourProvider::Copilot, true, true),
    ];

    assert_eq!(
        resolve_preferred_provider(&statuses, Some(CodeTourProvider::Codex), true),
        Some(CodeTourProvider::Codex)
    );
}
