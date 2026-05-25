#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/capture-screenshot-macos.sh --scenario review-workspace --output screenshots/macos/review-workspace.png
USAGE
}

SCENARIO="review-workspace"
OUTPUT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scenario)
      SCENARIO="${2:-}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$OUTPUT" ]]; then
  usage
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/.build/Remiss.app"
EXECUTABLE="$APP/Contents/MacOS/Remiss"

if [[ ! -x "$EXECUTABLE" ]]; then
  echo "Expected packaged Remiss executable at $EXECUTABLE. Run REMISS_ENABLE_SPARKLE=0 ./scripts/build-app.sh first." >&2
  exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
OUTPUT_DIR="$(cd "$(dirname "$OUTPUT")" && pwd)"
OUTPUT="$OUTPUT_DIR/$(basename "$OUTPUT")"
READY_FILE="$OUTPUT.ready"
rm -f "$OUTPUT" "$READY_FILE"

PID=""
cleanup() {
  if [[ -n "${PID:-}" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

REMISS_SCREENSHOT_MODE=1 \
REMISS_SCREENSHOT_SCENARIO="$SCENARIO" \
REMISS_SCREENSHOT_OUTPUT_FILE="$OUTPUT" \
"$EXECUTABLE" &
PID="$!"

osascript -e "tell application \"System Events\" to set frontmost of first process whose unix id is $PID to true" >/dev/null 2>&1 || true

for _ in $(seq 1 160); do
  if [[ -f "$READY_FILE" ]]; then
    break
  fi
  sleep 0.25
done

if [[ ! -f "$READY_FILE" ]]; then
  echo "Remiss did not become screenshot-ready: $READY_FILE" >&2
  exit 1
fi

sleep 0.2
WINDOW_RECT="$(
  osascript <<OSA 2>/dev/null || true
tell application "System Events"
  set remissProcesses to every process whose unix id is $PID
  if (count of remissProcesses) is 0 then return ""
  tell item 1 of remissProcesses
    if (count of windows) is 0 then return ""
    set windowPosition to position of window 1
    set windowSize to size of window 1
    return (item 1 of windowPosition as integer) & "," & (item 2 of windowPosition as integer) & "," & (item 1 of windowSize as integer) & "," & (item 2 of windowSize as integer)
  end tell
end tell
OSA
)"

if [[ "$WINDOW_RECT" =~ ^-?[0-9]+,-?[0-9]+,[0-9]+,[0-9]+$ ]]; then
  screencapture -x -R "$WINDOW_RECT" "$OUTPUT"
else
  screencapture -x "$OUTPUT"
fi

if [[ ! -s "$OUTPUT" ]]; then
  echo "Screenshot was not written: $OUTPUT" >&2
  exit 1
fi

echo "$OUTPUT"
