#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/check-line-budget.sh [--max-lines N] [--allowlist PATH]

Checks Rust source files against a maximum line budget.

Options:
  --max-lines N     Maximum allowed lines per file. Defaults to 3000.
  --allowlist PATH  Paths allowed to exceed the budget temporarily.
                    Defaults to scripts/line-budget-allowlist.txt.
USAGE
}

max_lines=3000
allowlist_path="scripts/line-budget-allowlist.txt"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --max-lines)
      if [[ $# -lt 2 || ! "$2" =~ ^[0-9]+$ ]]; then
        echo "--max-lines requires a positive integer" >&2
        exit 2
      fi
      max_lines="$2"
      shift 2
      ;;
    --allowlist)
      if [[ $# -lt 2 ]]; then
        echo "--allowlist requires a path" >&2
        exit 2
      fi
      allowlist_path="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

repo_root="$(git rev-parse --show-toplevel)"
allowlist_abs="$repo_root/$allowlist_path"

allowlisted=()
if [[ -f "$allowlist_abs" ]]; then
  while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
    line="${raw_line%%#*}"
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [[ -z "$line" ]] && continue
    allowlisted+=("$line")
  done < "$allowlist_abs"
fi

is_allowlisted() {
  local candidate="$1"
  local allowed
  (( ${#allowlisted[@]} == 0 )) && return 1
  for allowed in "${allowlisted[@]}"; do
    [[ "$candidate" == "$allowed" ]] && return 0
  done
  return 1
}

line_count_for() {
  local path="$1"
  local count
  count="$(wc -l < "$path")"
  count="${count//[[:space:]]/}"
  echo "$count"
}

violations=()
allowed_over_budget=()
allowlist_under_budget=()
stale_allowlist=()

if (( ${#allowlisted[@]} > 0 )); then
  for allowed in "${allowlisted[@]}"; do
    if [[ ! -f "$repo_root/$allowed" ]]; then
      stale_allowlist+=("$allowed")
    fi
  done
fi

while IFS= read -r -d '' file; do
  lines="$(line_count_for "$repo_root/$file")"
  if (( lines > max_lines )); then
    if is_allowlisted "$file"; then
      allowed_over_budget+=("$lines $file")
    else
      violations+=("$lines $file")
    fi
  elif is_allowlisted "$file"; then
    allowlist_under_budget+=("$lines $file")
  fi
done < <(git -C "$repo_root" ls-files --cached --others --exclude-standard -z -- '*.rs')

if (( ${#violations[@]} > 0 )); then
  echo "Rust line budget exceeded. Max lines per file: $max_lines" >&2
  printf '  %s\n' "${violations[@]}" >&2
fi

if (( ${#stale_allowlist[@]} > 0 )); then
  echo "Line-budget allowlist contains paths that do not exist:" >&2
  printf '  %s\n' "${stale_allowlist[@]}" >&2
fi

if (( ${#allowlist_under_budget[@]} > 0 )); then
  echo "Line-budget allowlist contains files that are now within budget:" >&2
  printf '  %s\n' "${allowlist_under_budget[@]}" >&2
  echo "Remove those paths from $allowlist_path." >&2
fi

if (( ${#violations[@]} > 0 || ${#stale_allowlist[@]} > 0 || ${#allowlist_under_budget[@]} > 0 )); then
  exit 1
fi

echo "Rust line budget ok. Max lines per file: $max_lines"

if (( ${#allowed_over_budget[@]} > 0 )); then
  echo "Allowed over-budget baseline files:"
  printf '  %s\n' "${allowed_over_budget[@]}"
fi
