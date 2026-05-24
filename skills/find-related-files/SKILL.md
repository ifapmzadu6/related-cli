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
npx -y --package related-cli@latest related query path/to/file --top 20 --json
```

Use `--repo PATH` only when querying another checkout from outside that repo:

```sh
npx -y --package related-cli@latest related query path/to/file --repo /path/to/repo --top 20 --json
```

For staged edits, ask for related files for the changed set:

```sh
npx -y --package related-cli@latest related diff --staged --top 20 --json
```

Open the strongest relevant results before making edits. Treat the ranking as a
context hint, not proof that a file must change.

## Options

- Add `--evidence N` when examples of shared commits would help.
- Use `explain file-a file-b --json` to inspect one relationship.
- Use `--history-backend git` when exact Git history is more important than
  low latency.
- Use the default backend for a fast pre-edit check in large workspaces.

## Distribution Notes

Do not require global installation. Use `related-cli@latest` so the skill picks
up CLI fixes and performance improvements automatically.
