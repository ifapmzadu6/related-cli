# v1 audit-first roadmap

The v1 goal is to evolve `related` from a related-file shortlist into a
changed-set omission audit with explicit evidence, support, confidence, and
history coverage.

## Implemented foundation

- `related audit` for worktree, staged, and revision-range scopes
- worktree discovery that includes untracked, non-ignored paths
- changed-file support attribution per candidate
- low/medium/high evidence-strength labels and default low-signal abstention
- five-candidate default output
- audit JSON schema 2 with structured seeds and history coverage
- public `--accuracy fast|exact` levels while retaining advanced backend flags
- chronological leave-one-out audit evaluation through `related eval --task audit`
- three-repository audit holdouts plus per-confidence precision and coverage
- a holdout-selected high-confidence boundary of 25 strongest-pair co-changes
- opt-in `--fail-on-confidence` enforcement with stable exit 3 for findings;
  ordinary discovery remains exit 0 and operational errors remain exit 1
- legacy `related diff` compatibility

## v1 completion gates

1. Extend committed rename-chain history beyond exact mode. Exact mode follows
   Git rename detection and combines old/new target paths. Both fast and exact
   audits map an uncommitted staged rename to its old history path, while fast
   pack queries still use the current path for older committed history. Make
   chronological evaluation account for rename boundaries as well. Every audit
   reports its level in `history_coverage.rename_tracking`.
2. Measure under-specified agent tasks and pre-PR audits. Do not claim a general
   agent-accuracy improvement unless it is reproduced without material token or
   latency regression.
3. Keep warm local p95 below 500 ms for representative five-candidate audits,
   or report when exact mode intentionally exceeds that budget.
4. Add hooks, Actions, or MCP integration only after the audit contract and
   thresholds are supported by these measurements.

## Compatibility policy

`query`, `explain`, and legacy `diff` retain schema 1 during the transition.
`audit` starts at schema 2. Backend implementation names remain available for
experiments, but user-facing workflows should use `--accuracy fast|exact`.
