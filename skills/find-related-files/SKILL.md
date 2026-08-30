---
name: find-related-files
description: Audit a Git worktree, staged change, or pull-request range for omitted companion files before committing or reviewing. Use when a changed set may be missing historically coupled tests, docs, configs, migrations, generated metadata, or platform counterparts.
---

# Audit Related Files

Run one changed-set omission audit near the end of an editing or review task.
Explicit task requirements, the current diff, and tests remain authoritative.

## Choose the changed set

Use exactly one scope that matches the work being checked:

```sh
# Unstaged, staged, and untracked non-ignored worktree changes
env npm_config_loglevel=error npx -y --package related-cli@latest related audit

# Staged changes only
env npm_config_loglevel=error npx -y --package related-cli@latest related audit --staged

# Pull-request or branch range
env npm_config_loglevel=error npx -y --package related-cli@latest related audit --range main..HEAD
```

If staged and unstaged work represent different intended changes, audit each set
once. Do not run one audit per changed file and do not repeat an identical audit.

## Review candidates

For each candidate, use `supported_by`, co-change counts, and evidence commits to
decide whether it is a plausible omission. Inspect the current task and diff
before editing it. Common omissions include tests, docs, configuration,
migrations, generated metadata, and cross-platform counterparts.

Confidence is evidence strength, not a probability or an instruction to edit.
An empty or abstained result means the available history did not support a
candidate strongly enough; it does not prove the change set is complete.

Use exact history for a final CI-equivalent check when rename completeness
matters:

```sh
env npm_config_loglevel=error npx -y --package related-cli@latest related audit --staged --accuracy exact
```

Follow any emitted `hint:` before relying on the result. Add `--evidence 3` when
the shared commits are needed to judge a candidate. Never enable
`--fail-on-confidence` unless the repository has intentionally adopted an
enforcement threshold.

## Distribution

Do not require global installation. Rerun
`npx -y --package related-cli@latest related-install-skill` to update a Codex
project install. Use `related-install-skill claude` for Claude Code and use
`--user` only when a user-level install is intentional.
