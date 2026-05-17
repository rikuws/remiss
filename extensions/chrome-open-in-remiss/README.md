# Open in Remiss

Chrome extension that adds an `Open in Remiss` button to GitHub pull request
pages.

The extension opens:

```text
remiss://github/{owner}/{repo}/pull/{number}
```

Remiss handles that URL by loading the matching pull request in the desktop app.

## Development

Load this directory as an unpacked extension in Chrome:

```sh
chrome://extensions
```

Enable Developer Mode, choose `Load unpacked`, and select this directory.

## Packaging

```sh
bash scripts/package.sh
```

The extension version is owned by `manifest.json` and does not need to match the
Remiss desktop app version.
