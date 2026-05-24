#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/install_skill.sh codex
  scripts/install_skill.sh claude
  scripts/install_skill.sh claude-project /path/to/project

Installs or updates the find-related-files skill by copying the repository's
skills/find-related-files directory into the selected agent skill directory.
EOF
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill_src="$repo_root/skills/find-related-files"
kind="${1:-}"

case "$kind" in
  codex)
    dest="${CODEX_HOME:-$HOME/.codex}/skills/find-related-files"
    ;;
  claude)
    dest="$HOME/.claude/skills/find-related-files"
    ;;
  claude-project)
    project="${2:-}"
    if [[ -z "$project" ]]; then
      usage >&2
      exit 2
    fi
    dest="$project/.claude/skills/find-related-files"
    ;;
  -h|--help|help|"")
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

mkdir -p "$(dirname "$dest")"
rm -rf "$dest"
cp -R "$skill_src" "$dest"
echo "installed find-related-files skill to $dest"
