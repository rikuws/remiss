#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/.build/Remiss.app"
EXECUTABLE="$ROOT/target/release/remiss"
ASSET_DIR="$ROOT/assets"
VERSION="${REMISS_VERSION:-$(sed -nE 's/^version = "([^"]+)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)}"
BUILD_NUMBER="${REMISS_BUILD_NUMBER:-1}"
SPARKLE_FEED_URL="${REMISS_SPARKLE_FEED_URL:-https://github.com/rikuws/remiss/releases/latest/download/appcast.xml}"
SPARKLE_PUBLIC_ED_KEY="${REMISS_SPARKLE_PUBLIC_ED_KEY:-}"
ENABLE_SPARKLE="${REMISS_ENABLE_SPARKLE:-1}"
REQUIRE_SPARKLE="${REMISS_REQUIRE_SPARKLE:-0}"

source "$ROOT/scripts/signing.sh"

SIGN_IDENTITY="$(remiss_resolve_sign_identity)"

cd "$ROOT"
cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"
mkdir -p "$APP/Contents/Frameworks"

cp "$EXECUTABLE" "$APP/Contents/MacOS/Remiss"
cp "$ASSET_DIR/brand/remiss-app-icon.icns" "$APP/Contents/Resources/AppIcon.icns"
ditto --norsrc "$ASSET_DIR" "$APP/Contents/Resources/assets"
find "$APP/Contents/Resources/assets" \( -name '.DS_Store' -o -name '._*' \) -delete

if [[ "$ENABLE_SPARKLE" == "1" ]]; then
  SPARKLE_ROOT="$("$ROOT/scripts/ensure-sparkle.sh")"
  ditto --norsrc "$SPARKLE_ROOT/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
  find "$APP/Contents/Frameworks" \( -name '.DS_Store' -o -name '._*' \) -delete
fi

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
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key>
      <string>dev.rikuwikman.remiss</string>
      <key>CFBundleURLSchemes</key>
      <array>
        <string>remiss</string>
      </array>
    </dict>
  </array>
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
if [[ -n "$SPARKLE_PUBLIC_ED_KEY" ]]; then
  /usr/libexec/PlistBuddy -c "Add :SUFeedURL string $SPARKLE_FEED_URL" "$APP/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Add :SUPublicEDKey string $SPARKLE_PUBLIC_ED_KEY" "$APP/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Add :SUEnableAutomaticChecks bool true" "$APP/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Add :SUAutomaticallyUpdate bool false" "$APP/Contents/Info.plist"
elif [[ "$REQUIRE_SPARKLE" == "1" ]]; then
  echo "REMISS_SPARKLE_PUBLIC_ED_KEY must be set for release packages with Sparkle updates." >&2
  exit 1
fi
plutil -lint "$APP/Contents/Info.plist" >/dev/null

if [[ "$REQUIRE_SPARKLE" == "1" && ! -d "$APP/Contents/Frameworks/Sparkle.framework" ]]; then
  echo "Sparkle.framework must be bundled for release packages." >&2
  exit 1
fi

remiss_print_signing_choice "$SIGN_IDENTITY"

SIGN_ARGS=(--force --deep --sign "$SIGN_IDENTITY")
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  SIGN_ARGS+=(--options runtime --timestamp)
fi

if [[ -d "$APP/Contents/Frameworks/Sparkle.framework" ]]; then
  codesign "${SIGN_ARGS[@]}" "$APP/Contents/Frameworks/Sparkle.framework"
fi
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP" >/dev/null

echo "$APP"
