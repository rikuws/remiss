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
