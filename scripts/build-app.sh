#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/.build/Remiss.app"
EXECUTABLE="$ROOT/target/release/remiss"
ASSET_DIR="$ROOT/assets"
VERSION="${REMISS_VERSION:-$(sed -nE 's/^version = "([^"]+)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)}"
BUILD_NUMBER="${REMISS_BUILD_NUMBER:-1}"

source "$ROOT/scripts/signing.sh"

SIGN_IDENTITY="$(remiss_resolve_sign_identity)"

cd "$ROOT"
cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"

cp "$EXECUTABLE" "$APP/Contents/MacOS/Remiss"
cp "$ASSET_DIR/brand/remiss-app-icon.icns" "$APP/Contents/Resources/AppIcon.icns"
ditto --norsrc "$ASSET_DIR" "$APP/Contents/Resources/assets"
find "$APP/Contents/Resources/assets" \( -name '.DS_Store' -o -name '._*' \) -delete

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Remiss</string>
  <key>CFBundleExecutable</key>
  <string>Remiss</string>
  <key>CFBundleIdentifier</key>
  <string>dev.rikuwikman.remiss</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Remiss</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright (c) 2026</string>
</dict>
</plist>
PLIST

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

remiss_print_signing_choice "$SIGN_IDENTITY"

SIGN_ARGS=(--force --deep --sign "$SIGN_IDENTITY")
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  SIGN_ARGS+=(--options runtime --timestamp)
fi

codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP" >/dev/null

echo "$APP"
