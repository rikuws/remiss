use gpui::prelude::*;
use gpui::*;

use crate::theme::*;

pub(crate) fn build_static_tooltip(text: &'static str, cx: &mut App) -> AnyView {
    build_text_tooltip(SharedString::from(text), cx)
}

pub(crate) fn build_chrome_tooltip(text: &'static str, cx: &mut App) -> AnyView {
    build_tooltip(SharedString::from(text), TooltipTone::Chrome, cx)
}

pub(crate) fn build_text_tooltip(text: SharedString, cx: &mut App) -> AnyView {
    build_tooltip(text, TooltipTone::Emphasis, cx)
}

fn build_tooltip(text: SharedString, tone: TooltipTone, cx: &mut App) -> AnyView {
    AnyView::from(cx.new(|_| StaticTooltipView { text, tone }))
}

#[derive(Clone, Copy)]
enum TooltipTone {
    Chrome,
    Emphasis,
}

struct StaticTooltipView {
    text: SharedString,
    tone: TooltipTone,
}

impl Render for StaticTooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let tooltip = div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(radius_sm())
            .border_1()
            .border_color(transparent())
            .bg(bg_overlay())
            .shadow(tooltip_shadow())
            .text_size(px(11.0));

        match self.tone {
            TooltipTone::Chrome => tooltip.text_color(fg_default()),
            TooltipTone::Emphasis => tooltip
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg_emphasis()),
        }
        .child(self.text.clone())
    }
}
