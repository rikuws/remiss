use gpui::prelude::*;
use gpui::*;

use crate::theme::*;

pub(crate) fn user_avatar(
    login: &str,
    avatar_url: Option<&str>,
    size: f32,
    emphasized: bool,
) -> AnyElement {
    let login = login.to_string();
    let avatar_url = avatar_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);

    match avatar_url {
        Some(url) => {
            let url = avatar_image_url(&url, size);
            let inner_size = avatar_inner_size(size);
            let loading_login = login.clone();
            let fallback_login = login.clone();
            div()
                .w(px(size))
                .h(px(size))
                .rounded(px(size / 2.0))
                .overflow_hidden()
                .border_1()
                .border_color(transparent())
                .bg(if emphasized {
                    accent_muted()
                } else {
                    bg_emphasis()
                })
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    img(url)
                        .size(px(inner_size))
                        .rounded(px(inner_size / 2.0))
                        .overflow_hidden()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(move || {
                            avatar_placeholder(&loading_login, inner_size, emphasized)
                                .into_any_element()
                        })
                        .with_fallback(move || {
                            avatar_placeholder(&fallback_login, inner_size, emphasized)
                                .into_any_element()
                        }),
                )
                .into_any_element()
        }
        None => avatar_placeholder(&login, size, emphasized).into_any_element(),
    }
}

fn avatar_inner_size(size: f32) -> f32 {
    (size - 2.0).max(1.0)
}

fn avatar_image_url(url: &str, display_size: f32) -> String {
    if !url.contains("avatars.githubusercontent.com") {
        return url.to_string();
    }

    let image_size = ((display_size * 3.0).ceil() as usize).clamp(96, 256);
    let (url_without_fragment, fragment) = url
        .split_once('#')
        .map(|(url, fragment)| (url, Some(fragment)))
        .unwrap_or((url, None));
    let (base, query) = url_without_fragment
        .split_once('?')
        .unwrap_or((url_without_fragment, ""));
    let mut params = query
        .split('&')
        .filter(|param| !param.is_empty() && !param.starts_with("s="))
        .map(str::to_string)
        .collect::<Vec<_>>();
    params.push(format!("s={image_size}"));

    let mut output = if params.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", params.join("&"))
    };
    if let Some(fragment) = fragment {
        output.push('#');
        output.push_str(fragment);
    }
    output
}

fn avatar_placeholder(login: &str, size: f32, emphasized: bool) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .border_1()
        .border_color(transparent())
        .bg(if emphasized {
            accent_muted()
        } else {
            bg_emphasis()
        })
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .text_size(px((size * 0.38).max(9.0)))
        .font_family(mono_font_family())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if emphasized { accent() } else { fg_emphasis() })
        .child(login_monogram(login))
}

fn login_monogram(login: &str) -> String {
    let mut monogram = login
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if monogram.is_empty() {
        monogram.push('?');
    }
    monogram
}

#[cfg(test)]
mod tests {
    use super::{avatar_image_url, login_monogram};

    #[test]
    fn github_avatar_url_uses_display_size_without_losing_query_or_fragment() {
        let url = avatar_image_url(
            "https://avatars.githubusercontent.com/u/1?v=4&s=40#profile",
            32.0,
        );

        assert_eq!(
            url,
            "https://avatars.githubusercontent.com/u/1?v=4&s=96#profile"
        );
    }

    #[test]
    fn github_avatar_url_clamps_large_display_size() {
        let url = avatar_image_url("https://avatars.githubusercontent.com/u/1", 120.0);

        assert_eq!(url, "https://avatars.githubusercontent.com/u/1?s=256");
    }

    #[test]
    fn login_monogram_uses_first_ascii_alphanumeric_characters() {
        assert_eq!(login_monogram("_riku-w"), "RI");
        assert_eq!(login_monogram("!!!"), "?");
    }
}
