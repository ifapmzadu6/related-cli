# v1 omission-audit scope

Version 1 establishes one product outcome: detect likely companion files omitted
from a changed set using Git co-change history.

## Stable audit contract

- worktree, staged, and revision-range changed sets;
- untracked, non-ignored worktree seeds;
- candidate support attribution and example commit evidence;
- low, medium, and high deterministic evidence bands;
- default abstention from one-off relationships;
- schema 2 JSON with structured seeds and history coverage;
- stable exit 0 for discovery, exit 3 for opted-in findings, and exit 1 for
  operational errors;
- public `--accuracy fast|exact` levels;
- exact similarity-based rename tracking;
- bounded fast tracking for unambiguous, content-identical renames;
- chronological leave-one-out omission evaluation without cross-holdout rename
  leakage;
- confidence precision and coverage measured across three repositories;
- a full-history exact pull-request audit in this repository's CI.

## Product decisions

Fast mode deliberately stops at bounded exact-blob rename detection. Similarity
detection remains exact-mode work so the fast latency contract stays explicit.
Every audit reports its actual history and rename coverage.

Enforcement is never implicit. Repositories should run their own chronological
omission evaluation before adopting `--fail-on-confidence`. The documented
starting point is exact history with high-confidence enforcement.

The stable CLI and JSON result compose with local hooks, CI, and agent runtimes;
no additional protocol is required for the v1 omission-audit workflow.
