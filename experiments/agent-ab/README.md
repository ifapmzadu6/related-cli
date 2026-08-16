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

Run the bundled 20-case benchmark against a local checkout of
`ifapmzadu6/too_tired_to_type`:

```sh
python3 scripts/agent_ab.py \
  --repo /path/to/too_tired_to_type \
  --cases experiments/agent-ab/too-tired-to-type-20.json \
  --output /tmp/related-agent-ab-results
```

The original three-case pilot remains in `too-tired-to-type.json`. The expanded
suite keeps those cases and adds cross-platform UI, notification persistence,
web concurrency, CI, and batch automation changes. It intentionally emphasizes
multi-file discovery tasks, which are the use case this project is designed to
help; it is not a random sample of all software-engineering work.

Recorded results:

- [Three-case pilot](results/2026-08-16-pilot.md)
- [Initial guardrail follow-up](results/2026-08-16-guardrail-follow-up.md)
- [20-case evaluation and corrected follow-up](results/2026-08-16-20-case-evaluation.md)

Use one or more `--case CASE_ID` arguments to run a subset of cases. Use
`--arm with-related` or `--arm without-related` for a targeted follow-up; normal
comparisons should run both arms.

Before a run, check the harness and case file without starting an agent:

```sh
python3 -m py_compile scripts/agent_ab.py
python3 scripts/agent_ab.py --help
python3 scripts/agent_ab.py \
  --repo /path/to/too_tired_to_type \
  --cases experiments/agent-ab/too-tired-to-type-20.json \
  --output /tmp/related-agent-ab-results \
  --validate-only
```

Add `--resume` to reuse completed case/arm pairs from an interrupted run. The
repository commit, case file and selection, arms, package, injected skill,
Codex version, and model metadata must match the original run.

The treatment arm runs `related-cli@0.4.1` before other repository inspection,
then applies the current skill guardrails: direct search must cover explicit
task targets, the seed query runs once, no more than one additional query may
resolve a genuinely independent surface, and task text overrides the ranking.
The control arm is prohibited from using co-change or Git-history based
related-file lookup. Arm order alternates between cases to reduce a simple
first/second-run bias.

Use `--treatment-skill skills/find-related-files` when evaluating a local skill
revision against historical commits. The harness injects that exact skill into
both arms while Codex runs, pins its `related-cli@latest` commands to the
requested package, and restores the checked-out version before scoring. This
keeps skill discovery constant while only the treatment prompt permits running
the history lookup.

Run `npm ci --prefix api` in the benchmark checkout before the bundled API case;
the harness shares that ignored dependency directory between disposable
worktrees. The API validation fixture bypasses the repository's integration-test
server and exercises the rate-limit and body-size acceptance criteria directly.

## Interpretation limits

Historical patch similarity is not identical to semantic correctness. An
equivalent implementation can score lower when its lines differ from the
recorded patch. Conversely, a close patch can still be wrong. Case-specific
hidden tests are therefore more important than line overlap when available.

`Target-file success` means the agent exited cleanly, all available checks
passed, and it changed every file in the reviewed historical target set. This
is the primary discovery metric, not a claim of semantic correctness. `Exact
patch success` is stricter and requires all added/deleted line units to match
the historical patch exactly.

This pilot is intentionally small and from one repository. It can detect large
effects and workflow failures, but it cannot establish a general improvement in
agent task success without more repositories, tasks, models, and repeated runs.

The first recorded run is in
[`results/2026-08-16-pilot.md`](results/2026-08-16-pilot.md). A targeted rerun
of its failed case with the explicit-scope guardrails is in
[`results/2026-08-16-guardrail-follow-up.md`](results/2026-08-16-guardrail-follow-up.md).
