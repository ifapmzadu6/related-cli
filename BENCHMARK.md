# Benchmark

This records the first effectiveness check for `related`.

## Setup

Large repository:

- Repository: `microsoft/vscode`
- Local path used: `/tmp/related-vscode`
- Files: `14,838`
- Commits visible to Git: `155,862`

Clone command used:

```sh
git clone --filter=blob:none --depth=1500 https://github.com/microsoft/vscode.git /tmp/related-vscode
```

Evaluation command:

```sh
target/release/related eval \
  --repo /tmp/related-vscode \
  --test-commits 200 \
  --train-commits 1000 \
  --top 10 \
  --max-files-per-commit 80
```

The newest 200 commits were held out as test data. The older 1000 commits were
used to build an in-memory co-change graph for the evaluator. For each held-out
commit, the evaluator gives the tool one known changed file and checks whether
the other known files from that same commit appear in the top 10.

The `path` mode is a content-blind path/name similarity baseline. It is not grep;
grep requires a text query, while this evaluation starts from a file path.

## Results

```text
candidate_tasks=767 evaluated_tasks=544 skipped_unknown_seed=223 skipped_no_known_target=0

mode          tasks      hit@k  precision@k   recall@k        mrr avg_results
direct          544     0.7188       0.1849     0.2955     0.4978        8.96
hot             544     0.2831       0.0607     0.0625     0.1379       10.00
pagerank        544     0.7868       0.2445     0.3460     0.5827        9.63
path            544     0.5460       0.1165     0.1822     0.3205       10.00
```

On this run, Personalized PageRank over the Git co-change graph improved over the
path/name baseline by:

- `hit@10`: `0.7868` vs `0.5460`
- `precision@10`: `0.2445` vs `0.1165`
- `recall@10`: `0.3460` vs `0.1822`
- `MRR`: `0.5827` vs `0.3205`

## Query Smoke Test

Example:

```sh
target/release/related query package.json \
  --repo /tmp/related-vscode \
  --mode pagerank \
  --top 10
```

Top results included `package-lock.json`, `remote/package-lock.json`,
`remote/package.json`, and `extensions/copilot/package.json`, all found from
history rather than source parsing.
