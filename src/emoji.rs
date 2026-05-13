#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmojiSuggestion {
    pub shortcode: String,
    pub glyph: String,
}

pub(crate) fn replace_shortcode_emoji(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find(':') else {
            out.push_str(&rest[start..]);
            return out;
        };

        let shortcode = &after_start[..end];
        if is_shortcode_body(shortcode) {
            if let Some(emoji) = emojis::get_by_shortcode(shortcode) {
                out.push_str(emoji.as_str());
            } else {
                out.push(':');
                out.push_str(shortcode);
                out.push(':');
            }
        } else {
            out.push(':');
            out.push_str(shortcode);
            out.push(':');
        }
        rest = &after_start[end + 1..];
    }

    out.push_str(rest);
    out
}

pub(crate) fn emoji_shortcode_suggestions(query: &str, limit: usize) -> Vec<EmojiSuggestion> {
    let query = query.trim_start_matches(':').trim_end_matches(':');
    if query.is_empty() || !is_shortcode_prefix(query) || limit == 0 {
        return Vec::new();
    }

    let mut suggestions = emojis::iter()
        .flat_map(|emoji| {
            emoji
                .shortcodes()
                .filter(move |shortcode| shortcode.starts_with(query))
                .map(move |shortcode| EmojiSuggestion {
                    shortcode: shortcode.to_string(),
                    glyph: emoji.as_str().to_string(),
                })
        })
        .collect::<Vec<_>>();

    suggestions.sort_by(|left, right| {
        let left_exact = left.shortcode == query;
        let right_exact = right.shortcode == query;
        right_exact
            .cmp(&left_exact)
            .then_with(|| left.shortcode.len().cmp(&right.shortcode.len()))
            .then_with(|| left.shortcode.cmp(&right.shortcode))
    });
    suggestions.truncate(limit);
    suggestions
}

fn is_shortcode_body(value: &str) -> bool {
    !value.is_empty() && is_shortcode_prefix(value)
}

fn is_shortcode_prefix(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_github_shortcodes_in_display_text() {
        assert_eq!(replace_shortcode_emoji("LGTM :+1: :rocket:"), "LGTM 👍 🚀");
        assert_eq!(replace_shortcode_emoji("keep :unknown:"), "keep :unknown:");
        assert_eq!(replace_shortcode_emoji("partial :+1"), "partial :+1");
    }

    #[test]
    fn suggests_github_shortcodes_by_prefix() {
        let suggestions = emoji_shortcode_suggestions("+", 4);
        assert!(suggestions
            .iter()
            .any(|suggestion| suggestion.shortcode == "+1" && suggestion.glyph == "👍"));
    }
}
