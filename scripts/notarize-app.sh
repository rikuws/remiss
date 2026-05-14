#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARTIFACT="${1:-}"
PROFILE="${NOTARYTOOL_PROFILE:-remiss-notary}"

if [[ -z "$ARTIFACT" && -d "$ROOT/dist" ]]; then
  ARTIFACT="$(find "$ROOT/dist" -maxdepth 1 -name 'remiss-*.dmg' -print | sort | tail -n 1)"
fi

if [[ -z "$ARTIFACT" || ! -f "$ARTIFACT" ]]; then
  echo "No DMG found. Run ./scripts/package-app.sh first, or pass the DMG path." >&2
  exit 1
fi

SIGN_DETAILS="$(codesign -dvv "$ARTIFACT" 2>&1)"
if ! grep -q "Authority=Developer ID Application" <<<"$SIGN_DETAILS"; then
  echo "The DMG is not signed with a Developer ID Application certificate." >&2
  echo "Run ./scripts/package-app.sh after installing a Developer ID Application certificate." >&2
  exit 1
fi

xcrun notarytool submit "$ARTIFACT" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$ARTIFACT"
xcrun stapler validate "$ARTIFACT"
spctl -a -vv -t open --context context:primary-signature "$ARTIFACT"
