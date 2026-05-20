use std::{fs, path::Path};

use crate::stacks::model::LineRange;

use super::MAX_PROMPT_SNIPPET_CHARS;

pub(super) fn symbol_position_in_document(
    document: &str,
    range: Option<LineRange>,
    symbol: &str,
) -> Option<(usize, usize)> {
    if let Some(line) = range.and_then(line_from_range) {
        if let Some(column) = column_for_symbol(document, line, symbol) {
            return Some((line, column));
        }
    }

    document
        .lines()
        .enumerate()
        .find_map(|(index, line)| identifier_column(line, symbol).map(|column| (index + 1, column)))
}

pub(super) fn symbol_position_in_document_line(
    document: &str,
    line: usize,
    symbol: &str,
) -> Option<(usize, usize)> {
    column_for_symbol(document, line, symbol).map(|column| (line, column))
}

fn column_for_symbol(document: &str, line: usize, symbol: &str) -> Option<usize> {
    let line_text = document.lines().nth(line.checked_sub(1)?)?;
    identifier_column(line_text, symbol)
}

fn identifier_column(line: &str, symbol: &str) -> Option<usize> {
    let byte_index = line.find(symbol)?;
    if !identifier_bounds_match(line, byte_index, symbol.len()) {
        return None;
    }
    Some(line[..byte_index].chars().count() + 1)
}

pub(super) fn line_from_range(range: LineRange) -> Option<usize> {
    usize::try_from(range.start).ok().filter(|line| *line > 0)
}

pub(super) fn read_checkout_line(checkout_root: &Path, path: &str, line: usize) -> Option<String> {
    let text = fs::read_to_string(checkout_root.join(path)).ok()?;
    text.lines()
        .nth(line.checked_sub(1)?)
        .map(|line| trim_text(line, MAX_PROMPT_SNIPPET_CHARS))
}

pub(super) fn clean_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
        .to_string()
}

pub(super) fn is_searchable_symbol(symbol: &str) -> bool {
    symbol.len() > 2
        && symbol.chars().any(|ch| ch.is_ascii_alphabetic())
        && symbol
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == ':')
        && !symbol.split("::").any(str::is_empty)
        && !is_keyword(symbol)
}

pub(super) fn declaration_symbol(line: &str) -> Option<String> {
    let trimmed = line
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("export ")
        .trim_start_matches("default ");
    for prefix in [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "type ",
        "const ",
        "static ",
        "class ",
        "function ",
        "interface ",
        "def ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let symbol = rest
                .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
                .find(|token| is_searchable_symbol(token))?;
            return Some(symbol.to_string());
        }
    }
    None
}

pub(super) fn similar_search_token(symbol: &str) -> Option<String> {
    let parts = split_symbol_parts(symbol);
    parts
        .into_iter()
        .filter(|part| part.len() >= 4 && !is_keyword(part))
        .max_by_key(|part| part.len())
}

fn split_symbol_parts(symbol: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in symbol.chars() {
        if ch == '_' || ch == ':' || ch == '-' {
            if current.len() > 1 {
                parts.push(current.to_lowercase());
            }
            current.clear();
            continue;
        }
        if ch.is_ascii_uppercase() && !current.is_empty() {
            parts.push(current.to_lowercase());
            current.clear();
        }
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        }
    }
    if current.len() > 1 {
        parts.push(current.to_lowercase());
    }
    if parts.is_empty() && symbol.len() >= 4 {
        parts.push(symbol.to_lowercase());
    }
    parts
}

pub(super) fn contains_identifier(line: &str, symbol: &str) -> bool {
    let mut start = 0usize;
    while let Some(relative) = line[start..].find(symbol) {
        let index = start + relative;
        if identifier_bounds_match(line, index, symbol.len()) {
            return true;
        }
        start = index + symbol.len();
        if start >= line.len() {
            break;
        }
    }
    false
}

fn identifier_bounds_match(line: &str, byte_index: usize, len: usize) -> bool {
    let before = line[..byte_index].chars().next_back();
    let after = line[byte_index + len..].chars().next();
    !before.map(is_identifier_char).unwrap_or(false)
        && !after.map(is_identifier_char).unwrap_or(false)
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_keyword(symbol: &str) -> bool {
    matches!(
        symbol,
        "let"
            | "mut"
            | "pub"
            | "fn"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "type"
            | "const"
            | "static"
            | "self"
            | "Self"
            | "crate"
            | "super"
            | "return"
            | "async"
            | "await"
            | "function"
            | "class"
            | "interface"
            | "import"
            | "export"
            | "from"
            | "def"
    )
}

pub(super) fn should_skip_directory(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".next"
            | "dist"
            | "build"
            | ".swiftpm"
            | "DerivedData"
    )
}

pub(super) fn is_text_search_candidate(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension,
        "rs" | "swift"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "go"
            | "py"
            | "rb"
            | "java"
            | "kt"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "m"
            | "mm"
            | "cs"
            | "php"
            | "scala"
            | "md"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
    )
}

pub(super) fn relative_path(root: &Path, path: &Path) -> String {
    normalize_repo_path(
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .as_ref(),
    )
}

pub(super) fn normalize_repo_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

pub(super) fn trim_text(value: &str, max_length: usize) -> String {
    let normalized = value.trim();
    if normalized.chars().count() <= max_length {
        return normalized.to_string();
    }
    let truncated = normalized
        .chars()
        .take(max_length.saturating_sub(1))
        .collect::<String>();
    format!("{}...", truncated.trim_end())
}

pub(super) fn limit_text(value: impl Into<String>, max_length: usize) -> String {
    trim_text(&value.into(), max_length)
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
