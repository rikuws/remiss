#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(sed -nE 's/^[[:space:]]*"version":[[:space:]]*"([^"]+)".*/\1/p' "$ROOT/manifest.json" | head -n 1)"
OUT_DIR="$ROOT/dist"
OUT="$OUT_DIR/open-in-remiss-$VERSION.zip"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

(
  cd "$ROOT"
  zip -qr "$OUT" manifest.json README.md src
)

echo "$OUT"
