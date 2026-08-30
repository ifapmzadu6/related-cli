#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHELL_INSTALL="$ROOT/scripts/install_skill.sh"
NPM_INSTALL=(node "$ROOT/npm/bin/install-skill.js")
VERSION="$(node -p "require('$ROOT/package.json').version")"

tmp="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT

check_installer() {
  local label="$1"
  shift
  local project="$tmp/$label-project"
  mkdir -p "$project"

  (cd "$project" && "$@" >/dev/null)
  test -f "$project/.agents/skills/find-related-files/SKILL.md"
  grep -qF "related-cli@$VERSION related audit" "$project/.agents/skills/find-related-files/SKILL.md"
  grep -qF "related-cli@latest related-install-skill" "$project/.agents/skills/find-related-files/SKILL.md"
  grep -qF "Run one changed-set omission audit" "$project/.agents/skills/find-related-files/SKILL.md"
  grep -qF "Confidence is evidence strength" "$project/.agents/skills/find-related-files/SKILL.md"
  ! grep -qF " related query " "$project/.agents/skills/find-related-files/SKILL.md"

  (cd "$project" && "$@" codex >/dev/null)
  test -f "$project/.agents/skills/find-related-files/SKILL.md"

  (cd "$project" && "$@" claude >/dev/null)
  test -f "$project/.claude/skills/find-related-files/SKILL.md"
  grep -qF "related-cli@$VERSION related audit" "$project/.claude/skills/find-related-files/SKILL.md"
  grep -qF "Run one changed-set omission audit" "$project/.claude/skills/find-related-files/SKILL.md"
  grep -qF "Confidence is evidence strength" "$project/.claude/skills/find-related-files/SKILL.md"
  ! grep -qF " related query " "$project/.claude/skills/find-related-files/SKILL.md"

  env HOME="$tmp/$label-codex-home" "$@" --user >/dev/null
  test -f "$tmp/$label-codex-home/.agents/skills/find-related-files/SKILL.md"

  env HOME="$tmp/$label-home" "$@" claude --user >/dev/null
  test -f "$tmp/$label-home/.claude/skills/find-related-files/SKILL.md"

  if "$@" codex-project >/dev/null 2>&1; then
    echo "$label: codex-project should not be accepted" >&2
    exit 1
  fi

  if "$@" claude "$tmp/other-project" >/dev/null 2>&1; then
    echo "$label: project path arguments should not be accepted" >&2
    exit 1
  fi
}

check_installer shell "$SHELL_INSTALL"
check_installer npm "${NPM_INSTALL[@]}"

echo "skill installer ok"
