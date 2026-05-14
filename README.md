# Remiss

Remiss is a native Rust/GPUI desktop app for read-only pull request review. It combines GitHub pull request metadata, local checkouts, semantic diff navigation, LSP-backed source context, and AI-generated code tours without trying to become a general editor.

## Status

Remiss is an early alpha. The core workflow is usable for local development, but onboarding and provider disclosure are still being hardened.

Remiss uses a small fork of difftastic for embedded structural diffs. See
`THIRD_PARTY_NOTICES.md` for attribution and fork details.

## Requirements

- macOS is the primary development target today.
- Rust toolchain from `rust-toolchain.toml`.
- Git.
- GitHub CLI (`gh`) authenticated with `gh auth login`.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

## Packaging

```sh
./scripts/build-app.sh
REMISS_ALLOW_DEVELOPMENT_PACKAGE=1 REMISS_SIGNING_MODE=adhoc ./scripts/package-app.sh
```

`./scripts/package-app.sh` creates `dist/remiss-<version>-macos-<arch>.dmg` and `.zip`. Downloadable release packages require a `Developer ID Application` certificate unless `REMISS_ALLOW_DEVELOPMENT_PACKAGE=1` is set for local-only testing. Tagged GitHub releases (`v*`) use the `REMISS_*` signing and notarization secrets from `.github/workflows/release.yml`.

Release builds also bundle Sparkle for in-app updates. Set `REMISS_SPARKLE_PUBLIC_ED_KEY` when building a distributable package, and set both `REMISS_SPARKLE_PUBLIC_ED_KEY` and `REMISS_SPARKLE_PRIVATE_KEY` as GitHub secrets for tagged releases. Generate/export those keys with Sparkle's `generate_keys` tool from `./scripts/ensure-sparkle.sh`.

## Data Model

Remiss uses GitHub CLI for live pull request data and caches snapshots locally. Local code intelligence prefers a checked-out repository at the pull request head, and committed file reads are cached by exact Git blob object. Worktree reads are not cached.

Large GitHub collections are paged until complete where possible. If GitHub reports more queue or pull request data than Remiss can load, the app records and displays an explicit completeness warning.

## AI Providers

Code tours may send pull request metadata, changed file lists, review comments, snippets, and a local checkout path to the selected provider. Codex tours run with a read-only sandbox and no network access. Copilot tours are constrained to read/search/glob tools.

## Managed Language Servers

Remiss can install managed language servers for supported languages. These installs may download toolchains or packages from upstream registries, so release builds should expose explicit consent, versions, and uninstall controls before broad distribution.

## Roadmap

See `PLAN.md` for the product direction and review-IDE constraints.

## Design

See `DESIGN_LANGUAGE.md` and `UI_IMPLEMENTATION_GUIDE.md` for the current UI direction and GPUI implementation rules.
