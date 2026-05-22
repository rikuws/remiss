#!/usr/bin/env bash
set -euo pipefail

REPO="${REMISS_GITHUB_REPO:-rikuws/remiss}"

prompt_secret() {
  local prompt="$1"
  local value
  read -r -s -p "$prompt" value
  printf '\n' >&2
  printf '%s' "$value"
}

set_secret() {
  local name="$1"
  local value="$2"

  if [[ -z "$value" ]]; then
    echo "Skipping empty $name." >&2
    return
  fi

  printf '%s' "$value" | gh secret set "$name" --repo "$REPO" --app actions
}

read -r -p "Developer ID .p12 path: " certificate_path
if [[ ! -f "$certificate_path" ]]; then
  echo "No .p12 found at: $certificate_path" >&2
  exit 1
fi

certificate_password="$(prompt_secret "Developer ID .p12 password: ")"
apple_password="$(prompt_secret "Apple app-specific password: ")"

certificate_base64="$(base64 < "$certificate_path" | tr -d '\n')"

set_secret REMISS_DEVELOPER_ID_CERTIFICATE_BASE64 "$certificate_base64"
set_secret REMISS_DEVELOPER_ID_CERTIFICATE_PASSWORD "$certificate_password"
set_secret REMISS_APPLE_APP_SPECIFIC_PASSWORD "$apple_password"

gh secret list --repo "$REPO" --app actions
