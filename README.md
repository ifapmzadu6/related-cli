# related

`related` is a small CLI that ranks files related to a target file using only Git
co-change history.

It intentionally does not parse source code, imports, symbols, embeddings, or file
contents. That makes the signal language-agnostic and useful for code, docs,
configs, migrations, prompts, runbooks, and any other files tracked in Git.

## Idea

If two files are repeatedly changed in the same commit, they probably carry an
operational relationship. `related` turns that history into a weighted graph:

- file = node
- same-commit change = edge
- large commits are down-weighted
- older commits are time-decayed

Queries can use either direct co-change ranking or Personalized PageRank over the
co-change graph.

## Quick Start

```sh
npx -y --package related-cli@latest related query src/auth.ts --top 20
```

The npm package is designed to bundle the prebuilt `related` binaries for the
supported macOS, Linux, and Windows CPU/OS combinations, so installing it does
not require Rust, Cargo, or a build toolchain. It does not download a binary at
install time; the small platform binaries are shipped inside the npm tarball.
For LLM skills and tools, global installation is optional: invoke it through
`npx`/`npm exec` from the target repository.

## Build

```sh
cargo build --release
cargo run --release -- query src/auth.ts --top 20
```

The release profile uses thin LTO and a single codegen unit because the CLI is
intended to be called frequently as a low-latency tool.

## Usage

```sh
npx -y --package related-cli@latest related query src/auth.ts --top 20
npx -y --package related-cli@latest related query src/auth.ts --mode direct --json
npx -y --package related-cli@latest related query src/auth.ts --history-backend git
npx -y --package related-cli@latest related query src/auth.ts --history-backend hybrid
npx -y --package related-cli@latest related query src/auth.ts --history-backend git-remove-empty
npx -y --package related-cli@latest related explain src/auth.ts tests/auth.test.ts
npx -y --package related-cli@latest related diff --staged
```

`related` no longer writes or reads a persistent index. `query`, `explain`, and
`diff` build the needed co-change graph on demand for the target file or changed
files. The default window is the target file's latest `1000` touching commits,
not the repository's latest `1000` commits.
By default commands run against the current directory's Git repository. Use
`--repo PATH` only when querying another checkout from outside that repository.

The default `pack-fast` backend reads `.git/objects/pack` and `.idx` files
directly, without invoking Git or a Git library for the hot path. It memory maps
pack files, binary-searches pack indexes, inflates commit/tree objects, applies
pack deltas, walks path history, and performs tree diffs in process. Pack
inflation uses the `zlib-rs` backend through `flate2` over the mmap slice, and
inflated object bytes are shared across the query-local caches.
`pack-fast` is intentionally latency-bounded for LLM-tool use: it walks at most
`17,500` recent commits by default, and after the first `1,000` walked commits
can stop once it has seen `256` target-touching commits or `5,000` walked
commits without another target hit. It does not inspect the returned ranking or
top-K stability to decide when to stop. This can still change co-change counts
versus exact Git history because the traversal is bounded. Use
`--history-backend git` for Git-exact counts, or
`--history-backend pack-scan --jobs N` for a deeper pack-only scan with explicit
parallel diff expansion.

For compact LLM-tool output, `query` and `diff` omit per-commit evidence by
default. Add `--evidence N` when the response should include example commits, or
use `explain` for a focused pair.

Alternative on-demand backends are also available for measurement:
`hybrid`, `gix`, `git`, `git-remove-empty`, `git-batch`,
`git-batch-parallel`, `git-diff-tree`, `git-diff-tree-parallel`,
`git-rev-list`, `pack-fast`, and `pack-scan`. The `hybrid` backend keeps Git for
target commit selection and uses `gix` for Rust-side diff expansion.
`git-rev-list` is a faster approximate backend: it selects commits with
`git rev-list`, which can change co-change counts while often preserving the top
companion paths.
`git-remove-empty` adds Git's `--remove-empty` path limiter and can be much
faster for files with short visible history, but it can change scores, so it is
explicit only. `pack-scan` uses the same pack-only implementation as
`pack-fast`, but does not apply the default latency-bounded cutoff. If
`--scan-commits` is set, `pack-scan` applies only that explicit scanned-commit
cap. `pack-scan` uses multiple threads for diff expansion only when `--jobs N`
is provided explicitly.

Earlier `git2`/plumbing experiments on the VS Code checkout were not adopted as
defaults because they were less robust on shallow/promisor clones or not
equivalent. See
[MEASUREMENTS.md](MEASUREMENTS.md#direct-git-file-reading-feasibility).

## Evaluation

`related eval` holds out the newest commits, trains on older commits, and asks:
given one file from a held-out commit, can the tool rank the other files from
that same commit in the top K?

It reports `hit@k`, `precision@k`, `recall@k`, and `MRR` for:

- `direct`: direct co-change score
- `pagerank`: Personalized PageRank on the co-change graph
- `path`: a content-blind path/name similarity baseline
- `hot`: a global frequently-changed-file baseline

```sh
npx -y --package related-cli@latest related eval --test-commits 200 --train-commits 1000 --top 10
```

The `path` baseline is not grep. It is included because `eval` is intentionally
content-blind; it answers whether history beats simple file-name/path proximity.

## Comparison

`related` overlaps with tools such as LaserOwl, Glaux, Sourcebook, Qartez,
repowise, Codegraph, and CodeScene. The narrow positioning is different:
`related` is a local Rust CLI for one job: quickly return historically
co-changed files for a target file without parsing source code.

The closest public CLIs that were installed and measured locally were
`@optave/codegraph` and `sourcebook`.

Measured on `microsoft/vscode`:

- repo: `/tmp/related-vscode`
- target: `src/vs/platform/sandbox/common/terminalSandboxEngine.ts`
- history window: since `2026-05-08T14:03:23-07:00`
- Codegraph version: `3.10.0`
- Sourcebook version: `0.14.0`

| tool / command | measured task | time | storage |
|---|---|---:|---:|
| `related query` x20 | Query related files on demand | measured in [MEASUREMENTS.md](MEASUREMENTS.md#on-demand-target-history) | none |
| `codegraph build` | Build Codegraph DB before co-change works | `158.12s` | `528 MiB` |
| `codegraph co-change --analyze` | Populate co-change data | `2.84s` | same DB |
| `codegraph co-change <file>` x20 | Query co-change partners | `3.76s` total, `0.188s/query` | same DB |
| `sourcebook scan-history` | Scan history for co-change pairs | `3.15s` | output only |
| `sourcebook preflight --file` x20 | Suggest companion files before editing | `174.68s` total, `8.734s/query` | no persistent index used here |

Both `related` and Codegraph ranked the same top companion file for this target:

```text
src/vs/platform/sandbox/test/common/terminalSandboxEngine.test.ts
```

This is not a claim that `related` replaces those tools. Codegraph builds a
broader source graph, and Sourcebook blends history with structural preflight
signals. The narrower claim is: for the LLM-tool task "given this file, quickly
return historically co-changed files," `related` is substantially lighter and
faster on this VS Code measurement.

See [COMPARISON.md](COMPARISON.md) for a broader public-docs-based comparison
against nearby tools.

See [MEASUREMENTS.md](MEASUREMENTS.md) for empirical measurements across
accuracy, top-K behavior, history window size, large-commit filtering, query
latency, multiple repositories, and speed against on-demand Git history
implementations.

To reproduce the comparison against public third-party CLIs on VS Code:

```sh
scripts/external_tool_compare.sh /tmp/related-vscode
```

To reproduce the speed comparison against similar local mechanisms:

```sh
scripts/speed_compare.py \
  --repo /tmp/related-vscode \
  --target package.json
```

## License

MIT
