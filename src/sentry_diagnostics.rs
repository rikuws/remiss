use std::borrow::Cow;

use serde_json::json;

use crate::branding;

const SENTRY_DSN: &str =
    "https://c80e5454cce8bec4e66968c595eb72e8@o4511421320265728.ingest.de.sentry.io/4511421325246544";
const MAX_SENTRY_TEXT_CHARS: usize = 12_000;

pub fn init() -> sentry::ClientInitGuard {
    let guard = sentry::init((
        SENTRY_DSN,
        sentry::ClientOptions {
            release: release_name(),
            send_default_pii: true,
            ..Default::default()
        },
    ));

    sentry::configure_scope(|scope| {
        scope.set_tag("app.name", branding::APP_NAME);
        scope.set_tag("app.version", release_version());
        scope.set_tag("target.os", std::env::consts::OS);
        scope.set_tag("target.arch", std::env::consts::ARCH);
    });

    guard
}

pub fn capture_error(message: &str) {
    sentry::with_scope(
        |scope| {
            scope.set_tag("remiss.event", "app_error");
        },
        || {
            sentry::capture_message(message, sentry::Level::Error);
        },
    );
}

pub fn capture_ai_failure(
    feature: &str,
    provider: Option<&str>,
    error: &str,
    configure_scope: impl FnOnce(&mut sentry::Scope),
) {
    let message = format!("{} AI feature failed: {feature}", branding::APP_NAME);
    sentry::with_scope(
        |scope| {
            scope.set_tag("remiss.event", "ai_failure");
            scope.set_tag("ai.feature", feature);
            scope.set_tag("ai.error_kind", ai_error_kind(error));
            if let Some(provider) = provider {
                scope.set_tag("ai.provider", provider);
            }
            scope.set_extra("error", json!(limit_sentry_text(error)));
            configure_scope(scope);
        },
        || {
            sentry::capture_message(&message, sentry::Level::Error);
        },
    );
}

fn release_name() -> Option<Cow<'static, str>> {
    let version = release_version();
    if version.trim().is_empty() {
        return sentry::release_name!();
    }

    Some(Cow::Owned(format!("{}@{version}", env!("CARGO_PKG_NAME"))))
}

fn release_version() -> String {
    #[cfg(target_os = "macos")]
    {
        crate::platform_macos::app_short_version()
    }

    #[cfg(not(target_os = "macos"))]
    {
        option_env!("REMISS_VERSION")
            .filter(|version| !version.trim().is_empty())
            .unwrap_or(branding::APP_VERSION)
            .to_string()
    }
}

fn ai_error_kind(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("not authenticated") || lower.contains("login") {
        "authentication"
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "timeout"
    } else if lower.contains("permission") || lower.contains("denied") {
        "permission"
    } else if lower.contains("parse") || lower.contains("json") || lower.contains("schema") {
        "parse"
    } else if lower.contains("prompt exceeded") || lower.contains("truncated prompt") {
        "prompt_budget"
    } else if lower.contains("no final") || lower.contains("without returning") {
        "empty_response"
    } else {
        "provider"
    }
}

fn limit_sentry_text(value: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= MAX_SENTRY_TEXT_CHARS {
            output.push_str("...");
            break;
        }
        output.push(ch);
    }
    output
}
