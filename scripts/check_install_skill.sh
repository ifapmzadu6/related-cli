#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHELL_INSTALL="$ROOT/scripts/install_skill.sh"
NPM_INSTALL=(node "$ROOT/npm/bin/install-skill.js")

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

  (cd "$project" && "$@" codex >/dev/null)
  test -f "$project/.agents/skills/find-related-files/SKILL.md"

  (cd "$project" && "$@" claude >/dev/null)
  test -f "$project/.claude/skills/find-related-files/SKILL.md"

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
