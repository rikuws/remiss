# Visual Design Brief

## Purpose

This document defines the visual rules for the next phase of Remiss.
It is intended to stop the product from drifting back into generic "developer tool dark mode"
while still protecting the clarity and speed required for serious code review work.

The goal is not to make the product decorative. The goal is to make it feel authored,
editorial, cinematic, and premium without compromising legibility or navigation.

## Core Direction

The design language is:

- editorial, not dashboard-first
- cinematic, not flat
- restrained, not loud
- research-grade, not startup-generic
- software-native, but with cultural/art-direction cues

The strongest reference pattern is the tension between:

- classical typography
- painterly or photographic atmospheres
- precise technical overlays
- quiet, premium interface chrome

Short name for the direction:

- `Cinematic Editorial Tech`

Working interpretation for this product:

- `A serious review IDE shaped like an editorial artifact`

## Product Translation

This is a desktop code review product, not a marketing site. That matters.

We should borrow the mood from the references, but apply it selectively:

- overview, empty states, PR header, settings, and brand surfaces can carry the strongest mood
- core review surfaces must stay quieter and more functional
- code, diff, and navigation areas should inherit the new tone through materials, spacing, typography hierarchy, and restrained texture, not through heavy illustration behind code

The product should feel like:

- an intentional review studio
- a high-trust analysis environment
- a native tool with taste

It should not feel like:

- a browser wrapper
- a neon AI app
- a generic GitHub client
- a figma-shot SaaS landing page applied directly to code

## Design Principles

### 1. Typography creates the drama

The references rely on typography scale, proportion, and contrast more than on UI decoration.

Rules:

- Use a high-contrast serif as the display voice for hero titles, empty states, major headers, and selected section titles.
- Use a restrained sans for interface copy, labels, controls, and long-form UI text.
- Keep monospaced type for code, metadata, shortcuts, counts, and technical labels.
- Do not use the display serif for dense utility UI or code-adjacent metadata.
- Large headlines should feel elegant, spacious, and deliberate rather than bold and blocky.

Typography roles:

- Display: editorial serif, high contrast, used sparingly
- UI Sans: quiet sans, neutral, readable at small sizes
- Mono: existing code voice for code and technical utilities

### 2. Mood comes from atmosphere, not gradients

The visual tone should come from image-based atmosphere and material texture.

Rules:

- Prefer mist, grain, paper, bloom, scan noise, watercolor wash, field patterns, mesh topography, and low-contrast overlays.
- Avoid loud app-store gradients and glossy glassmorphism.
- Use light bloom and halo sparingly to imply computation or focus.
- Prefer matte surfaces with occasional glow accents over shiny surfaces everywhere.

### 3. Use asymmetry and negative space

The references feel premium because they are not packed edge-to-edge with cards.

Rules:

- Let major surfaces breathe.
- Use offset composition and controlled imbalance in overview and header layouts.
- Allow one dominant focal element per screen.
- Avoid equal-weight card grids as the default organizing pattern.

### 4. Make software feel indirect and intelligent

The references rarely show literal dashboards. They imply capability through signals and fragments.

Rules:

- Prefer symbolic overlays, diagram traces, graph fields, subtle map lines, and ambient UI fragments.
- Show "intelligence" as structure and atmosphere, not as magic sparkles or robot imagery.
- Keep technical motifs abstract and precise.

### 5. Restraint is the premium signal

Rules:

- One accent family is enough.
- One dominant image or texture per major surface is enough.
- Strong serif moments should be rare enough that they still feel special.
- Motion should feel ambient and structural, never busy.

## Visual Tokens

These are not final implementation values, but they define the intended system.

### Palette

Base palette direction:

- Deep forest / black-green for primary dark canvas
- Ink / charcoal for working surfaces
- Bone / parchment / fog for light surfaces and contrast moments
- Muted brass, soft moss, pale cyan, or cold moonlight as accent families

Rules:

- Replace the current blue-purple primary feel with a darker, more natural palette.
- Purple should not be the brand accent.
- Accent colors should behave like signals, not like the whole theme.
- Success and danger states can remain functional, but should be slightly softened.

Suggested token families:

- `canvas`: near-black green or ink
- `surface`: charcoal, smoked olive, graphite
- `surface-elevated`: slightly warmer or lighter than the base surface
- `text-primary`: warm ivory or soft gray
- `text-secondary`: desaturated stone
- `line`: translucent warm-gray or pale moss
- `accent`: pale sage, muted cyan, or dim gold
- `highlight`: soft halo white with limited bloom usage

### Radii

Rules:

- Increase major surface radii slightly to feel more framed and gallery-like.
- Keep code blocks and dense utility controls tighter than hero or shell surfaces.
- Avoid toy-like over-rounding.

### Borders

Rules:

- Use thin, low-contrast borders.
- Favor frame-like containment over obvious card outlines.
- Inner contrast should come from tone shifts, not thick borders.

### Shadows and glow

Rules:

- Prefer soft ambient separation over obvious drop shadows.
- Use localized bloom around art, focus states, or symbolic overlays.
- Do not use strong shadow stacks on every panel.

## Typography Rules

### Display Serif

Use for:

- overview welcome/title treatment
- PR title treatment in expanded state
- empty states
- auth or onboarding screens
- settings section hero copy

Do not use for:

- sidebar navigation
- buttons
- tab labels
- file lists
- dense metadata
- diff annotations

Desired qualities:

- high contrast
- elegant curves
- editorial feel
- readable at large sizes

### UI Sans

Use for:

- navigation
- body text
- buttons
- helper copy
- settings
- inspectors

Desired qualities:

- quiet
- slightly humanist or refined grotesk
- not overly geometric
- excellent small-size readability

### Mono

Use for:

- code
- counts
- paths
- repository names
- shortcut hints
- structured review metadata

Rules:

- Mono remains functional and technical.
- Mono should become the "precision layer," not the main decorative voice.

## Image Rules

Images should not feel like stock SaaS illustrations.

Preferred image types:

- cinematic photography with haze, darkness, and selective glow
- painterly landscapes with technical overlays
- abstract topographic meshes
- scientific diagrams over atmospheric backgrounds
- tactile collage with paper, grids, and print textures

Avoid:

- literal robots
- floating hologram dashboards
- bright cyberpunk scenes
- glossy 3D blobs
- corporate vector illustration
- generic gradient wallpaper

Processing direction:

- add grain
- slightly soften edges
- preserve darkness
- keep saturation controlled
- allow one focal glow or halo
- favor imperfect texture over sterile cleanliness

## Layout Rules

### Shell

Rules:

- The app shell should feel like a framed workspace, not like nested default panels.
- The sidebar should feel quieter, more architectural, and less like a list of buttons.
- The workspace header should feel authored, with a clear change from expanded editorial mode to compact working mode.

### Overview

Rules:

- The overview should become the most expressive screen in the product.
- It should use a hero treatment, not just counters and panels.
- Queue summaries should still be functional, but the composition should have a dominant focal area and a calmer secondary information band.

### PR Header

Rules:

- Expanded state can carry a strong editorial mood.
- Compact state must collapse into a disciplined work header.
- The transition between those states should feel intentional and smooth.

### Diff Workspace

Rules:

- The diff workspace must remain mostly utilitarian.
- Express the new design through spacing, hierarchy, materials, and subtle texture only.
- Do not put dramatic imagery behind code.
- Let the toolbar and side panels carry more of the visual identity than the code pane itself.

## Motion Rules

Rules:

- Motion should be slow enough to feel premium, fast enough not to interrupt work.
- Prefer fades, subtle parallax, atmospheric drift, and measured size transitions.
- Avoid bounce, springy overshoot, or flashy hover choreography.
- Code review actions should still feel crisp and immediate.

Good motion targets:

- PR header compacting
- tab changes
- palette backdrop fade
- subtle spotlight or halo drift on hero surfaces
- hover states that slightly lift tone rather than jump

## Component Rules

### Buttons

Rules:

- Primary buttons should feel substantial and calm.
- Secondary buttons should feel quiet and editorial, not flat gray pills.
- Avoid bright filled accent buttons as the default.

### Pills and badges

Rules:

- Use more restrained badges with softer contrast.
- Technical state pills should feel integrated into the palette.
- Avoid candy-color badge clusters.

### Panels

Rules:

- Panels should feel like framed editorial blocks, not generic cards.
- Reduce the sense of "many small containers."
- Reserve stronger contrast panels for important focus zones.

### Sidebar

Rules:

- Lower the visual chatter.
- Use typography, spacing, and subtle active indicators instead of obvious button chrome.
- Counts and statuses should feel like annotations, not alerts.

### Tabs

Rules:

- Tabs should feel like section routing inside a review workspace, not browser tabs.
- Reduce their resemblance to generic IDE tab strips where possible.

## Non-Negotiables

These rules should hold even as the design evolves:

- No purple-led brand direction
- No default SaaS gradient aesthetic
- No generic glassmorphism
- No heavy illustration behind code
- No dense card soup
- No overuse of the display serif
- No novelty AI imagery
- No sacrificing review readability for mood

## Implementation Plan

This is the first-pass plan for the current codebase.

### Phase 1. Rebuild the theme system around the new art direction

Goal:

- Replace the current cool blue / purple developer-tool palette with a cinematic editorial palette.

Primary files:

- `/Users/rikuwikman/Dev/gh-ui/src/theme.rs`
- `/Users/rikuwikman/Dev/gh-ui/src/main.rs`
- `/Users/rikuwikman/Dev/gh-ui/assets/fonts`

Changes:

- Introduce new color tokens for canvas, surface, elevated surface, text tiers, line tiers, accent, glow, and atmospheric backdrop.
- Separate "brand accent" from functional success/danger colors.
- Add typography tokens instead of directly mixing system font and mono everywhere.
- Add at least one bundled display serif and one refined UI sans, then map them into reusable font roles.
- Tune radii, borders, and overlay values for a framed, premium shell.

### Phase 2. Redesign the shell before touching every feature

Goal:

- Change the product's first impression quickly without destabilizing the review flows.

Primary files:

- `/Users/rikuwikman/Dev/gh-ui/src/views/root.rs`
- `/Users/rikuwikman/Dev/gh-ui/src/views/sections.rs`
- `/Users/rikuwikman/Dev/gh-ui/src/app_assets.rs`
- `/Users/rikuwikman/Dev/gh-ui/assets/icons`
- `/Users/rikuwikman/Dev/gh-ui/assets/brand`

Changes:

- Rework the sidebar into a quieter architectural rail.
- Redesign the workspace tab strip to feel less like stock IDE tabs.
- Update shared primitives like panels, buttons, badges, and eyebrows to reflect the new system.
- Refresh icons so they fit the calmer, more editorial tone.
- Add ambient background and framing assets where appropriate.

### Phase 3. Turn the overview into the brand-defining screen

Goal:

- Make the overview screen the clearest statement of the new visual direction.

Primary files:

- `/Users/rikuwikman/Dev/gh-ui/src/views/sections.rs`

Changes:

- Replace the current "welcome plus stat cards" composition with a hero-led overview.
- Introduce a dominant headline treatment, atmospheric art, and clearer hierarchy between focal content and operational content.
- Keep queue data and quick actions, but compose them as a secondary band instead of equal-weight cards everywhere.
- Use generated imagery here more strongly than anywhere else in the app.

### Phase 4. Give the PR workspace an editorial header and disciplined compact mode

Goal:

- Make PR entry feel authored while preserving a highly usable working state.

Primary files:

- `/Users/rikuwikman/Dev/gh-ui/src/views/pr_detail.rs`

Changes:

- Redesign the expanded PR header with editorial type, stronger hierarchy, and optional lightweight atmospheric art.
- Keep the compact state tight, quiet, and work-focused.
- Improve surface tabs so they read like mode switches in a review workspace rather than utility chips.
- Reframe badges and metadata rows with softer, more premium contrast.

### Phase 5. Quietly restyle the diff workspace

Goal:

- Bring the new direction into the core review workflow without making code harder to read.

Primary files:

- `/Users/rikuwikman/Dev/gh-ui/src/views/diff_view.rs`
- `/Users/rikuwikman/Dev/gh-ui/src/code_display.rs`
- `/Users/rikuwikman/Dev/gh-ui/src/source_browser.rs`

Changes:

- Rework toolbar hierarchy and spacing.
- Introduce restrained texture or tonal depth around toolbars and side panels, not in the code background.
- Refine hunk headers, semantic section headers, and review panels to feel less mechanical.
- Keep code backgrounds disciplined and low-noise.
- Rebalance diff success/danger colors to fit the new palette.

### Phase 6. Refresh modal and utility surfaces

Goal:

- Make the command palette, settings, and auxiliary screens feel coherent with the new system.

Primary files:

- `/Users/rikuwikman/Dev/gh-ui/src/views/palette.rs`
- `/Users/rikuwikman/Dev/gh-ui/src/views/settings.rs`
- `/Users/rikuwikman/Dev/gh-ui/src/views/tour_view.rs`

Changes:

- Give the palette a more cinematic backdrop and quieter result rows.
- Turn settings into a proper editorial utility surface rather than a plain form stack.
- Restyle the tour surface so it feels like a guided route, not a disconnected feature.

## Priority Order

If we want the most visible improvement with the least product risk, the order should be:

1. Theme tokens and typography roles
2. Shared primitives and shell
3. Overview screen
4. PR header
5. Diff workspace refinements
6. Palette, settings, and utility surfaces

## Asset List To Generate

We do not need dozens of images. We need a small, disciplined set.

Priority assets:

1. Overview hero artwork
2. PR header artwork set
3. Ambient texture pack
4. Mesh / topography abstract set
5. Empty-state editorial illustrations

Use cases:

- overview hero
- expanded PR header background or side art
- settings / onboarding / auth mood panels
- palette or modal backdrops
- sparse decorative inserts for empty states

## Prompt Rules For Your Image Model

Apply these rules to every prompt unless there is a good reason not to:

- editorial technology mood
- cinematic lighting
- restrained palette
- natural texture and grain
- premium print / magazine art direction
- subtle computational motifs
- no literal UI screenshots unless explicitly requested
- no generic startup gradient aesthetic
- no robots, no neon cyberpunk, no corporate illustration

Recommended prompt suffix:

`moody editorial technology aesthetic, premium art direction, restrained palette, realistic texture, subtle film grain, atmospheric depth, soft bloom, elegant composition, not stock, not cyberpunk, not glossy 3d, no text, no watermark`

## Image Prompts

### 1. Overview Hero

Use for the overview screen's dominant visual.

Prompt:

`A cinematic editorial scene for a premium software product about code review and technical judgment: two or three human figures in a misty natural landscape at dusk, quietly collaborating with laptops, soft halo light around them, deep forest and black-green tones, large areas of negative space, subtle sense of intelligence and analysis without showing a literal dashboard, photographed like a luxury magazine cover, moody editorial technology aesthetic, premium art direction, restrained palette, realistic texture, subtle film grain, atmospheric depth, soft bloom, elegant composition, not stock, not cyberpunk, not glossy 3d, no text, no watermark`

Suggested aspect ratios:

- `16:10`
- `3:2`

### 2. Abstract Computation Field

Use for PR headers, settings hero, or section dividers.

Prompt:

`An abstract computational landscape made of glowing topographic mesh lines floating over dark terrain, faint mist, pale cyan and moss highlights over an ink-black and deep green base, elegant and mysterious, like a scientific visualization from a future research journal, minimal but atmospheric, premium editorial technology aesthetic, restrained palette, subtle film grain, soft bloom, realistic texture, no text, no watermark`

Suggested aspect ratios:

- `21:9`
- `16:9`

### 3. Painterly Technical Collage

Use for settings, onboarding, or editorial blocks.

Prompt:

`A refined collage combining watercolor paper texture, celestial diagram lines, technical drafting marks, print-era grids, and a luminous abstract symbol at the center, in cream, muted coral, dusty blue, navy, and soft gold, tactile and imperfect like a museum poster mixed with future software branding, highly composed, premium editorial art direction, subtle grain, no text, no watermark`

Suggested aspect ratios:

- `4:3`
- `3:2`

### 4. Dark Security / Structure Motif

Use for review route, security-sensitive states, or feature panels.

Prompt:

`A dark abstract structural pattern suggesting fracture maps, cellular glass, network topology, and software integrity, white or pale mineral lines over a near-black surface with faint blue-gray internal depth, minimal, stark, elegant, high-contrast editorial technology artwork, like a gallery print for a software research lab, subtle texture, no text, no watermark`

Suggested aspect ratios:

- `4:3`
- `1:1`

### 5. Ambient Empty-State Illustration

Use for moments with no PR selected, no queue items, or disconnected auth states.

Prompt:

`A quiet atmospheric scene representing waiting, orientation, and technical focus: distant lights in fog, a single workstation glow, abstract signals in the air, deep green-black palette with warm ivory highlights, emotionally calm and thoughtful, cinematic editorial photography feel, premium art direction, restrained palette, subtle grain, no text, no watermark`

Suggested aspect ratios:

- `3:2`
- `16:10`

### 6. Texture Pack: Paper / Grain / Wash

Use as subtle overlays across the shell.

Prompt:

`A seamless high-resolution texture sheet with soft paper grain, faint watercolor bloom, print-era noise, and very subtle tonal variation in warm gray, bone, moss-gray, and charcoal, elegant and understated, designed as a premium editorial background texture for software UI, minimal contrast, no objects, no text, no watermark`

Suggested output:

- `2048px or larger`
- generate both light and dark variants

### 7. Symbolic Graph / Route Artwork

Use for review-route or guided-tour surfaces.

Prompt:

`An abstract navigational diagram for a code review product: drifting nodes, route lines, call-graph inspired structure, sparse labels implied but not actually rendered, elegant geometry layered over dark atmospheric texture, muted ivory, pale sage, and cold cyan on black-green, intelligent and calm rather than flashy, premium editorial technology artwork, no text, no watermark`

Suggested aspect ratios:

- `16:9`
- `5:4`

## Negative Prompt Suggestions

Use these when your model supports negative prompts:

- `text, letters, watermark, logo, generic app screenshot, glossy ui, cyberpunk, neon purple, robot, anime, cartoon, vector illustration, plastic 3d render, oversaturated colors, corporate stock photo, busy composition, low-detail faces, cheap sci-fi`

## Review Checklist

Before approving any new visual work, check:

- Does it feel editorial rather than generic SaaS?
- Does it preserve clarity for a real code review workflow?
- Is the palette restrained?
- Is the serif used sparingly and with intent?
- Is the imagery atmospheric rather than literal?
- Does the shell feel calmer and more premium?
- Would this still feel credible for engineers using it for hours?

If the answer to any of those is no, revise before shipping.
