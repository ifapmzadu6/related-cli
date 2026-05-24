#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

tmp="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT

version="$(cd "$ROOT" && node -p "require('./package.json').version")"
tarball="$tmp/related-cli-$version.tgz"

(cd "$ROOT" && RELATED_NPM_ALLOW_MISSING_PREBUILT=1 npm pack --pack-destination "$tmp" >/dev/null)
test -f "$tarball"

project="$tmp/project"
mkdir -p "$project"

(cd "$project" && npm exec --yes --package "$tarball" related-install-skill >/dev/null)
test -f "$project/.codex/skills/find-related-files/SKILL.md"

(cd "$project" && npm exec --yes --package "$tarball" related-install-skill claude >/dev/null)
test -f "$project/.claude/skills/find-related-files/SKILL.md"

echo "npm package ok"
