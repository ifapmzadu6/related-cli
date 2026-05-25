#!/usr/bin/env bash
set -euo pipefail

repo="${1:-/tmp/related-vscode}"
target="${RELATED_EXTERNAL_TARGET:-src/vs/platform/sandbox/common/terminalSandboxEngine.ts}"
since="${RELATED_EXTERNAL_SINCE:-2026-05-08T14:03:23-07:00}"
out_dir="${RELATED_EXTERNAL_OUT:-/tmp/related-external-compare}"
external_prefix="${RELATED_EXTERNAL_PREFIX:-/tmp/related-external}"
related_bin="${RELATED_BIN:-target/release/related}"
mkdir -p "$out_dir" "$external_prefix"

if [[ ! -x "$related_bin" ]]; then
  cargo build --release
fi

if [[ ! -x "$external_prefix/node_modules/.bin/codegraph" || ! -x "$external_prefix/node_modules/.bin/sourcebook" ]]; then
  npm install --prefix "$external_prefix" @optave/codegraph sourcebook
fi

codegraph="$external_prefix/node_modules/.bin/codegraph"
sourcebook="$external_prefix/node_modules/.bin/sourcebook"

{
  echo "# External Tool Speed Comparison"
  echo
  echo "- repo: $repo"
  echo "- target: $target"
  echo "- since: $since"
  echo "- codegraph: $("$codegraph" --version)"
  echo "- sourcebook: $("$sourcebook" --version)"
  echo
} > "$out_dir/summary.md"

echo "## related on-demand query x20" >> "$out_dir/summary.md"
echo '```text' >> "$out_dir/summary.md"
/usr/bin/time -p bash -c '
  for _ in $(seq 1 20); do
    "$0" query "$1" --repo "$2" --since "$3" --mode direct --top 10 >/dev/null
  done
' "$related_bin" "$target" "$repo" "$since" >> "$out_dir/summary.md" 2>&1
echo '```' >> "$out_dir/summary.md"

echo "## codegraph build" >> "$out_dir/summary.md"
echo '```text' >> "$out_dir/summary.md"
rm -rf "$repo/.codegraph"
/usr/bin/time -p "$codegraph" build "$repo" --no-ast --no-complexity --no-dataflow --no-cfg \
  >> "$out_dir/summary.md" 2>&1
echo '```' >> "$out_dir/summary.md"

echo "## codegraph co-change analyze" >> "$out_dir/summary.md"
echo '```text' >> "$out_dir/summary.md"
/usr/bin/time -p "$codegraph" co-change --analyze --since "$since" --min-support 1 --min-jaccard 0 --include-tests --full \
  >> "$out_dir/summary.md" 2>&1
echo '```' >> "$out_dir/summary.md"

echo "## codegraph query x20" >> "$out_dir/summary.md"
echo '```text' >> "$out_dir/summary.md"
/usr/bin/time -p bash -c '
  for _ in $(seq 1 20); do
    "$0" co-change "$1" --include-tests --min-support 1 --min-jaccard 0 -n 10 --json >/dev/null
  done
' "$codegraph" "$target" >> "$out_dir/summary.md" 2>&1
echo '```' >> "$out_dir/summary.md"

echo "## sourcebook scan-history" >> "$out_dir/summary.md"
echo '```text' >> "$out_dir/summary.md"
/usr/bin/time -p "$sourcebook" scan-history --dir "$repo" --json --top 20 >/tmp/sourcebook-vscode-scan-history.json 2>> "$out_dir/summary.md"
echo '```' >> "$out_dir/summary.md"

echo "## sourcebook preflight x20" >> "$out_dir/summary.md"
echo '```text' >> "$out_dir/summary.md"
/usr/bin/time -p bash -c '
  for _ in $(seq 1 20); do
    "$0" preflight --dir "$1" --file "$2" --json >/dev/null
  done
' "$sourcebook" "$repo" "$target" >> "$out_dir/summary.md" 2>&1
echo '```' >> "$out_dir/summary.md"

echo "$out_dir/summary.md"
