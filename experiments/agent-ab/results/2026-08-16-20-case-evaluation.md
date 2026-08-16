# Codex editing A/B evaluation — 2026-08-16

## Decision

Do not make `related` a mandatory step for every agent task. In this evaluation
it did not improve target-file discovery accuracy, and it increased resource
use. Keep it as an optional context-expansion tool when the full companion-file
scope is uncertain.

The current skill was tightened accordingly: skip history when explicit targets
and direct search already resolve the edit set; otherwise run one seed query,
never repeat it, and allow at most one additional query for a genuinely
unresolved independent surface.

## Method

The harness ran Codex CLI 0.147.0 once per arm on 20 reviewed historical tasks
from `ifapmzadu6/too_tired_to_type` (40 trials total). Each trial started at the
parent of its target commit in a disposable worktree. The treatment arm had to
run `related-cli@0.4.1`; the control arm could use ordinary filename and source
search but not Git-history or co-change lookup. Arm order alternated by case.

The suite contains cross-platform UI, notification persistence, web
concurrency, API protection, CI, and automation changes. It deliberately favors
multi-file discovery work, where this project should have its best chance to
help. It is not a random sample of all software-engineering tasks.

The primary metric, target-file success, requires a clean agent exit, all
available checks to pass, and every reviewed historical target file to be
changed. It measures discovery coverage rather than complete semantic
correctness. Exact-patch success additionally requires all added and deleted
line units to match the historical patch. This dataset-and-repeatable-run shape
follows OpenAI's [agent evaluation guidance](https://developers.openai.com/api/docs/guides/agent-evals),
while the limitations below prevent a general causal claim.

## Primary 20-case result

| Metric | Without `related` | With `related` | Treatment difference |
|---|---:|---:|---:|
| Target-file success | 19/20 | 19/20 | 0 |
| Exact-patch success | 2/20 | 2/20 | 0 |
| Mean file precision | 97.08% | 95.75% | -1.33 points |
| Mean file recall | 98.33% | 98.33% | 0 points |
| Mean changed-line precision | 62.09% | 57.67% | -4.42 points |
| Mean changed-line recall | 62.58% | 63.98% | +1.40 points |
| Non-cached input tokens | 1,404,354 | 1,428,622 | +1.7% |
| Output tokens | 101,559 | 131,858 | +29.8% |
| Total elapsed time | 4,287.3s | 6,586.6s | +53.6% |
| Median elapsed time | 223.3s | 330.9s | +48.2% |

Paired target-file outcomes were 0 treatment wins, 0 losses, and 20 ties. Both
arms missed the same two of six historical files in the topics cost-protection
case, while both passed its four implementation-independent acceptance tests.
All other trials covered every historical target file.

Treatment was faster in 2 pairs and slower in 18. A descriptive two-sided sign
test gives `p = 0.000402`, but the tasks were manually selected and are not an
independent random sample. Non-cached input was lower in 9 treatment pairs and
higher in 11 (`p = 0.823803`). Changed-line similarity is especially weak as a
semantic metric because valid implementations can differ from the historical
patch.

## Trace finding and skill correction

The 20 treatment traces issued 58 `related query` commands: 48 distinct command
strings and 10 exact repeats, or 2.9 commands per task. One case issued eight.
The benchmark prompt encouraged another anchor for each independent surface,
historical checkouts exposed older installed skill text, and transient npm
lookup failures triggered some retries. These factors inflate the primary
efficiency penalty, though they do not change the 20/20 paired accuracy ties.

The harness now supports `--treatment-skill` to inject one exact local skill
revision into both arms during execution and restore the historical checkout
before scoring. It fingerprints that skill for safe `--resume` behavior and
records total and duplicate query commands. The skill and treatment prompt now
default to one query, prohibit identical repeats, and permit at most one extra
query after direct search leaves an independent surface unresolved.

## Five-case corrected follow-up

Five representative cases were rerun in both arms with the revised local skill
injected. The subset included the earlier eight-query case, a prior apparent
efficiency win, cross-platform behavior, CI, and automation work. All five
treatment runs issued exactly one query, with no duplicates.

| Metric | Without `related` | With `related` | Treatment difference |
|---|---:|---:|---:|
| Target-file success | 5/5 | 5/5 | 0 |
| Exact-patch success | 1/5 | 1/5 | 0 |
| Mean file precision | 100% | 100% | 0 points |
| Mean file recall | 100% | 100% | 0 points |
| Non-cached input tokens | 304,521 | 343,997 | +13.0% |
| Output tokens | 22,663 | 28,195 | +24.4% |
| Total elapsed time | 936.2s | 1,240.5s | +32.5% |
| Median elapsed time | 209.7s | 227.9s | +8.6% |

Treatment used fewer non-cached tokens and less time in one pair, and more in
four. The query-count fix therefore worked, but this subset still showed no
accuracy gain and no aggregate efficiency gain. This is a diagnostic rerun of
selected cases, not a new independent sample, and ordinary agent-run variance
can affect its timing and token totals.

## Interpretation

The evidence supports a narrower claim than “this makes agents more accurate”:
Git co-change ranking can supply useful candidate context, but it was redundant
for these detailed tasks because direct search already exposed the named
platforms, components, and files. Forcing the lookup on every task added work
without changing target coverage.

The best current product policy is:

- Use `related` when a task starts from one or a few files and likely companion
  tests, configs, docs, migrations, or platform equivalents are not yet known.
- Skip it when the complete target set is explicit and direct search resolves
  that set.
- Treat rankings as candidates, never as an authoritative edit plan.
- Verify explicit task nouns directly and use tests as the semantic authority.
- Measure future gains on under-specified issue tasks, additional repositories,
  multiple agent runs, and stronger hidden acceptance tests.

## Reproduction

Validate and run the full suite:

```sh
python3 scripts/agent_ab.py \
  --repo /path/to/too_tired_to_type \
  --cases experiments/agent-ab/too-tired-to-type-20.json \
  --output /tmp/related-agent-ab-results \
  --treatment-skill skills/find-related-files \
  --validate-only

python3 scripts/agent_ab.py \
  --repo /path/to/too_tired_to_type \
  --cases experiments/agent-ab/too-tired-to-type-20.json \
  --output /tmp/related-agent-ab-results \
  --related-package related-cli@0.4.1 \
  --treatment-skill skills/find-related-files
```

Use `--resume` after an interrupted run. Case files are trusted input because
their validation commands execute locally.
