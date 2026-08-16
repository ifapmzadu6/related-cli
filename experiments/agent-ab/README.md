# AI agent A/B evaluation

This experiment compares the same Codex editing task with and without a
mandatory `related-cli` lookup. Each case starts from the parent of a historical
commit in a disposable worktree, so the target patch is not reachable from the
checked-out `HEAD`.

The harness records:

- changed-file precision and recall against the historical patch
- added/deleted-line precision and recall against the historical patch
- Codex input/output token usage and elapsed time
- `git diff --check` plus optional hidden validation commands

Case files are trusted input: their validation commands run locally. Use only
case definitions you have reviewed.

Run the bundled pilot against a local checkout of
`ifapmzadu6/too_tired_to_type`:

```sh
python3 scripts/agent_ab.py \
  --repo /path/to/too_tired_to_type \
  --cases experiments/agent-ab/too-tired-to-type.json \
  --output /tmp/related-agent-ab-results
```

Use one or more `--case CASE_ID` arguments to run a subset of cases. Use
`--arm with-related` or `--arm without-related` for a targeted follow-up; normal
comparisons should run both arms.

Before a run, check the harness and case file without starting an agent:

```sh
python3 -m py_compile scripts/agent_ab.py
python3 scripts/agent_ab.py --help
```

The treatment arm runs `related-cli@0.4.0` before other repository inspection,
then applies the current skill guardrails: direct search must cover explicit
task targets, independent surfaces should use multiple anchors, and task text
overrides the ranking. The control arm is prohibited from using co-change or
Git-history based related-file lookup. Arm order alternates between cases to
reduce a simple first/second-run bias.

Run `npm ci --prefix api` in the benchmark checkout before the bundled API case;
the harness shares that ignored dependency directory between disposable
worktrees. The API validation fixture bypasses the repository's integration-test
server and exercises the rate-limit and body-size acceptance criteria directly.

## Interpretation limits

Historical patch similarity is not identical to semantic correctness. An
equivalent implementation can score lower when its lines differ from the
recorded patch. Conversely, a close patch can still be wrong. Case-specific
hidden tests are therefore more important than line overlap when available.

This pilot is intentionally small and from one repository. It can detect large
effects and workflow failures, but it cannot establish a general improvement in
agent task success without more repositories, tasks, models, and repeated runs.

The first recorded run is in
[`results/2026-08-16-pilot.md`](results/2026-08-16-pilot.md). A targeted rerun
of its failed case with the explicit-scope guardrails is in
[`results/2026-08-16-guardrail-follow-up.md`](results/2026-08-16-guardrail-follow-up.md).
