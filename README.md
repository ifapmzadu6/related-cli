<div align="center">

# related-cli

**Changed-set omission audits from Git co-change history**

[![npm version](https://img.shields.io/npm/v/related-cli?logo=npm&logoColor=white&label=npm&color=cb3837)](https://www.npmjs.com/package/related-cli)
[![CI](https://github.com/ifapmzadu6/related-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/ifapmzadu6/related-cli/actions/workflows/ci.yml)
[![Node.js](https://img.shields.io/node/v/related-cli?logo=node.js&logoColor=white&color=339933)](package.json)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-000000?logo=rust&logoColor=white)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

</div>

`related audit` checks a worktree, staged change, or revision range for likely
omitted companion files. It uses only Git co-change history and never reads
source contents, so the same signal covers code, tests, docs, configs,
migrations, prompts, and generated metadata.

## How omission detection works

Files that repeatedly changed together form a historical relationship. For the
current changed set, `related audit` ranks unchanged files that were coupled to
one or more changed files and reports:

- the candidate path;
- the changed paths that support it;
- co-change counts and example commits;
- a deterministic `low`, `medium`, or `high` evidence band;
- the actual history and rename coverage used by the run.

Weak one-off relationships are omitted by default. A candidate is a review
prompt, not proof that the file must change.

## Run an audit

No global installation is required:

```sh
# Unstaged, staged, and untracked non-ignored worktree changes
npx -y --package related-cli@latest related audit

# Staged changes only
npx -y --package related-cli@latest related audit --staged

# A pull-request or branch range
npx -y --package related-cli@latest related audit --range main..HEAD
```

Commands use the current Git repository by default. Use `--repo PATH` to audit
another checkout.

The default output contains at most five medium-or-higher candidates. Useful
controls include:

```sh
related audit --top 10
related audit --min-confidence high
related audit --evidence 3
related audit --format json
```

## Confidence and enforcement

Confidence is evidence strength rather than probability:

- `low`: the strongest changed-file/candidate pair occurred once;
- `medium`: it occurred 2–24 times;
- `high`: it occurred at least 25 times.

The high boundary was selected from chronological omission holdouts on three
repositories. It is conservative and intentionally has limited coverage.
Evaluate the target repository before enabling enforcement.

Audit discovery exits 0 even when it prints candidates. Enforcement is
explicit:

```sh
related audit --staged --accuracy exact --fail-on-confidence high
```

Exit codes are stable:

- `0`: audit completed without an enforced finding;
- `3`: at least one displayed candidate met the enforcement threshold;
- `1`: usage, repository, or runtime error.

The complete result is written before exit 3.

## Fast and exact history

```sh
related audit --accuracy fast
related audit --accuracy exact
```

`fast` uses a bounded pack-native history reader. It follows an unambiguous
committed rename when the old and new paths have identical content and maps an
uncommitted rename to its old history path. Unsupported repositories fall back
to Git with a visible hint.

`exact` uses Git history and similarity-based rename detection. Use it for CI,
content-changing renames, and ambiguous rename boundaries. Every audit reports
its actual coverage in `history_coverage`.

## Evaluate omission detection

The evaluator hides one known file from each eligible historical multi-file
change, uses the remaining files as the changed set, and checks whether the
audit recovers the omission. Training commits are strictly older than held-out
commits, and rename information does not cross holdout boundaries.

```sh
related eval --task audit \
  --test-commits 200 \
  --train-commits 1000 \
  --top 5 \
  --min-confidence medium
```

It reports hit rate, false positives, abstention, and precision and coverage for
each confidence band. See [MEASUREMENTS.md](MEASUREMENTS.md) for the recorded
three-repository omission results.

## CI and hooks

The repository's own pull-request workflow runs a full-history exact audit with
high-confidence enforcement. See [CI and hook integration](docs/ci-integration.md)
for a reusable GitHub Actions job and local staged-change hook.

Machine consumers should use the [audit JSON contract](docs/json-output.md),
which documents schema 2, history coverage, enforcement fields, and exit codes.

## Agent skill installation

The npm package ships a `find-related-files` skill that performs one changed-set
omission audit before a commit or pull request. It does not install a global
binary.

Codex project install:

```sh
npx -y --package related-cli@latest related-install-skill
```

Claude Code project install:

```sh
npx -y --package related-cli@latest related-install-skill claude
```

Use `--user` only for an intentional user-level install. Rerun the installer to
update the pinned package version in an existing installed skill.

## Supported npm platforms

The package bundles native binaries for Node.js 14 or newer on:

- macOS: Apple silicon and Intel;
- Linux: `arm64` and `x64`;
- Windows: `arm64` and `x64`.

## Limitations

- New files and repositories with little history provide weak evidence.
- Squashed histories and broad mechanical commits reduce signal quality.
- Co-change is correlation; inspect the task and diff before editing a candidate.
- Deleted paths are not returned as omission candidates.
- Fast mode follows only unambiguous, content-identical committed renames.
- Confidence boundaries require repository-local calibration for strict CI use.

## License

MIT
