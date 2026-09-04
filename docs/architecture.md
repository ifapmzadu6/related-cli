# Internal architecture

The public Rust entry points remain `run`, `exit_code_for_error`,
`AuditFindingsError`, and `EXIT_AUDIT_FINDINGS`. CLI syntax and serialized output
are described in [the JSON contract](json-output.md) and command help.

## Code map

| Area | Responsibility |
|---|---|
| `src/commands/` | CLI dispatch, shared option parsing, command orchestration, and help |
| `src/engine.rs` | Query execution, backend selection, fallback policy, and coverage reporting |
| `src/history/` | Git CLI and gitoxide history readers, Git output parsing, rename canonicalization |
| `src/pack/` | Pack-native queries, history traversal, tree diffs, object decoding, and cached storage |
| `src/graph.rs`, `src/ranking.rs` | Relationship graph, scoring, path resolution, and bounded top-k selection |
| `src/audit.rs` | Changed-set aggregation, confidence classification, and shared candidate budget |
| `src/evaluation/` | Chronological holdouts, training-only query cache, and evaluation metrics |
| `src/model.rs`, `src/output.rs` | Shared domain data, serialized contracts, and rendering |
| `src/git_utils.rs`, `src/repo.rs`, `src/path_utils.rs` | Bounded Git subprocesses, changed paths, repository discovery, and path handling |
| `src/tests/` | Behavior-grouped repository and CLI regression tests with shared fixtures |
| `npm/lib/prebuilt.js` | Bundled binary resolution and executable permissions for npm entry points |

## Boundaries

Commands parse user input and render results; the engine selects history
backends. History and pack readers do not depend on CLI parsing. The pack
backend keeps byte decoders separate from disk access and bounded caches;
resource limits live in `src/pack/limits.rs`.

Graph internals and pack scoring accumulators belong to their implementations,
not the serialized model. JSON field names, confidence thresholds, ordering,
error messages, and exit codes require compatibility review when changed.

Both query and audit evaluation use `TrainingQueries`, which receives only the
training window. Holdout commits cannot populate that cache. Audit evaluation
and shipping discovery share the per-seed candidate budget.

## Verification

Run `cargo test --locked --quiet`, `cargo fmt --check`, and
`cargo clippy --locked --all-targets --features fuzzing -- -D warnings`.
The pack and Git parser tests remain next to their modules; the fuzz entry point
remains `related::fuzzing::parse_repository_bytes`.

For behavior-preserving changes, build the baseline and candidate binaries in
separate checkouts and compare them against the same temporary repository:

```sh
python3 scripts/check_cli_compatibility.py /path/to/baseline/related target/debug/related
```

This compares stdout, stderr, and exit codes across commands, ranking modes,
history backends, evaluation, staged/mixed changes, renames, deletions, and
loose/packed objects. It requires Python 3 and Git, and does not edit the current
repository.

Use `scripts/check_install_skill.sh` and `scripts/check_npm_package.sh` for npm
and skill packaging. Release operations and the full release checklist remain
in [AGENTS.md](../AGENTS.md).
