# Audit JSON output contract

`related audit --format json` writes one schema 2 object followed by a newline.
Operational errors remain text on stderr and exit 1.

Consumers should reject unsupported higher schema versions and ignore unknown
fields in schema 2. Removing a field, renaming a field, or changing its type
requires a schema-version increment. New informational fields may be added
without an increment.

## Audit result

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
    "backend": "GitCli",
    "completeness": "target-window-exact",
    "approximate": false,
    "rename_tracking": "git-follow",
    "max_target_commits": 1000,
    "scan_commits": 0
  },
  "hints": []
}
```

`scope` is `worktree`, `staged`, or `range:<revision-range>`. Worktree audit
includes untracked, non-ignored paths in `seeds`.

`supported_by` identifies the changed paths whose history contributed to a
candidate. `cochanges` sums support across those paths.
`strongest_pair_cochanges` is the strongest repeated changed-file/candidate
relationship and determines the confidence band.

`abstained` is true when no candidate meets `minimum_confidence`. Confidence is
deterministic evidence strength: low is one strongest-pair co-change, medium is
2–24, and high is at least 25. Active boundaries are machine-readable in
`confidence_thresholds`.

`enforcement` appears only with `--fail-on-confidence`. Discovery exits 0 even
when candidates exist. Triggered enforcement writes the complete object and
then exits 3.

## History coverage

`history_coverage` describes the actual audit rather than the requested mode.
Important `rename_tracking` values are:

- `git-follow`: exact committed history with Git similarity detection;
- `git-follow+diff-renames`: exact history plus an uncommitted rename mapping;
- `exact-blob-renames`: bounded fast history with unambiguous identical-blob
  rename tracking;
- `exact-blob-renames+diff-renames`: fast committed and uncommitted rename
  tracking.

Use exact accuracy for content-changing or ambiguous committed renames.

## Omission evaluation

`related eval --task audit --format json` also uses schema 2. It reports task
eligibility counters, chronological settings, and per-mode metrics including
`hits_at_k`, `hit_rate_at_k`, `mrr`, `avg_false_positives`, and
`abstention_rate`.

`confidence_metrics` contains one row per mode and confidence band with
candidate count, correct count, candidate precision, task coverage, and
conditional hit rate. Rows are computed before minimum-confidence filtering so
one evaluation can compare all evidence bands.

`rename_tracking` is `training-window+current-test-diff`. Training-window rename
chains are canonicalized, and only the current held-out commit's rename mapping
is available during its task. Renames from other held-out commits are excluded.
