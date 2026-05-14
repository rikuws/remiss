#!/usr/bin/env bash

remiss_identity_names() {
  security find-identity -p codesigning -v 2>/dev/null \
    | sed -nE 's/^ *[0-9]+\) [A-Fa-f0-9]{40} "(.+)".*$/\1/p'
}

remiss_find_identity_with_prefix() {
  local prefix="$1"
  local identity

  while IFS= read -r identity; do
    case "$identity" in
      "$prefix"*)
        printf '%s\n' "$identity"
        return 0
        ;;
    esac
  done < <(remiss_identity_names)

  return 1
}

remiss_missing_identity_message() {
  local wanted="$1"

  {
    echo "No $wanted code signing identity was found in your keychain."
    echo "Available code signing identities:"
    security find-identity -p codesigning -v 2>/dev/null || true
  } >&2
}

remiss_resolve_sign_identity() {
  local explicit="${REMISS_CODESIGN_IDENTITY:-${CODE_SIGN_IDENTITY:-}}"
  local mode="${REMISS_SIGNING_MODE:-auto}"
  local identity=""

  if [[ -n "$explicit" ]]; then
    printf '%s\n' "$explicit"
    return 0
  fi

  case "$mode" in
    auto)
      identity="$(remiss_find_identity_with_prefix "Developer ID Application:")" || true
      if [[ -n "$identity" ]]; then
        printf '%s\n' "$identity"
        return 0
      fi

      identity="$(remiss_find_identity_with_prefix "Apple Development:")" || true
      if [[ -n "$identity" ]]; then
        printf '%s\n' "$identity"
        return 0
      fi

      identity="$(remiss_find_identity_with_prefix "Mac Developer:")" || true
      if [[ -n "$identity" ]]; then
        printf '%s\n' "$identity"
        return 0
      fi

      printf '%s\n' "-"
      ;;
    developer-id)
      identity="$(remiss_find_identity_with_prefix "Developer ID Application:")" || true
      if [[ -z "$identity" ]]; then
        remiss_missing_identity_message "Developer ID Application"
        return 1
      fi
      printf '%s\n' "$identity"
      ;;
    development)
      identity="$(remiss_find_identity_with_prefix "Apple Development:")" || true
      if [[ -z "$identity" ]]; then
        identity="$(remiss_find_identity_with_prefix "Mac Developer:")" || true
      fi
      if [[ -z "$identity" ]]; then
        remiss_missing_identity_message "Apple Development"
        return 1
      fi
      printf '%s\n' "$identity"
      ;;
    adhoc)
      printf '%s\n' "-"
      ;;
    *)
      echo "Unknown REMISS_SIGNING_MODE '$mode'. Use auto, developer-id, development, or adhoc." >&2
      return 1
      ;;
  esac
}

remiss_print_signing_choice() {
  local identity="$1"

  if [[ "$identity" == "-" ]]; then
    echo "Signing Remiss with ad hoc identity (-)." >&2
  else
    echo "Signing Remiss with $identity." >&2
  fi
}
