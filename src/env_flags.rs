pub fn env_var_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| strict_truthy_value(&value))
        .unwrap_or(false)
}

pub fn strict_truthy_value(value: &str) -> bool {
    let normalized = value.trim();
    matches!(normalized, "1")
        || normalized.eq_ignore_ascii_case("true")
        || normalized.eq_ignore_ascii_case("yes")
        || normalized.eq_ignore_ascii_case("on")
}

pub fn permissive_truthy_value(value: &str) -> bool {
    let normalized = value.trim();
    !normalized.is_empty()
        && !matches!(normalized, "0")
        && !normalized.eq_ignore_ascii_case("false")
        && !normalized.eq_ignore_ascii_case("no")
        && !normalized.eq_ignore_ascii_case("off")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_truthy_value_accepts_only_known_opt_in_values() {
        assert!(strict_truthy_value("1"));
        assert!(strict_truthy_value("true"));
        assert!(strict_truthy_value("YES"));
        assert!(strict_truthy_value(" on "));

        assert!(!strict_truthy_value(""));
        assert!(!strict_truthy_value("0"));
        assert!(!strict_truthy_value("false"));
        assert!(!strict_truthy_value("enabled"));
    }

    #[test]
    fn permissive_truthy_value_accepts_any_non_disabled_value() {
        assert!(permissive_truthy_value("1"));
        assert!(permissive_truthy_value("true"));
        assert!(permissive_truthy_value("yes"));
        assert!(permissive_truthy_value("enabled"));

        assert!(!permissive_truthy_value(""));
        assert!(!permissive_truthy_value(" "));
        assert!(!permissive_truthy_value("0"));
        assert!(!permissive_truthy_value("FALSE"));
        assert!(!permissive_truthy_value("no"));
        assert!(!permissive_truthy_value("off"));
    }
}
