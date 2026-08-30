# JSON output contract

`query`, `audit`, `diff`, `explain`, and `eval` accept `--format json`.
Successful JSON output is one object followed by a newline. Operational errors
remain text on stderr and return a non-zero exit status.

Every top-level object contains `schema_version`. Query-oriented commands retain
schema 1. The audit contract starts at schema 2 so changed sets are represented
as arrays rather than the legacy comma-separated `target` field.

Schema 1 objects contain:

```json
{"schema_version":1}
```

Consumers should reject unsupported higher schema versions and ignore unknown
fields in a supported version. Removing a field, renaming a field, or changing
its type requires a schema-version increment. New optional or informational
fields may be added without incrementing the version.

## Query and diff

`query` and `diff` share this shape:

```json
{
  "schema_version": 1,
  "target": "src/auth.ts",
  "mode": "direct:on-demand:PackFast",
  "related": [
    {
      "path": "src/auth.test.ts",
      "score": 0.82,
      "cochanges": 4,
      "weight": 1.25,
      "last_seen": "2026-08-01T12:00:00Z",
      "reason": "direct_cochange",
      "evidence": []
    }
  ],
  "hints": []
}
```

For `diff`, `target` is the comma-separated changed-file set used by the
command. Evidence entries contain `hash`, `date`, `subject`, `file_count`, and
`weight`.

## Audit

`audit` uses schema 2:

```json
{
  "schema_version": 2,
  "scope": "staged",
  "seeds": ["src/auth.ts", "src/session.ts"],
  "mode": "direct",
  "minimum_confidence": "medium",
  "confidence_thresholds": {
    "medium_min_strongest_pair_cochanges": 2,
    "high_min_strongest_pair_cochanges": 25
  },
  "candidates": [
    {
      "path": "tests/auth.test.ts",
      "score": 1.42,
      "confidence": "high",
      "support_count": 2,
      "supported_by": ["src/auth.ts", "src/session.ts"],
      "cochanges": 29,
      "strongest_pair_cochanges": 25,
      "weight": 2.1,
      "last_seen": "2026-08-01T12:00:00Z",
      "reason": "direct_cochange",
      "evidence": []
    }
  ],
  "abstained": false,
  "enforcement": {
    "threshold": "high",
    "finding_count": 1,
    "triggered": true,
    "exit_code": 3
  },
  "history_coverage": {
    "backend": "PackFast",
    "completeness": "latency-bounded",
    "approximate": true,
    "rename_tracking": "current-path-only",
    "max_target_commits": 1000,
    "scan_commits": 0
  },
  "hints": []
}
```

`scope` is `worktree`, `staged`, or `range:<revision-range>`. Worktree audit
includes untracked, non-ignored paths in `seeds`. `supported_by` identifies the
changed paths whose rankings contributed to a candidate. `cochanges` sums edge
support across those paths, while `strongest_pair_cochanges` prevents one broad
commit touching many seeds from looking like a repeatedly observed pair.

`abstained` is true when no candidate met `minimum_confidence`. Confidence is a
deterministic evidence-strength label, not a probability. `low` means the
strongest changed-file pair occurred at most once, `medium` means 2-24 times,
and `high` means at least 25 times. The high boundary was selected from
chronological holdouts on three repositories; repository-local evaluation is
still recommended.

`confidence_thresholds` makes those active boundaries machine-readable in both
audit and audit-evaluation JSON.

`enforcement` is present only with `--fail-on-confidence`. It describes the
displayed findings used for the decision. Normal audit discovery exits 0 even
when candidates exist. Triggered enforcement writes the complete output and
then exits 3; usage, repository, and runtime errors exit 1.

`rename_tracking` is `git-follow` for exact committed history,
`git-follow+diff-renames` when exact audit also mapped an uncommitted rename,
`diff-renames-only` when fast audit mapped only the current diff, and
`current-path-only` otherwise. Use exact accuracy when the current path has
already crossed a committed rename boundary.

## Explain

`explain` returns `a`, `b`, `related`, `cochanges`, `weight`, `last_seen`,
`evidence`, and `hints` alongside `schema_version`.

## Eval

`eval` returns its repository and evaluation settings, task counters, and a
`metrics` array alongside `schema_version`. Each metrics entry contains `mode`,
`tasks`, `hit_rate_at_k`, `precision_at_k`, `recall_at_k`, `mrr`, and
`avg_results`.

With `--task audit`, eval uses schema 2 and returns
`query_shape: "on-demand-leave-one-out"`, the configured
`minimum_confidence`, audit eligibility counters, and metrics including
`hits_at_k`, `avg_false_positives`, and `abstention_rate`. Its
`confidence_metrics` array contains one row per mode and confidence band with
candidate counts, correct candidates, candidate precision, tasks containing the
band, task coverage, and the conditional hit rate. These rows are computed from
the top-K candidate set before `minimum_confidence` filtering so one evaluation
can compare all three bands.

Audit evaluation also returns `rename_tracking`, `training_renames`, and
`test_diff_renames`. `rename_tracking` is
`training-window+current-test-diff`: rename chains entirely inside training are
canonicalized to the training-boundary path, and a rename in the commit being
evaluated maps its destination to the known source path. Renames learned from a
different held-out test commit are deliberately unavailable, preventing future
or cross-holdout leakage.
