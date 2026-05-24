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

## Skill Installation

This repository ships a portable skill at `skills/find-related-files`. The skill
does not vendor the binary and does not require a global install; it calls the
published npm package with `npx -y --package related-cli@latest related ...`.

Clone the repository once, then copy the skill folder into your agent's skill
directory:

```sh
git clone --depth 1 https://github.com/ifapmzadu6/related-cli.git
cd related-cli
```

### Codex

```sh
mkdir -p "${CODEX_HOME:-$HOME/.codex}/skills"
rm -rf "${CODEX_HOME:-$HOME/.codex}/skills/find-related-files"
cp -R skills/find-related-files "${CODEX_HOME:-$HOME/.codex}/skills/"
```

Restart Codex after installing the skill.

### Claude Code

Personal install, available in all projects:

```sh
mkdir -p "$HOME/.claude/skills"
rm -rf "$HOME/.claude/skills/find-related-files"
cp -R skills/find-related-files "$HOME/.claude/skills/"
```

Project install, checked into a single repository:

```sh
mkdir -p .claude/skills
rm -rf .claude/skills/find-related-files
cp -R /path/to/related-cli/skills/find-related-files .claude/skills/
```

Claude Code can load the skill automatically from its description, or you can
invoke it directly with `/find-related-files`.

## CLI

The skill is the intended entry point for agent use. The CLI remains available
for direct checks:

```sh
npx -y --package related-cli@latest related query src/auth.ts --top 20
npx -y --package related-cli@latest related diff --staged --top 20
```

By default commands run against the current directory's Git repository. Use
`--repo PATH` only when querying another checkout from outside that repository.
No persistent index is created; the graph is built on demand from the target
file's Git history.

The default backend, `pack-fast`, is optimized for low-latency agent calls in
large repositories and may stop before an exact full history walk. Use
`--history-backend git` when exact Git history is more important than speed, or
`--history-backend pack-scan` for a deeper pack-only scan.

If the top results all look like broad release, formatting, or initial-commit
churn, retry with a smaller commit-size filter before trusting the ranking:

```sh
npx -y --package related-cli@latest related query src/auth.ts --top 20 --max-files-per-commit 20
```

For compact LLM-tool output, `query` and `diff` omit per-commit evidence by
default. Add `--evidence N` when example commits would help, or use
`explain file-a file-b` for one focused relationship.

## Local Development

```sh
cargo build --release
cargo run --release -- query src/auth.ts --top 20
```

The release profile uses thin LTO and a single codegen unit because the CLI is
intended to be called frequently as a low-latency tool.

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
