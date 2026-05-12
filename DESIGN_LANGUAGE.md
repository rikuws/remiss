# Remiss Design Language

## Direction

Remiss uses a bright workbench design language for serious code review. The product should feel like native productivity software: soft, precise, command-driven, and calm during long review sessions.

The style is:

- light-first, with an equally polished dark theme
- sans-first, with rare brand title moments
- ink-forward for primary actions, with phthalo reserved for brand accents and blue reserved for keyboard focus
- roomy around the shell and panels
- practical and dense inside code, diff, and source views
- colorful only where semantic state, focus, routing, or attention needs it

The interface should feel useful before it feels decorative. Most screens should make it obvious what changed, where the reviewer is, and what action is available next.

## Principles

### 1. Workbench Before Showcase

Remiss is a review workspace, not a campaign page. Layouts should prefer rails, panes, rows, toolbars, inspectors, command palettes, and stable scroll regions.

Use roomy spacing to clarify structure, not to make screens sparse. A reviewer should be able to scan queues, files, threads, route state, and code without fighting empty space.

### 2. Soft Precision

Surfaces should be quiet and rounded, with thin borders and subtle depth. The app should look approachable without becoming bubbly or toy-like.

Use:

- large soft shells for command palette and modal-like overlays
- medium-radius panels and list rows
- compact radii for code-adjacent controls
- low-contrast borders
- neutral selected and emphasis surfaces, with phthalo reserved for brand accents and blue reserved for explicit focus edges

Avoid heavy outlines, hard black dividers, sharp stacked boxes, and nested panel clutter.

### 3. Workflow Objects

Important workflow objects should feel like compact native workbench surfaces: a clear title, subdued metadata, calm grouping, and an obvious action. Prefer neutral surfaces, subtle depth, stable spacing, and small semantic edges over decorative treatment.

Legacy abstract material or shader accents may still exist in older overview surfaces, but they are transitional. Do not introduce them in new UI, and remove them when touching an existing surface unless the accent has a clear workflow role that cannot be expressed with normal theme roles.

Avoid using borders as the primary way to make everything visible. Key objects should hold together through surface, shadow, spacing, and restrained emphasis.

### 4. Phthalo Is Brand, Blue Is Focus

Phthalo green is the product accent. Use it for identity, local-review accents, waypoint treatment, and occasional non-semantic emphasis where the app should feel distinctly Remiss.

Blue is an interaction color, not the product color. It means keyboard focus and current keyboard attention.

Semantic colors are separate:

- green for success, approval, added code, and ready states
- red for danger, removal, failed states, and change requests
- amber for warnings, queued attention, and medium priority
- cyan or violet only for category signals where needed

Primary commands use ink or its inverse. Do not make every badge, icon, selected row, or CTA phthalo. Color should help a reviewer find state quickly.

### 5. Sans First

The UI voice is a clean system sans. Use it for navigation, list rows, panels, buttons, settings, command results, and PR metadata.

Use mono only for code, paths, shortcuts, counts, hashes, technical identifiers, and diff metadata.

A serif may appear in rare brand moments, but not in normal workflow controls or dense review surfaces.

### 6. Texture Is Legacy

Abstract color, texture, and shader treatments are legacy accents, not a core identity for new Remiss surfaces. New UI should use theme roles first: neutral fills, semantic chips, active-route edges, and explicit focus treatment.

Do not place image texture, shader surfaces, or decorative material behind code, diff hunks, source browsing, file trees, or dense side panels. Work surfaces should remain readable in both themes.

## Theme Roles

The theme should expose roles, not one-off colors:

- `canvas`: app background
- `surface`: primary panes and shell regions
- `surface_elevated`: command palette, popovers, focused panels
- `inset`: code-adjacent or recessed regions
- `subtle`: quiet grouped areas
- `selected`: neutral selected row or active route surface
- `hover`: pointer hover wash
- `border_default`: visible frame
- `border_muted`: low-contrast separator
- `text_emphasis`: primary text
- `text_default`: standard body text
- `text_muted`: secondary text
- `text_subtle`: labels and disabled-adjacent metadata
- `accent`: phthalo brand accent for non-semantic emphasis
- `focus`: blue keyboard focus color
- `success`, `warning`, `danger`, `info`: semantic signals

Both light and dark themes must use the same roles. Dark theme should feel like the same product, not a separate skin.

## Surface Rules

- App shell: quiet rail, clear active section, low visual noise.
- Command palette: large soft shell, prominent search, grouped results, neutral selected row, visible focus and shortcut hints.
- Queues and lists: roomy fixed-height rows, clear titles, subdued metadata, small status chips.
- PR workspace: compact work header when reviewing, softer expanded overview when entering a PR.
- Diff workspace: practical density, high code contrast, restrained chrome, no decorative backgrounds behind code.
- Context panels: inspector-like hierarchy with label/value rows, thread summaries, and stable empty/loading/error states.
- Settings: utility surface with clear groups and no marketing copy.

## Motion

Motion should make state changes understandable:

- fades for overlays
- short selection transitions
- measured header compacting
- subtle pane or popover reveals

Avoid bounce, overshoot, long delays, and decorative motion that slows typing, navigation, or selection.

## Avoid

- generic AI app styling
- marketing-page layouts inside the desktop app
- rainbow badges without semantic meaning
- texture, shaders, or imagery behind code
- uniform card grids where lists or panes scan better
- raw colors in views when a theme token should exist
- serif-heavy workflow screens
- tiny low-contrast controls
- hidden keyboard focus
