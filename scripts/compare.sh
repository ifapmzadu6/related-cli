#!/usr/bin/env bash
set -euo pipefail

repo="${1:-/tmp/related-vscode}"
bin="${RELATED_BIN:-target/release/related}"
out_dir="${RELATED_COMPARE_OUT:-/tmp/related-compare}"
mkdir -p "$out_dir"

if [[ ! -x "$bin" ]]; then
  cargo build --release
fi

files="$(git -C "$repo" ls-files | wc -l | tr -d ' ')"
commits="$(git -C "$repo" rev-list --count HEAD | tr -d ' ')"

{
  echo "# Measurement Run"
  echo
  echo "- repo: $repo"
  echo "- files: $files"
  echo "- commits: $commits"
  echo "- binary: $bin"
  echo "- output_dir: $out_dir"
  echo
} > "$out_dir/summary.md"

run_eval() {
  local name="$1"
  shift
  echo "## $name" >> "$out_dir/summary.md"
  echo >> "$out_dir/summary.md"
  echo '```text' >> "$out_dir/summary.md"
  "$bin" eval --repo "$repo" "$@" | tee "$out_dir/$name.txt" >> "$out_dir/summary.md"
  echo '```' >> "$out_dir/summary.md"
  echo >> "$out_dir/summary.md"
}

run_query_timing() {
  local target="${RELATED_QUERY_TARGET:-package.json}"
  local runs="${RELATED_QUERY_RUNS:-20}"
  {
    echo "## query-latency"
    echo
    echo "- target: $target"
    echo "- runs per mode: $runs"
    echo
    echo '```text'
  } >> "$out_dir/summary.md"
  for mode in direct pagerank path hot; do
    local timing="$out_dir/query-$mode.time.txt"
    /usr/bin/time -p bash -c '
      for _ in $(seq 1 "$0"); do
        "$1" query "$2" --repo "$3" --mode "$4" --top 10 >/dev/null
      done
    ' "$runs" "$bin" "$target" "$repo" "$mode" 2> "$timing"
    echo "$mode" >> "$out_dir/summary.md"
    cat "$timing" >> "$out_dir/summary.md"
  done
  {
    echo '```'
    echo
  } >> "$out_dir/summary.md"
}

run_eval "accuracy-top5" --test-commits 200 --train-commits 1000 --top 5 --max-files-per-commit 80
run_eval "accuracy-top10" --test-commits 200 --train-commits 1000 --top 10 --max-files-per-commit 80
run_eval "accuracy-top20" --test-commits 200 --train-commits 1000 --top 20 --max-files-per-commit 80

run_eval "train-200" --test-commits 200 --train-commits 200 --top 10 --max-files-per-commit 80
run_eval "train-500" --test-commits 200 --train-commits 500 --top 10 --max-files-per-commit 80
run_eval "train-1000" --test-commits 200 --train-commits 1000 --top 10 --max-files-per-commit 80

run_eval "max-files-20" --test-commits 200 --train-commits 1000 --top 10 --max-files-per-commit 20
run_eval "max-files-80" --test-commits 200 --train-commits 1000 --top 10 --max-files-per-commit 80
run_eval "max-files-200" --test-commits 200 --train-commits 1000 --top 10 --max-files-per-commit 200

run_query_timing

echo "$out_dir/summary.md"
