---
name: gh-ui-visual-verification
description: Visually verify Remiss GPUI changes in this repo. Use after changing review surfaces, titlebar controls, sidebars, file trees, animations, or any native UI path where cargo check is not enough.
---

# GH-UI Visual Verification

Use this skill after GPUI UI changes in this repo. Do not stop at `cargo check` when the user reported a visual regression or when the change affects layout, animation, clipping, titlebar controls, sidebars, file trees, panes, or review-mode switching.

## Core Workflow

1. Build-check first.
   - Run `cargo fmt`.
   - Run `cargo check`.
   - If either fails, fix that before launching the app.

2. Open the exact screen under test.
   - Prefer the app's debug-open path for PR review screens:
     ```sh
     REMISS_DEBUG_OPEN_PR='owner/repo#number' cargo run
     ```
   - Use a cached PR that exercises the surface. To discover cached PRs:
     ```sh
     sqlite3 "$HOME/Library/Application Support/remiss/cache.sqlite3" \
       "select key from documents where key like 'pr-detail-%' or key like 'review-session-%' order by fetched_at_ms desc limit 40;"
     ```
   - For the review file-tree path, `REMISS_DEBUG_OPEN_PR='rikuws/haastis#6' cargo run` has been a useful local fixture when present in the cache.
   - If you only see the overview dashboard, you have not verified the review surface.

3. Bring the Remiss window forward before screenshotting.
   - Check visible process names when needed:
     ```sh
     osascript -e 'tell application "System Events" to get name of processes whose visible is true'
     ```
   - Raw `cargo run` windows may appear as process `remiss`; make it frontmost:
     ```sh
     osascript -e 'tell application "System Events" to set frontmost of process "remiss" to true'
     ```
   - Do not trust a screenshot if Warp, another terminal, or the overview dashboard is still in front.

4. Capture and inspect the real UI.
   - Take a screenshot:
     ```sh
     screencapture -x /tmp/gh-ui-visual-check.png
     ```
   - Open it with the available image viewer tool and inspect the changed surface.
   - Verify the intended screen, not just that the app launched.

5. Exercise the changed interaction.
   - For titlebar toggles or sparse accessibility trees, screen-coordinate clicks may be necessary:
     ```sh
     osascript -e 'tell application "System Events" to click at {x, y}'
     ```
   - Capture another screenshot after the interaction.
   - For animations, verify both endpoints and whether the transition leaves clipped, blank, or stale content.

6. Clean up.
   - Stop the `cargo run` session before finishing.
   - Report what was visually checked and any gap, for example: "verified PR file-tree view via debug-open" or "screenshot only reached overview, not sufficient."

## What To Check For Review-Surface Changes

- The left review pane is full height and scrollable.
- The file tree shows both header and rows; no vertical clipping after animation wrappers.
- The diff/source pane starts at the expected x-position after hide/show.
- Titlebar buttons are adjacent, aligned, and keep the shared chrome style.
- Hide/show leaves no orphan border, blank gutter, or invisible pane stealing space.
- The screenshot is from a PR file view with `Review` and `Code` selected when checking file-tree behavior.

## Common Failure Modes

- `cargo check` passes but an animated wrapper lacks `h_full()`, `min_h_0()`, or the right flex direction, causing vertical clipping.
- The screenshot captures Warp or another foreground app instead of Remiss.
- The app opens on the overview dashboard, so the broken review pane is never exercised.
- A titlebar click lands on the wrong button because macOS coordinates include the menu bar and window chrome.
- The UI is correct at rest but broken while collapsed or expanded because the child was unmounted instead of clipped.
