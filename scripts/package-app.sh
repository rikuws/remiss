#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/.build/Remiss.app"
DIST="$ROOT/dist"
STAGING="$ROOT/.build/package/remiss"

source "$ROOT/scripts/signing.sh"

export REMISS_SIGNING_MODE="${REMISS_SIGNING_MODE:-developer-id}"
SIGN_IDENTITY="$(remiss_resolve_sign_identity)"
export REMISS_CODESIGN_IDENTITY="$SIGN_IDENTITY"

if [[ "${REMISS_ALLOW_DEVELOPMENT_PACKAGE:-0}" != "1" ]]; then
  export REMISS_REQUIRE_SPARKLE=1
fi

"$ROOT/scripts/build-app.sh" >/dev/null

if [[ "${REMISS_ALLOW_DEVELOPMENT_PACKAGE:-0}" != "1" ]]; then
  SIGN_DETAILS="$(codesign -dvv "$APP" 2>&1)"
  if ! grep -q "Authority=Developer ID Application" <<<"$SIGN_DETAILS"; then
    echo "Package builds intended for downloads must use a Developer ID Application certificate." >&2
    echo "Install that certificate, or set REMISS_ALLOW_DEVELOPMENT_PACKAGE=1 for a local-only package." >&2
    exit 1
  fi

  if ! /usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$APP/Contents/Info.plist" >/dev/null 2>&1; then
    echo "Release packages must include Sparkle's SUPublicEDKey." >&2
    echo "Set REMISS_SPARKLE_PUBLIC_ED_KEY before running ./scripts/package-app.sh." >&2
    exit 1
  fi
  if [[ ! -d "$APP/Contents/Frameworks/Sparkle.framework" ]]; then
    echo "Release packages must bundle Sparkle.framework." >&2
    exit 1
  fi
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
ARCH="$(uname -m)"
BASENAME="remiss-${VERSION}-macos-${ARCH}"
ZIP="$DIST/$BASENAME.zip"
DMG="$DIST/$BASENAME.dmg"

rm -rf "$STAGING"
mkdir -p "$DIST" "$STAGING"
rm -f "$ZIP" "$DMG"

ditto --norsrc "$APP" "$STAGING/Remiss.app"
ln -s /Applications "$STAGING/Applications"
find "$STAGING" \( -name '.DS_Store' -o -name '._*' \) -delete

ditto -c -k --norsrc --keepParent "$APP" "$ZIP"
hdiutil create -volname "Remiss" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null

DMG_SIGN_ARGS=(--force --sign "$SIGN_IDENTITY")
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  DMG_SIGN_ARGS+=(--timestamp)
fi

codesign "${DMG_SIGN_ARGS[@]}" "$DMG"
codesign --verify --verbose=2 "$DMG" >/dev/null

cat <<EOF
$DMG
$ZIP
EOF
