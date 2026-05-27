pub(crate) fn is_automation_actor(login: &str) -> bool {
    let login = login.trim().to_ascii_lowercase();
    if login.is_empty() {
        return false;
    }

    login.contains("[bot]")
        || login.ends_with("-bot")
        || login.ends_with("bot")
        || matches!(
            login.as_str(),
            "copilot-pull-request-reviewer"
                | "github-actions"
                | "dependabot"
                | "renovate"
                | "vercel"
                | "netlify"
                | "supabase"
        )
}

#[cfg(test)]
mod tests {
    use super::is_automation_actor;

    #[test]
    fn automation_actor_detection_matches_bot_logins() {
        assert!(is_automation_actor("coderabbitai[bot]"));
        assert!(is_automation_actor("copilot-pull-request-reviewer"));
        assert!(is_automation_actor("github-actions"));
        assert!(is_automation_actor("review-bot"));
        assert!(!is_automation_actor("alice"));
    }
}
