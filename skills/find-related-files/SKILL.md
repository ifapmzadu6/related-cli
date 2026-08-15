---
name: find-related-files
description: Find historically related files before editing or reviewing a Git-tracked file by running related-cli against Git co-change history. Use when Codex should discover likely companion files in a repository without parsing source code, grep, embeddings, or persistent indexes.
---

# Find Related Files

## Overview

Use `related-cli` as a lightweight context expansion step before editing,
reviewing, or explaining a file. It ranks files that changed together in Git
history, so it can surface tests, configs, docs, migrations, and companion code
that text search may miss.

## Workflow

Run from the repository root when possible:

```sh
env npm_config_loglevel=error npx -y --package related-cli@latest related query path/to/file --top 20
```

Use `--repo PATH` only when querying another checkout from outside that repo:

```sh
env npm_config_loglevel=error npx -y --package related-cli@latest related query path/to/file --repo /path/to/repo --top 20
```

For staged edits, ask for related files for the changed set:

```sh
env npm_config_loglevel=error npx -y --package related-cli@latest related diff --staged --top 20
```

Open the strongest relevant results before making edits. Treat the ranking as a
context hint, not proof that a file must change.

If the output includes `hint:` lines, follow them before opening many files. Add
`--evidence N` when another tool needs example commits behind the ranking.

If the top results look like broad release, dependency, formatting, generated,
or initial commit churn, inspect evidence and retry with a tighter commit-size
filter plus result exclusions before opening files:

```sh
env npm_config_loglevel=error npx -y --package related-cli@latest related query path/to/file --top 20 --evidence 3
env npm_config_loglevel=error npx -y --package related-cli@latest related query path/to/file --top 20 --max-files-per-commit 10 --exclude '*.lock,*-lock.*,*lockb,.github/workflows/*'
```

## Options

- Add `--evidence N` when examples of shared commits would help.
- Add `--exclude PATTERNS` to hide comma-separated path patterns such as
  `*.lock,*-lock.*,*lockb,.github/workflows/*` from results.
- Use `explain file-a file-b` to inspect one relationship.
- Use `--history-backend git` when exact Git history is more important than
  low latency.
- Use the default `pack-fast` backend for a fast approximate pre-edit check in
  large workspaces. It automatically falls back to Git when the repository's
  object format or storage layout is unsupported.

## Distribution Notes

Do not require global installation. Use `related-cli@latest` so the skill picks
up CLI fixes and performance improvements automatically. The skill instructions
themselves are copied into the agent, so update them by pulling this repository
or by rerunning `npx -y --package related-cli@latest related-install-skill` for
a Codex project install from the target project root. Use
`related-install-skill claude` for a Claude Code project install, and use
`--user` only when a user-level install is intentional.
