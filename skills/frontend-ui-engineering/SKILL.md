---
name: frontend-ui-engineering
description: Designs and refines production-quality GPUI interfaces. Use when building or polishing native app views, panels, split panes, lists, and other user-facing surfaces where visual hierarchy and product feel matter more than framework detail.
---

# Frontend UI Engineering (GPUI)

## Overview

Build production-quality GPUI interfaces that feel intentional, calm, and native to this app. Prioritize visual hierarchy, scannability, information density, and product polish over framework cleverness. The target is not generic AI UI; it is compact review tooling with layered surfaces, restrained accent use, and layouts shaped around real workflows.

## When to Use

- Building or revising screens in `src/views/`
- Refining layout, spacing, hierarchy, or information density
- Designing panels, sidebars, lanes, tab bars, lists, and detail views
- Improving hover, selected, loading, empty, error, or sync states
- Removing generic "AI UI" styling from a GPUI surface

## GPUI View Composition

Keep view code close to the feature, and extract shared helpers only after a pattern is clearly repeated. Favor readable composition with existing primitives like `panel()`, `ghost_button()`, `badge()`, `eyebrow()`, and the theme helpers in `src/theme.rs`.

**Prefer compositional GPUI builders over giant all-in-one widgets:**

```rust
panel().child(
    div()
        .p(px(28.0))
        .px(px(32.0))
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(eyebrow("Settings / Language Servers"))
        .child(
            div()
                .text_size(px(24.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child("Managed language servers"),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(fg_muted())
                .max_w(px(760.0))
                .child("Download or repair the LSPs this app can manage itself."),
        ),
)
```

## State and Async

Keep the technical side subordinate to the design work:

- Separate long-running work from rendering so the view reads like layout code first.
- Do not over-index on state architecture in this skill; the important part is that loading, error, empty, and ready states all preserve hierarchy and calmness.
- Avoid layout jumps during refreshes. Keep the frame stable while content updates.

## Design System Adherence

### Use the Existing Theme

Pull colors, radii, and sizing from `src/theme.rs` before inventing anything new.

- Prefer `bg_canvas()`, `bg_surface()`, `bg_overlay()`, `bg_selected()`, `hover_bg()`
- Prefer `fg_default()`, `fg_muted()`, `fg_subtle()`, `fg_emphasis()`
- Use `accent()`, `success()`, and `danger()` sparingly
- Use `radius()` and `radius_sm()` instead of ad hoc corner values
- Do not hardcode raw hex colors in views unless you are intentionally extending the theme

### Match the App's Visual Language

This app already points to a compact, GitHub-dark-inspired design language:

- Layered solid surfaces instead of gradients or glass
- Muted secondary text with one clear focal point per surface
- Compact controls and dense scan-friendly lists
- Clear selected and hover states without loud decoration
- Real product copy instead of placeholder dashboard language

### Spacing and Layout

Favor the values already common in the codebase rather than introducing one-off measurements.

- Reuse familiar sizes like `px(4.0)`, `6.0`, `8.0`, `10.0`, `12.0`, `14.0`, `16.0`, `20.0`, `24.0`, `28.0`, `32.0`, `40.0`, and `48.0`
- Prefer consistent panel padding and tighter list density for scan-heavy views
- Use `topbar_height()`, `sidebar_width()`, `file_tree_width()`, and `detail_side_width()` before adding new layout constants
- Keep stable edges and clear grouping; do not center everything just because space exists

### Typography and Copy

- Use typography to create hierarchy, not decoration
- Existing patterns are restrained: compact labels around 12-13px, section titles around 15px, major page titles around 24px
- Keep copy direct and operational: repositories, queues, reviews, sync state, install failures, empty states
- Avoid marketing tone, lorem ipsum, and generic SaaS dashboard language

### Purpose-Built Surfaces

- Panels should frame content, not show off styling
- Lanes and sidebars should help scanning and comparison
- Keep actions close to the content they affect
- Prefer left-aligned text, predictable truncation, and stable rhythm over ornamental layouts

## Avoid the AI Aesthetic

| Avoid | Why It Hurts | Prefer |
|---|---|---|
| Accent-colored everything | It makes the whole screen compete for attention | Mostly neutral surfaces with restrained accents |
| Big rounded cards everywhere | It softens the interface and removes hierarchy | `radius()` and `radius_sm()` with crisper edges |
| Gradients, glass, and heavy shadows | Decorative depth fights the content | Flat layered surfaces from the theme |
| Symmetric card grids | They ignore how review tooling is actually scanned | Lists, lanes, split panes, and sidebar/detail layouts |
| Huge empty whitespace | It lowers information density and makes tooling feel sluggish | Compact spacing with clear grouping |
| Raw hex colors in views | It creates visual drift and weakens the design system | Theme helpers from `src/theme.rs` |
| Generic dashboard stats and copy | It feels templated and disconnected from the task | Content-first layouts with real domain language |

## Desktop-First Resilience

This is a native desktop UI, so think in terms of window pressure rather than mobile breakpoints.

- Layouts should survive narrow laptop windows, normal working widths, and wide external displays
- Test long repository names, long PR titles, empty queues, large diffs, and scroll-heavy states
- Prefer split panes and stable sidebars over collapsing everything into stacked cards
- Avoid accidental horizontal overflow unless the content truly requires it

## Interaction and Accessibility

- Every interactive element should look interactive and remain keyboard reachable
- Hover, selected, disabled, and busy states should all read as different states
- Icon-only actions need clear labels or obvious affordances
- Do not rely on color alone for approval, failure, or review status; pair color with text, badges, or icons
- Empty, error, loading, and sync states are part of the design, not afterthoughts
- Preserve orientation when panels open, selections change, or content refreshes

## Loading and Motion

- Prefer stable layouts with subtle status text, row placeholders, or panel-level busy states
- Avoid full-screen spinners when content can keep its structure
- Keep animation minimal and purposeful; the app should feel calm and fast, not playful

## See Also

- `src/theme.rs`
- `src/views/root.rs`
- `src/views/sections.rs`
- `src/views/pr_detail.rs`

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "It is just internal tooling" | Internal tools are used for long stretches; density and polish directly affect speed and trust. |
| "We can style it later" | Layout and hierarchy are structural decisions, not icing. |
| "A generic dashboard is good enough" | Review workflows need purpose-built scanning and comparison, not template UI. |
| "The state work matters more than the design" | Users experience the product through the interface first. |
| "The accent color makes it feel designed" | Real polish comes from hierarchy, rhythm, and restraint. |

## Red Flags

- New raw hex colors inside views
- Large radii, gradients, glassmorphism, or shadow-heavy panels
- Oversized padding that reduces information density
- Uniform card grids where lists, lanes, or split views would scan better
- Multiple competing emphasis colors in the same surface
- Missing empty, error, loading, or sync states
- Long pages that should be split into sidebar/detail or lane-based layouts
- UI that could belong to any AI-generated SaaS template

## Verification

After designing or updating a GPUI surface:

- [ ] Uses shared theme tokens and existing helpers where appropriate
- [ ] Matches the app's compact, GitHub-dark-inspired visual language
- [ ] Makes the primary information immediately scannable and secondary information clearly subdued
- [ ] Works in narrow and wide desktop windows without awkward overflow or broken hierarchy
- [ ] Gives hover, selected, disabled, loading, empty, and error states intentional visual treatment
- [ ] Keeps keyboard interaction clear and usable
- [ ] Avoids generic "AI UI" styling and unnecessary decoration
