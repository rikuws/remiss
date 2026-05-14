#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPARKLE_VERSION="2.9.1"
SPARKLE_SHA256="c0dde519fd2a43ddfc6a1eb76aec284d7d888fe281414f9177de3164d98ba4c7"
SPARKLE_URL="https://github.com/sparkle-project/Sparkle/releases/download/${SPARKLE_VERSION}/Sparkle-${SPARKLE_VERSION}.tar.xz"
SPARKLE_CACHE="$ROOT/.build/sparkle"
SPARKLE_ROOT="$SPARKLE_CACHE/Sparkle-${SPARKLE_VERSION}"
SPARKLE_ARCHIVE="$SPARKLE_CACHE/Sparkle-${SPARKLE_VERSION}.tar.xz"

if [[ -x "$SPARKLE_ROOT/bin/generate_appcast" && -d "$SPARKLE_ROOT/Sparkle.framework" ]]; then
  printf '%s\n' "$SPARKLE_ROOT"
  exit 0
fi

mkdir -p "$SPARKLE_CACHE"

if [[ ! -f "$SPARKLE_ARCHIVE" ]]; then
  curl --fail --location --silent --show-error "$SPARKLE_URL" --output "$SPARKLE_ARCHIVE"
fi

ACTUAL_SHA256="$(shasum -a 256 "$SPARKLE_ARCHIVE" | awk '{print $1}')"
if [[ "$ACTUAL_SHA256" != "$SPARKLE_SHA256" ]]; then
  rm -f "$SPARKLE_ARCHIVE"
  echo "Sparkle archive checksum mismatch." >&2
  echo "Expected: $SPARKLE_SHA256" >&2
  echo "Actual:   $ACTUAL_SHA256" >&2
  exit 1
fi

TMP_ROOT="$SPARKLE_ROOT.tmp.$$"
rm -rf "$TMP_ROOT"
mkdir -p "$TMP_ROOT"
tar -xf "$SPARKLE_ARCHIVE" -C "$TMP_ROOT"

if [[ ! -x "$TMP_ROOT/bin/generate_appcast" || ! -d "$TMP_ROOT/Sparkle.framework" ]]; then
  rm -rf "$TMP_ROOT"
  echo "Downloaded Sparkle distribution is missing expected tools or framework." >&2
  exit 1
fi

rm -rf "$SPARKLE_ROOT"
mv "$TMP_ROOT" "$SPARKLE_ROOT"

printf '%s\n' "$SPARKLE_ROOT"
