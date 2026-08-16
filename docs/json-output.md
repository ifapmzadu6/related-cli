# JSON output contract

`query`, `diff`, `explain`, and `eval` accept `--format json`. Successful JSON
output is one object followed by a newline. Operational errors remain text on
stderr and return a non-zero exit status.

Every top-level object contains:

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

## Explain

`explain` returns `a`, `b`, `related`, `cochanges`, `weight`, `last_seen`,
`evidence`, and `hints` alongside `schema_version`.

## Eval

`eval` returns its repository and evaluation settings, task counters, and a
`metrics` array alongside `schema_version`. Each metrics entry contains `mode`,
`tasks`, `hit_rate_at_k`, `precision_at_k`, `recall_at_k`, `mrr`, and
`avg_results`.
