# Omission audit measurements

These measurements evaluate one product outcome: whether `related audit`
recovers a file omitted from a changed set. They do not claim improvements to
general editing quality, token use, or agent productivity.

## Method

`related eval --task audit` creates chronological leave-one-out tasks. For each
eligible held-out multi-file commit, one history-known file is hidden and the
remaining known files become the changed set. The evaluator checks whether the
audit recovers the hidden file.

Training commits are strictly older than test commits. Rename chains contained
inside training are canonicalized to the training-boundary path. A rename in
the current held-out commit maps only that commit's destination to its known
source path. Rename information from other held-out commits is unavailable, so
future and cross-holdout leakage are excluded.

## Three-repository holdout

Recorded on 2026-08-30 JST with direct ranking, `--top 5`, and
`--min-confidence medium`:

| repository | test/train | evaluated tasks | hit@5 | MRR |
|---|---:|---:|---:|---:|
| `related-cli` | 10/30 | 69 | 0.7101 | 0.5072 |
| `too_tired_to_type` | 50/200 | 162 | 0.7716 | 0.6825 |
| `vscode-edge-devtools` | 50/200 | 146 | 0.7123 | 0.5880 |

The evaluator excluded hidden targets that were unknown to the training window
and tasks that did not leave enough known seed files. Those exclusions are
reported by the command rather than counted as successful abstentions.

These tasks establish recovery of historical changed-set omissions. They do not
prove that every historically co-changed file was semantically required in a
new change.

## High-confidence enforcement calibration

Threshold sweeps compared the strongest changed-file/candidate pair across the
same chronological histories. A boundary of 20 co-changes produced too many
false candidates in `vscode-edge-devtools`; 30 removed every high candidate
from `too_tired_to_type`. The cross-repository compromise is 25:

| repository | evaluated tasks | high candidates | correct | candidate precision | task coverage |
|---|---:|---:|---:|---:|---:|
| `related-cli` | 66 | 0 | 0 | n/a | 0.0000 |
| `too_tired_to_type` | 162 | 13 | 8 | 0.6154 | 0.0802 |
| `vscode-edge-devtools` | 145 | 48 | 44 | 0.9167 | 0.3310 |
| **combined** | **373** | **61** | **52** | **0.8525** | **0.1635** |

The small repository correctly produced no high findings because its training
window could not establish a 25-occurrence pair. This is deliberate abstention.
The 61-candidate sample is too small to interpret `high` as a universal
probability, so enforcement remains explicit and repository-local evaluation is
recommended.

## Rename-aware omission recovery

The rename-aware rerun changed `vscode-edge-devtools` from 145 to 146 evaluated
tasks, reduced unknown hidden targets from 9 to 8, and increased recovered tasks
from 100 to 104. Hit@5 moved from 0.6897 to 0.7123 and MRR from 0.5625 to
0.5880.

Repository fixtures also cover:

- an exact audit that combines evidence before and after a content-changing
  rename;
- a staged rename that attributes old-path history to the visible new path;
- fast-mode recovery across two unambiguous content-identical renames;
- fast-mode abstention when two deleted sources have the same blob.

On the real `vscode-edge-devtools` R100 rename from
`src/host_beta/messageRouter.ts` to `src/host/messageRouter.ts`, fast and exact
returned identical top-five candidate paths and co-change counts.

## Audit latency

Twenty sequential runs audited a representative twenty-file changed set after
rename tracking was enabled:

| accuracy | median | p95 | max |
|---|---:|---:|---:|
| `fast` | 61.85 ms | 79.02 ms | 667.53 ms |
| `exact` | 114.84 ms | 122.64 ms | 126.43 ms |

Both p95 values met the provisional 500 ms warm local budget. Fast had one
cold/noisy maximum above the budget; this is not a cross-machine service-level
guarantee.

## Reproduce

```sh
related eval --task audit --repo ../too_tired_to_type \
  --test-commits 50 --train-commits 200 --top 5 \
  --modes direct,pagerank --min-confidence medium

related eval --task audit --repo ../vscode-edge-devtools \
  --test-commits 50 --train-commits 200 --top 5 \
  --modes direct,path --min-confidence low
```

Use a full clone when evaluating CI enforcement so the training window and
rename history are not truncated by a shallow checkout.
