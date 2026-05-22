# Remiss

Remiss is a native macOS workspace for GitHub pull request review.

It is read-only by design. The app is meant for understanding a change,
navigating the surrounding code, keeping review state in one place, and leaving
clear review feedback. It is not an editor, merge tool, or chat-first code
assistant.

## Download

- [Latest macOS build](https://github.com/rikuws/remiss/releases/latest/download/remiss-latest-macos.dmg)
- [All releases](https://github.com/rikuws/remiss/releases)

Remiss is early alpha. The core review workflow is usable, but onboarding,
provider setup, and distribution are still being hardened.

## What Remiss Does

- Opens GitHub pull requests in a focused desktop review workspace.
- Combines PR metadata, changed files, review threads, and local checkout
  context.
- Provides diff navigation, structural diff views, and a read-only source
  browser for nearby code.
- Keeps review comments, status, and submission actions inside the review flow.
- Can generate Review Briefs and Guided Review routes when experimental AI
  features are explicitly enabled and a provider is configured.
- Uses managed language servers where available for richer source context.

## Review Workflow

Remiss is built around the part of review where most time is lost:
understanding what changed, finding the important context, judging risk, and
staying oriented across a longer review.

A typical review starts from a GitHub pull request, loads the PR data and local
checkout context, then moves through the diff, source browser, comments, Review
Brief, and Guided Review surfaces as needed. The center of the app is the review
workspace, not a browser tab or a prompt box.

## Boundaries

- Remiss does not edit code, stage files, merge branches, or resolve conflicts.
- macOS is the primary target today.
- Pull request data comes through the GitHub CLI, so `gh auth login` is required.
- Local code intelligence prefers a checked-out copy of the repository at the PR
  head.

## AI and Privacy

AI features are experimental, off by default, and provider-dependent. Review
Briefs and Guided Review may send pull request metadata, changed file lists,
review comments, snippets, and local checkout paths to the selected provider.
Codex/Copilot background jobs have a separate opt-in setting.

Codex-backed flows run with a read-only sandbox and no network access. Copilot
flows are constrained to read/search/glob tools. Provider setup and unavailable
states are treated as visible review states, not generic loading states.

## Notes

Remiss uses small forks of difftastic and sem for embedded structural diffs and
semantic review evidence. See `THIRD_PARTY_NOTICES.md` for attribution and fork
details.

`PLAN.md` captures the product direction. `DESIGN_LANGUAGE.md` and
`UI_IMPLEMENTATION_GUIDE.md` capture the current interface and GPUI rules.

## Development

For working on the repo locally:

```sh
cargo fmt --check
./scripts/check-line-budget.sh
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

To build a local app bundle:

```sh
./scripts/build-app.sh
```
