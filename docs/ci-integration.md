# CI and hook integration

`related audit` is discovery-only unless `--fail-on-confidence` is supplied.
Start by running the chronological evaluator on the repository, then enable a
failure threshold whose candidate precision and coverage are acceptable for
that project.

## GitHub Actions pull-request audit

The checkout must include history and both pull-request endpoints. This job
uses exact history so similarity-based renames are included and exits 3 only for
displayed high-confidence findings:

```yaml
name: Related-file audit

on:
  pull_request:

permissions:
  contents: read

jobs:
  audit:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
      - uses: actions/setup-node@v6
        with:
          node-version: "24"
      - name: Audit changed files
        env:
          BASE_SHA: ${{ github.event.pull_request.base.sha }}
          HEAD_SHA: ${{ github.event.pull_request.head.sha }}
        run: |
          npx -y --package related-cli@1 related audit \
            --range "$BASE_SHA..$HEAD_SHA" \
            --accuracy exact \
            --fail-on-confidence high
```

Exit 0 means the audit completed without an enforced finding. Exit 3 means at
least one displayed candidate met the requested confidence threshold. Exit 1
means usage, repository, or runtime failure. The complete text or JSON result is
written before exit 3.

## Local Git hook

A non-blocking pre-commit hook is a useful calibration starting point:

```sh
#!/bin/sh
exec npx -y --package related-cli@1 related audit --staged --accuracy fast
```

After repository-local evaluation, enforcement can be enabled explicitly:

```sh
#!/bin/sh
exec npx -y --package related-cli@1 related audit \
  --staged \
  --accuracy exact \
  --fail-on-confidence high
```

Do not install hooks automatically into contributor worktrees. Keep the hook in
project documentation or a repository-managed hook framework so its behavior is
reviewable.
