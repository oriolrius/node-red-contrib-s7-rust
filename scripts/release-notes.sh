#!/bin/sh
# Builds the GitHub release notes of one version:
#   the summary .github/release-notes/vX.Y.Z.md (written for readers on GitHub),
#   then the CHANGELOG.md section of that version (the details), then the compare link.
# Usage: sh scripts/release-notes.sh X.Y.Z [out.md]   (fails when a part is missing)
set -eu
v="${1:?version X.Y.Z}"
out="${2:-/dev/stdout}"
root="$(cd "$(dirname "$0")/.." && pwd)"
summary="$root/.github/release-notes/v$v.md"
[ -s "$summary" ] || { echo "missing $summary: write the release summary first (see AGENTS.md)" >&2; exit 1; }
details="$(awk -v v="$v" '$0 ~ "^## \\[" v "\\]" {on=1; next} on && /^## \[/ {exit} on' "$root/CHANGELOG.md")"
[ -n "$(printf '%s' "$details" | tr -d '[:space:]')" ] || { echo "CHANGELOG.md has no section [$v]" >&2; exit 1; }
prev="$(grep -o '^## \[[0-9][0-9.]*\]' "$root/CHANGELOG.md" | sed 's/^## \[//; s/\]$//' | awk -v v="$v" 'f {print; exit} $0 == v {f=1}')"
repo="https://github.com/oriolrius/node-red-contrib-s7-rust"
{
  cat "$summary"
  printf '\n\n## Details (CHANGELOG)\n'
  printf '%s\n' "$details"
  if [ -n "$prev" ]; then
    printf '\n**Full diff:** %s/compare/v%s...v%s\n' "$repo" "$prev" "$v"
  fi
} > "$out"
