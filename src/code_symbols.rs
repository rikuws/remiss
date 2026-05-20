#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeSymbolPattern {
    pub prefix: &'static str,
    pub kind: &'static str,
}

pub const DECLARATION_PATTERNS: &[CodeSymbolPattern] = &[
    CodeSymbolPattern {
        prefix: "fn ",
        kind: "function",
    },
    CodeSymbolPattern {
        prefix: "struct ",
        kind: "struct",
    },
    CodeSymbolPattern {
        prefix: "enum ",
        kind: "enum",
    },
    CodeSymbolPattern {
        prefix: "trait ",
        kind: "trait",
    },
    CodeSymbolPattern {
        prefix: "impl ",
        kind: "impl",
    },
    CodeSymbolPattern {
        prefix: "mod ",
        kind: "module",
    },
    CodeSymbolPattern {
        prefix: "type ",
        kind: "type",
    },
    CodeSymbolPattern {
        prefix: "class ",
        kind: "class",
    },
    CodeSymbolPattern {
        prefix: "interface ",
        kind: "interface",
    },
    CodeSymbolPattern {
        prefix: "function ",
        kind: "function",
    },
    CodeSymbolPattern {
        prefix: "def ",
        kind: "function",
    },
    CodeSymbolPattern {
        prefix: "func ",
        kind: "function",
    },
];

pub fn strip_declaration_modifiers(value: &str) -> &str {
    value
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("export ")
        .trim_start_matches("default ")
}

pub fn declaration_symbol_name(value: &str, pattern: &str) -> Option<String> {
    let (_, rest) = value.split_once(pattern)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let name = if pattern == "impl " {
        rest.split('{').next().unwrap_or(rest).trim()
    } else {
        rest.split(['(', '{', '<', ':', '=', ' '])
            .next()
            .unwrap_or(rest)
            .trim()
    };
    let clean = name
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != ':')
        .to_string();
    (!clean.is_empty()).then_some(clean)
}

pub fn declaration_symbol(value: &str) -> Option<(&'static CodeSymbolPattern, String)> {
    let trimmed = strip_declaration_modifiers(value);
    DECLARATION_PATTERNS.iter().find_map(|pattern| {
        declaration_symbol_name(trimmed, pattern.prefix).map(|name| (pattern, name))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        declaration_symbol, declaration_symbol_name, strip_declaration_modifiers,
        DECLARATION_PATTERNS,
    };

    #[test]
    fn strips_known_declaration_modifiers_in_existing_order() {
        assert_eq!(
            strip_declaration_modifiers(" pub async fn load_user("),
            "fn load_user("
        );
        assert_eq!(
            strip_declaration_modifiers("export default class Panel {"),
            "class Panel {"
        );
    }

    #[test]
    fn extracts_declaration_names_like_existing_callers() {
        assert_eq!(
            declaration_symbol_name("fn load_user<T>(", "fn ").as_deref(),
            Some("load_user")
        );
        assert_eq!(
            declaration_symbol_name("impl crate::User {", "impl ").as_deref(),
            Some("crate::User")
        );
    }

    #[test]
    fn maps_declaration_prefix_to_review_memory_kind() {
        let Some((pattern, name)) = declaration_symbol("pub async fn load_user(") else {
            panic!("expected declaration symbol");
        };
        assert_eq!(pattern.kind, "function");
        assert_eq!(name, "load_user");
        assert!(DECLARATION_PATTERNS
            .iter()
            .any(|pattern| pattern.prefix == "func "));
    }
}
