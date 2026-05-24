#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/install_skill.sh [codex] [--user]
  scripts/install_skill.sh claude [--user]

Installs or updates the find-related-files skill by copying the repository's
skills/find-related-files directory into the selected agent skill directory.
With no arguments, installs the Codex project skill into the current working
directory. Use --user for a user-level install.
EOF
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill_src="$repo_root/skills/find-related-files"
agent="codex"
scope="project"

while [[ $# -gt 0 ]]; do
  case "$1" in
    codex)
      agent="codex"
      ;;
    claude)
      agent="claude"
      ;;
    --user)
      scope="user"
      ;;
    -h|--help|help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
  shift
done

case "$agent:$scope" in
  codex:project)
    dest="$PWD/.codex/skills/find-related-files"
    ;;
  codex:user)
    dest="${CODEX_HOME:-$HOME/.codex}/skills/find-related-files"
    ;;
  claude:project)
    dest="$PWD/.claude/skills/find-related-files"
    ;;
  claude:user)
    dest="$HOME/.claude/skills/find-related-files"
    ;;
esac

mkdir -p "$(dirname "$dest")"
rm -rf "$dest"
cp -R "$skill_src" "$dest"
echo "installed find-related-files skill to $dest"
