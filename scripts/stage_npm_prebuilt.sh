#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$ROOT/npm/prebuilt/manifest.json"
PREBUILT="$ROOT/npm/prebuilt"

usage() {
  cat <<'USAGE'
Usage:
  scripts/stage_npm_prebuilt.sh --local
  scripts/stage_npm_prebuilt.sh [DIST_DIR]

Stages binaries into npm/prebuilt/<target-triple>/ so the npm package can
bundle every supported platform binary.

--local copies the current host release binary after cargo build --release.

DIST_DIR mode expects either:
  DIST_DIR/<target-triple>/related
  DIST_DIR/<target-triple>/related.exe
  DIST_DIR/related-<target-triple>/related
  DIST_DIR/related-<target-triple>/related.exe
  DIST_DIR/related-<target-triple>
  DIST_DIR/related-<target-triple>.exe
USAGE
}

manifest_rows() {
  node -e '
const manifest = require(process.argv[1]);
for (const target of Object.values(manifest.targets)) {
  console.log(`${target.triple}\t${target.binary}`);
}
' "$MANIFEST"
}

host_triple() {
  rustc -vV | sed -n 's/^host: //p'
}

stage_one() {
  local source="$1"
  local triple="$2"
  local binary="$3"
  local target_dir="$PREBUILT/$triple"
  mkdir -p "$target_dir"
  cp "$source" "$target_dir/$binary"
  if [[ "$binary" != *.exe ]]; then
    chmod 755 "$target_dir/$binary"
  fi
  echo "staged npm/prebuilt/$triple/$binary"
}

stage_local() {
  cargo build --release --quiet
  local triple
  triple="$(host_triple)"
  local binary="related"
  if [[ "$triple" == *windows* ]]; then
    binary="related.exe"
  fi
  local source="$ROOT/target/release/$binary"
  if [[ ! -f "$source" ]]; then
    echo "missing local release binary: $source" >&2
    exit 1
  fi
  stage_one "$source" "$triple" "$binary"
}

stage_from_dist() {
  local dist="$1"
  local missing=0
  while IFS=$'\t' read -r triple binary; do
    local candidates=(
      "$dist/$triple/$binary"
      "$dist/related-$triple/$binary"
      "$dist/npm-prebuilt-$triple/$binary"
      "$dist/related-$triple"
      "$dist/related-$triple.exe"
    )
    local found=""
    for candidate in "${candidates[@]}"; do
      if [[ -f "$candidate" ]]; then
        found="$candidate"
        break
      fi
    done
    if [[ -z "$found" ]]; then
      echo "missing dist binary for $triple ($binary)" >&2
      missing=1
      continue
    fi
    stage_one "$found" "$triple" "$binary"
  done < <(manifest_rows)
  if [[ "$missing" -ne 0 ]]; then
    exit 1
  fi
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "${1:-}" == "--local" ]]; then
  stage_local
else
  stage_from_dist "${1:-$ROOT/dist/npm-prebuilt}"
fi
