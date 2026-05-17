# Third-Party Notices

Remiss includes and depends on third-party open source software. The projects
below remain owned by their respective authors and are used under their
published licenses.

## difftastic

Remiss uses a fork of difftastic to power structural diffs.

- Upstream project: https://github.com/Wilfred/difftastic
- Upstream author: Wilfred Hughes and contributors
- License: MIT
- Fork: https://github.com/rikuws/difftastic
- Use in Remiss: in-process structural diffing for review views
- Local fork changes: exposes a Rust library API that accepts text buffers and
  returns typed structural diff data, including changed chunks and aligned line
  information.

The difftastic license notice is preserved in the fork.

## sem

Remiss uses a fork of sem's `sem-core` crate to build semantic review evidence.

- Upstream project: https://github.com/Ataraxy-Labs/sem
- Upstream author: Ataraxy Labs and contributors
- License: MIT OR Apache-2.0
- Fork: https://github.com/rikuws/sem
- Use in Remiss: embedded entity-level change extraction and semantic review
  evidence from local Git checkouts
- Local fork changes: exposes the embedded API used by Remiss and disables
  unused `git2` network features in `sem-core` for packaged app builds.

The sem license notices are preserved in the fork.
