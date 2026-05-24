# Comparison

Checked on 2026-05-24 JST from public product pages, docs, and repository
descriptions; npm package versions and local Codegraph/token measurements were
rechecked on 2026-05-25 JST. This is a feature comparison, not an
apples-to-apples benchmark: most comparable products are hosted, MCP-first, or
multi-signal systems that cannot be run locally in the same VS Code holdout
benchmark without extra setup.

## Summary

`related` is closest to LaserOwl, Glaux, Sourcebook, Qartez, repowise, Codegraph,
and CodeScene in that all of them use some form of change coupling, co-change, or
repository graph intelligence.

The narrower position for `related` is:

- local Rust CLI
- Git co-change history only
- no source parsing, imports, AST, symbols, embeddings, or file contents
- direct co-change and Personalized PageRank over the co-change graph
- `related query <file>` JSON output for LLM tools
- `related explain <a> <b>` evidence commits
- built-in holdout evaluation with `direct`, `pagerank`, and `path` baselines

That makes it weaker than the larger systems for full code intelligence, but
cleaner for the specific job of "given this file, what should an agent read next
based only on historical co-edits?"

## Feature Matrix

| Tool | Publicly described purpose | Local CLI | MCP / agent surface | Uses Git co-change | Uses source parsing | PageRank / graph centrality | Built-in related-file query | Built-in holdout eval | Notes |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| `related` | Rank related files from Git co-change history | Yes | Not yet | Yes | No | Yes, Personalized PageRank on co-change graph | Yes | Yes | Single-purpose, content-blind, works for docs/configs/prompts as well as code |
| LaserOwl | Catch missing files in an AI agent's plan using commit history | Not clear from public page | Yes | Yes | Not emphasized | Not publicly clear | Plan-level `evaluate_plan`, not a simple CLI query | Not publicly clear | Very close problem framing; appears more product/hosted and plan-completeness oriented |
| Glaux | MCP/REST graph intelligence and risk context for coding agents | Not clear from public page | Yes | Yes | Yes | Yes | Agent context block, not a small Unix-style query CLI | Not publicly clear | Combines Git, static, and semantic graphs; explicitly broader than `related` |
| Sourcebook | Check diffs for files an AI agent forgot to change | Yes | Yes | Yes | Yes | Yes, import-graph PageRank | Diff completeness check | Public benchmarks mentioned, methodology separate | Strong overlap on "forgotten files"; not content-blind and not just file-to-file retrieval |
| Qartez | Code intelligence MCP server for agents | CLI plus MCP is described | Yes | Yes | Yes | Yes | Context/impact tools | Benchmarks described | Rust binary, but much broader: tree-sitter, blast radius, complexity, clone detection |
| repowise | Codebase intelligence, docs, graph, Git history, MCP tools | Yes | Yes | Yes | Yes | Yes, on dependency graph | Context/risk tools | Not clear from public page | Broader codebase documentation and graph system |
| Codegraph | Parse code into graph DB and expose MCP tools | Yes | Yes | Yes | Yes | Not clear from public page | `codegraph co-change <file>` is described | Not clear from public page | Has an explicit co-change command, but is part of a source graph system |
| CodeScene | Technical debt / behavioral code analysis platform | Product UI/API, not small CLI | Not positioned as an LLM tool | Yes | Yes / broader analysis | Temporal-coupling graph | Temporal coupling views | Not focused on LLM holdout eval | Origin-adjacent concept: temporal/change coupling from developer behavior |

## Closest Matches

### LaserOwl

LaserOwl is the closest product-level match in problem statement. Its docs say it
analyzes commit history, builds a co-change index, and exposes an MCP
`evaluate_plan` call for agents before editing. The main difference is shape:
LaserOwl is plan-completeness and delivery-risk oriented, while `related` is a
small local query tool.

Source: https://docs.laserowl.io/

### Glaux

Glaux also targets the exact "agent should know what it is missing before
editing" moment. It returns a decision-ready context block over MCP/REST and
combines Git history, static dependency analysis, semantic clustering, PageRank,
centrality, and co-change trends. That is broader than `related`; it is not
content-blind.

Sources:

- https://www.glaux.dev/
- https://www.glaux.dev/docs

### Sourcebook

Sourcebook overlaps strongly on "files your AI agent forgot to change" and has
CLI, hooks, and MCP surfaces. Its public page describes import graphs, Git
co-change history, test-file mapping, convention detection, and import-graph
PageRank. `related` intentionally excludes those non-history signals.

Source: https://sourcebook.run/

### Qartez

Qartez is also Rust and agent-oriented. Public descriptions emphasize PageRank,
blast radius, co-change, hotspots, clone detection, and Tree-sitter parsing
across many languages. It competes more as a full code-intelligence MCP server
than as a content-blind related-file CLI.

Sources:

- https://github.com/kuberstar/qartez-mcp
- https://mcpservers.org/servers/kuberstar/qartez-mcp

### repowise

repowise has CLI, web UI, and MCP surfaces. It indexes dependency graphs, Git
history, documentation, decisions, ownership, and co-change partners. `related`
is smaller and does not generate docs or parse code.

Sources:

- https://www.repowise.dev/
- https://docs.repowise.dev/docs/getting-started/quickstart
- https://www.repowise.dev/architecture

### Codegraph

This comparison refers to `@optave/codegraph` / `optave/ops-codegraph-tool`.
Its public README describes a local Tree-sitter source graph stored in SQLite,
CLI and MCP access, function-level dependency analysis, imports/callers,
dataflow, CFG, semantic search, role classification, CI gates, incremental
rebuilds, and Git co-change.

Codegraph is the closest public command-level match because it has
`codegraph co-change <file>`. On the VS Code target measured below, Codegraph
and `related` returned the same top companion file. The difference is scope:
Codegraph's co-change feature is part of a full source graph system; `related`
is only the historical companion-file query and does not build source state.

Sources:

- https://github.com/optave/ops-codegraph-tool
- https://www.npmjs.com/package/@optave/codegraph

### CodeScene

CodeScene is the established reference point for temporal/change coupling. Its
docs define temporal coupling as modules changing together over time and include
same-commit modification as the strongest level of coupling. It is not framed as
a lightweight LLM context-selection CLI.

Sources:

- https://docs.enterprise.codescene.io/versions/3.5.22/guides/technical/temporal-coupling.html
- https://codescene.com/blog/change-coupling-visualize-the-cost-of-change

## Positioning Result

There are already close tools. The open space for `related` is not "no one uses
co-change for agents." The open space is a smaller, composable tool:

```sh
related query path/to/file --json
related explain path/to/file other/file
related eval
```

The practical bet is that many agents do not need a whole code-intelligence
stack before every edit. They need a cheap first call that answers:

```text
Based on how this repository actually evolved, what else should I inspect?
```

That scope also keeps `related` usable outside source code, where AST and import
graphs do not apply.

## Speed Positioning

Some external products are hosted or MCP-first, but two public CLIs could be run
locally on the VS Code checkout: Codegraph and Sourcebook. Their public surfaces
are not identical to `related`, so the claim below is scoped to the overlapping
task: quickly return companion files for a given file using historical co-change
or preflight context.

On `microsoft/vscode`, target
`src/vs/platform/sandbox/common/terminalSandboxEngine.ts`, history since
`2026-05-08T14:03:23-07:00`:

- `related query` x20: on-demand timings are tracked in `MEASUREMENTS.md`
- `codegraph build` with optional heavy analyses disabled: `158.12s`, `.codegraph`
  `527 MiB`
- `codegraph co-change --analyze`: `2.84s`
- `related query <file>` x20 on the same target in the current run: `1.14s`,
  about `0.057s/query`
- `codegraph co-change <file>` x20 in the current run: `6.21s`, about
  `0.311s/query`
- `sourcebook scan-history`: `3.15s`
- `sourcebook preflight --file` x20: `174.68s`, about `8.734s/query`

Both Codegraph and `related` returned the same top companion file for the target:
`src/vs/platform/sandbox/test/common/terminalSandboxEngine.test.ts`.

This supports a narrow speed claim: for the file-to-related-files co-change
lookup, `related` is faster and much smaller because it does not build source
graphs or run broader preflight analysis.

## Token Positioning

Token-efficiency claims need to be scoped carefully because nearby tools answer
different questions. Codegraph has token-efficient source summaries such as
`brief`, while `related` returns only a historical file shortlist.

On the same VS Code target used for speed measurements, with `tiktoken`
`cl100k_base`:

| artifact | tokens | interpretation |
|---|---:|---|
| `related query` compact text output, top 10 | `231` | on-demand related-file shortlist |
| `related query --json`, top 10 | `980` | structured tool output |
| paths extracted from `related --json` | `170` | smallest file-name-only shortlist |
| `codegraph co-change` text output, top 10 | `331` | co-change table after DB/co-change setup |
| `codegraph co-change --json`, top 10 | `797` | structured co-change output |
| `codegraph brief --json` for the target | `4,328` | source summary, not a co-change query |
| raw target file | `8,625` | source read |
| raw target + top companion test | `14,179` | common first inspection pair |
| raw top 10 companion files | `37,436` | speculative broad read |
| raw target + top 10 companion files | `46,061` | broad speculative read |

The scoped result: `related`'s compact default text output is slightly smaller
than Codegraph's text co-change table on this target, and it does not require a
source-graph database. The larger token advantage is the workflow shape: a
no-index, roughly 230-token shortlist can stop an agent from spending 14k-46k
tokens opening source files before it knows which files matter. When the agent
only needs file names, paths extracted from `related --json` were about 170
tokens.

A five-target VS Code sweep over recent code files showed the same direction:
`related` compact text was smaller in all five rows, with median `231` tokens vs
Codegraph `331`. The same sweep had median query latency `25.3ms` for `related`
vs `159.3ms` for Codegraph after Codegraph's `.codegraph` database and
co-change analysis already existed. For `package.json`, Codegraph returned no
co-change data while `related` returned manifest and lockfile partners, which is
the practical value of staying content-blind.

The local apples-to-apples implementation test also compares `related` with
implementations that compute the same co-change answer directly from Git history
on each query. The current CLI no longer has a persistent index path, so these
measurements are on-demand.

On `microsoft/vscode`, using `package.json`, `--mode direct`, `--top 5`, and
`--max-commits 1000`:

- `related` default `pack-fast`: `0.0926s/query` median in the latest
  randomized-order run, same top-5 path set as Git exact but approximate counts
- `related` exact compact `git --no-renames`: `0.3695s/query` median in the
  same run
- `related` pack-only `pack-scan --scan-commits 17500`: not rerun in the latest
  pass; the previous median was `0.5181s/query`, with the same top-5 path set as
  Git exact
- `related` pack-only `pack-scan --scan-commits 17500 --jobs 8`:
  `0.2007s/query` median, same top-5 path set as Git exact
- earlier compact `git --no-renames`: `0.4104s/query` median in the previous
  randomized-order run before the pack-only default
- previous `git-diff-tree` default: `0.4483s/query` median in that earlier run

For the same target, `--max-commits 200` was much faster (`0.1351s` median) but
only captured `134` co-changes for the top companion, while `1000` captured
`410`. `--max-commits 5000` exceeded two minutes and was rejected as a default.

So the measured speed claim is narrow: for the file-to-related-files co-change
lookup, `related` keeps setup at zero storage and makes an on-demand Git-history
query fast enough for an LLM tool call, while avoiding source parsing and hosted
services.
