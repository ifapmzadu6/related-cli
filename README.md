<div align="center">

# related-cli

**Content-blind related-file ranking from Git co-change history**

[![npm version](https://img.shields.io/npm/v/related-cli?logo=npm&logoColor=white&label=npm&color=cb3837)](https://www.npmjs.com/package/related-cli)
[![CI](https://github.com/ifapmzadu6/related-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/ifapmzadu6/related-cli/actions/workflows/ci.yml)
[![Node.js](https://img.shields.io/node/v/related-cli?logo=node.js&logoColor=white&color=339933)](package.json)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-000000?logo=rust&logoColor=white)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

</div>

`related` audits a changed file set and ranks likely omissions using only Git
co-change history. It can also query files individually as a small,
supplementary context-expansion step before an agent or developer edits,
reviews, or explains a tracked file.

It does not parse source code, imports, symbols, embeddings, or file contents.
The same signal therefore works for code, tests, docs, configs, migrations,
prompts, and other files tracked in Git.

## How it works

If two files repeatedly change in the same commit, they probably carry an
operational relationship. `related` turns repository history into a weighted
graph:

- file = node
- same-commit change = edge
- large commits are down-weighted
- older commits are time-decayed

Queries can use direct co-change ranking or Personalized PageRank over the
target-local co-change graph. Rankings are context hints, not proof that a file
must change.

`related audit` combines the historical relationships of the changed set,
reports which changed files support each candidate, and omits weak one-off
relationships by default. Its `low`, `medium`, and `high` confidence labels are
deterministic evidence-strength bands, not probabilities. The conservative
`high` boundary requires at least 25 co-changes for one changed-file/candidate
pair and was selected from chronological holdouts on three repositories.

## Skill installation

The npm package ships a portable `find-related-files` skill for Codex and
Claude Code. The skill does not vendor the binary or require a global install.
Its installer pins normal query commands to the installed package version; run
the installer again when you want to update it.

The installed workflow keeps explicit task requirements, direct source search,
and tests authoritative. Co-change rankings add candidates; they do not define
the edit plan. It is especially useful as a pre-commit or pre-PR completeness
audit: aggregate the changed set once, then check whether historically coupled
docs, tests, configs, or companion implementations were unintentionally missed.

### Codex

Project install is recommended for shared repositories. Run this from the
target project root:

```sh
npx -y --package related-cli@latest related-install-skill
```

This copies the skill to `.agents/skills/find-related-files`. Commit that
directory when the workflow should travel with the repository.

For an intentional user-level install:

```sh
npx -y --package related-cli@latest related-install-skill --user
```

Restart Codex after installing the skill.

### Claude Code

Project install:

```sh
npx -y --package related-cli@latest related-install-skill claude
```

This copies the skill to `.claude/skills/find-related-files`.

User-level install:

```sh
npx -y --package related-cli@latest related-install-skill claude --user
```

Claude Code can load the skill automatically from its description, or you can
invoke it directly with `/find-related-files`.

## CLI usage

The skill is the intended entry point for agent use. The CLI is also available
directly through the npm package:

```sh
npx -y --package related-cli@latest related query src/auth.ts --top 20
npx -y --package related-cli@latest related audit
npx -y --package related-cli@latest related audit --staged
npx -y --package related-cli@latest related audit --range main..HEAD
npx -y --package related-cli@latest related audit --staged --fail-on-confidence high
```

The commands are:

| Command | Purpose |
|---|---|
| `related audit [--staged\|--range RANGE]` | Audit a changed set for likely omitted companion files |
| `related query <file>` | Rank files related to one tracked file |
| `related explain <file-a> <file-b>` | Show direct co-change evidence for a pair |
| `related diff [--staged]` | Legacy changed-set aggregation without confidence filtering |
| `related eval [--task query\|audit]` | Run a chronological holdout evaluation |

Run `related <command> --help` for the complete option list.

### Supported npm platforms

The npm package bundles native binaries for Node.js 14 or newer on:

- macOS: Apple silicon (`arm64`) and Intel (`x64`)
- Linux: `arm64` and `x64`
- Windows: `arm64` and `x64`

On another operating system or CPU architecture, the npm wrapper exits with a
message listing the supported platform keys.

### Paths and repositories

Commands use the current directory's Git repository by default. Relative file
arguments are resolved from that directory, so querying from a repository
subdirectory works as expected.

Use `--repo PATH` to target another repository or to make its path the base for
relative file arguments:

```sh
npx -y --package related-cli@latest related query src/auth.ts --repo /path/to/repo
```

Query targets must be tracked by Git. Typos and paths outside the repository
are reported as errors. UTF-8 file names, including non-ASCII names, are
supported; Git paths that are not valid UTF-8 are rejected because the text
protocol is UTF-8.

### History backends

The default `pack-fast` backend reads SHA-1 object storage directly and uses a
latency-bounded history scan. It is optimized for quick agent calls and can stop
before an exact full-history walk.

- Use `--history-backend git` when exact Git history is more important than
  latency.
- Use `--history-backend pack-scan` for a deeper pack-only scan.
- An unsupported default object format or storage layout automatically falls
  back to `git` and emits a hint.
- Pack readers reject an individual decompressed Git object larger than 256 MiB;
  the default backend falls back to `git` if it encounters one.
- Pack readers reject delta chains deeper than 128 objects and changed-file
  trees deeper than 256 directories.
- Git subprocess output is capped at 64 MiB per invocation to avoid unbounded
  memory use on unusually broad histories.
- An explicitly requested incompatible backend returns an error.

The Git backend is path-exact and follows similarity-detected file renames.
Pack backends follow a committed rename when exactly one deleted source has the
same blob as the new path; ambiguous copies and content-changing renames are
left for exact mode. No backend creates a persistent index.

For normal use, prefer the stable accuracy levels instead of selecting an
implementation backend directly:

```sh
related audit --accuracy fast
related audit --accuracy exact
```

`fast` uses the latency-bounded default and can fall back to Git. It maps an
uncommitted staged rename to its old history path and follows unambiguous,
content-identical committed renames directly from pack data. `exact` uses Git's
exact target-history selection and also follows similarity-detected renames
whose contents changed, combining old and new target paths into one relationship
chain. `--history-backend` remains available for advanced measurement and
compatibility.

### Ranking controls

Common controls include:

```sh
related query src/auth.ts --top 20 --evidence 3
related query src/auth.ts --max-commits 500 --half-life-days 180
related query src/auth.ts --mode pagerank
related query src/auth.ts --format json
```

If broad dependency, release, formatting, generated, or initial commits dominate
the results, inspect evidence and retry with a smaller commit-size limit plus
exclusions:

```sh
related query src/auth.ts \
  --top 20 \
  --max-files-per-commit 10 \
  --exclude '*.lock,*-lock.*,*lockb,.github/workflows/*' \
  --evidence 3
```

The default compact text output contains ranked paths and short `co=` counts.
Use `--format json` when another tool needs structured output. Query-oriented
commands retain schema 1; `audit` and audit evaluation use schema 2. See
[the JSON output contract](docs/json-output.md). Evidence is opt-in. Follow any
emitted `hint:` lines before opening a large number of files.

### Evaluation

`related eval` defaults to `--query-shape on-demand`, which reconstructs the
target-local graph shape used by normal queries:

```sh
related eval --test-commits 200 --train-commits 1000 --top 10
```

`--query-shape global` builds one graph over the entire training window. It is
useful for measuring the potential of the history signal but should not be
presented as production-query accuracy.

Audit evaluation chronologically hides one known file from each eligible
multi-file commit and tests whether the remaining changed set recovers it:

```sh
related eval --task audit --test-commits 200 --train-commits 1000 --top 5
```

Audit evaluation also reports candidate precision and task coverage for each
confidence band. Rename aliases learned inside the training window are combined
without crossing the holdout boundary. A rename in the currently evaluated
commit is mapped like an uncommitted diff, but renames from other held-out test
commits are not reused. Run the evaluator on the target repository before
enabling enforcement.

### CI enforcement

Audit is discovery-only by default and exits 0 even when it returns candidates.
`--fail-on-confidence LEVEL` opts into enforcement: the audit output is still
written, then the process exits 3 when any displayed candidate meets the chosen
level. Operational and usage errors exit 1. A safe starting point is:

```sh
related audit --staged --accuracy exact --fail-on-confidence high
```

The failure threshold cannot be lower than `--min-confidence`. Enforcement is
never enabled implicitly.

See [CI and hook integration](docs/ci-integration.md) for a full-history GitHub
Actions job and non-blocking or enforcing staged-change hooks.

## Limitations

- New files and repositories with little history have weak or no co-change
  evidence.
- Squashed histories and broad mechanical commits reduce signal quality.
- Fast pack history follows only unambiguous, content-identical committed
  renames. Use `--accuracy exact` for content-changing or ambiguous rename
  boundaries.
- Deleted paths are not returned as related-file candidates.
- Co-change is correlation, not a requirement to edit every returned file.
- Audit confidence is an evidence band rather than a probability. The measured
  high boundary is conservative but not universally reliable; use the built-in
  audit evaluation before enforcing it in CI.
- End-to-end agent accuracy improvement is not yet established; the initial
  three-task paired pilot found one efficiency win, one regression, and one
  neutral functional result. One guarded rerun corrected the known regression,
  but that is not enough to establish a general effect.
- The default `pack-fast` backend favors latency over an exact complete walk.

## Measurements and comparisons

The detailed research material is kept separate from the user guide:

- [BENCHMARK.md](BENCHMARK.md) describes the evaluator and a reproducible VS
  Code benchmark.
- [MEASUREMENTS.md](MEASUREMENTS.md) records accuracy, latency, history-window,
  token, and backend experiments.
- [COMPARISON.md](COMPARISON.md) compares the project with nearby tools and
  documents the scope of those comparisons.
- [The Codex editing pilot](experiments/agent-ab/results/2026-08-16-pilot.md)
  compares three tasks with and without the lookup; its
  [guardrail follow-up](experiments/agent-ab/results/2026-08-16-guardrail-follow-up.md)
  checks the known failure once more.

Reproduction helpers are available in `scripts/compare.sh`,
`scripts/speed_compare.py`, and `scripts/external_tool_compare.sh`.

## License

MIT
