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
- rename-aware chronological evaluation that canonicalizes training aliases and
  maps only the current test diff without cross-holdout leakage
- bounded pack-native tracking for unambiguous, content-identical committed
  renames; exact mode retains similarity-based tracking
- three-repository audit holdouts plus per-confidence precision and coverage
- a holdout-selected high-confidence boundary of 25 strongest-pair co-changes
- opt-in `--fail-on-confidence` enforcement with stable exit 3 for findings;
  ordinary discovery remains exit 0 and operational errors remain exit 1
- representative twenty-file fast and exact audits below the 500 ms warm p95
  budget after rename tracking
- a 20-task editing-agent A/B that found no target-file accuracy improvement and
  measured higher token/time cost, plus three-repository historical pre-PR
  omission holdouts; no general agent-accuracy claim is made
- a full-history GitHub Actions pull-request audit in this repository plus
  documented Actions and local-hook recipes
- legacy `related diff` compatibility

## v1 product decisions

For v1, fast deliberately stops at bounded exact-blob rename detection; adding
similarity detection would duplicate exact mode's cost and weaken the latency
contract. Exact mode follows Git similarity detection and combines old/new
target paths. Every audit reports its actual level in
`history_coverage.rename_tracking`.

The repository's own pull-request CI runs exact audit with high-confidence
enforcement. Reusable Actions and hook recipes are documented, but hooks are not
installed into contributor worktrees automatically. MCP remains optional rather
than a v1 requirement because the stable CLI/JSON contract already composes with
agent runtimes.

## Compatibility policy

`query`, `explain`, and legacy `diff` retain schema 1 during the transition.
`audit` starts at schema 2. Backend implementation names remain available for
experiments, but user-facing workflows should use `--accuracy fast|exact`.
