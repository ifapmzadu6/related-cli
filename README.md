# related

`related` is a small CLI that ranks files related to a target file using only Git
co-change history.

It intentionally does not parse source code, imports, symbols, embeddings, or file
contents. That makes the signal language-agnostic and useful for code, docs,
configs, migrations, prompts, runbooks, and any other files tracked in Git.

## Idea

If two files are repeatedly changed in the same commit, they probably carry an
operational relationship. `related` turns that history into a weighted graph:

- file = node
- same-commit change = edge
- large commits are down-weighted
- older commits are time-decayed

Queries can use either direct co-change ranking or Personalized PageRank over the
co-change graph.

## Skill Installation

The npm package ships a portable skill at `skills/find-related-files`. The skill
does not vendor the binary and does not require a global CLI install; it calls
the published npm package with `npx -y --package related-cli@latest related ...`.

Run the installer from the target project root to copy the skill folder into
your agent's skill directory. Re-run the same command later to refresh the
copied skill instructions.

### Codex

Project install is recommended for shared repositories:

```sh
npx -y --package related-cli@latest related-install-skill
```

Run it from the target project root. It copies the skill to
`.codex/skills/find-related-files` in the current project. Commit that directory
when you want the same pre-edit related-file workflow to travel with the
repository.

User-level install is also available as an explicit option, but is mainly useful
for local experiments:

```sh
npx -y --package related-cli@latest related-install-skill --user
```

Restart Codex after installing the skill.

### Claude Code

Project install is recommended for shared repositories:

```sh
npx -y --package related-cli@latest related-install-skill claude
```

Run it from the target project root. It copies the skill to
`.claude/skills/find-related-files` in the current project.

User-level install, available in all projects:

```sh
npx -y --package related-cli@latest related-install-skill claude --user
```

Claude Code can load the skill automatically from its description, or you can
invoke it directly with `/find-related-files`.

## CLI

The skill is the intended entry point for agent use. The CLI remains available
for direct checks:

```sh
npx -y --package related-cli@latest related query src/auth.ts --top 20
npx -y --package related-cli@latest related diff --staged --top 20
```

By default commands run against the current directory's Git repository. Use
`--repo PATH` only when querying another checkout from outside that repository.
No persistent index is created; the graph is built on demand from the target
file's Git history.

The default backend, `pack-fast`, is optimized for low-latency agent calls in
large repositories and may stop before an exact full history walk. Use
`--history-backend git` when exact Git history is more important than speed, or
`--history-backend pack-scan` for a deeper pack-only scan.

If the top results all look like broad release, dependency, formatting, or
initial-commit churn, inspect evidence and retry with a tighter commit-size
filter plus result exclusions before trusting the ranking:

```sh
npx -y --package related-cli@latest related query src/auth.ts --top 20 --evidence 3
npx -y --package related-cli@latest related query src/auth.ts --top 20 --max-files-per-commit 10 --exclude '*.lock,*-lock.*,*lockb,.github/workflows/*'
```

For compact LLM-tool output, `query` and `diff` omit per-commit evidence by
default. Add `--evidence N` when example commits would help, or use
`explain file-a file-b` for one focused relationship.

## Comparison

`related` sits near tools such as LaserOwl, Glaux, Sourcebook, Qartez, repowise,
Codegraph, and CodeScene. Those systems validate the same underlying idea:
repository history is a useful signal for agent context, missed-file detection,
and change-risk analysis.

The difference is shape. Most nearby tools are broader code-intelligence
systems: they parse code, build indexes or databases, expose hosted or MCP
surfaces, combine semantic/static/history signals, or evaluate a whole edit
plan. That breadth is valuable, but it is also heavier than what an LLM often
needs immediately before touching one file.

`related` is intentionally small:

- one local Rust binary
- no persistent index
- no source parsing, embeddings, imports, ASTs, or file contents
- works for docs, prompts, configs, migrations, and code
- returns compact JSON suitable for an LLM tool call
- can show evidence commits when the agent needs to verify the relationship

The bet is that Git history is already a behavior graph of the project. If two
files repeatedly changed together, the repository is telling the agent, "look
here too." `related` makes that signal cheap enough to call before ordinary
edits, especially in large workspaces where grep finds text matches but not
operational coupling.

The closest public CLIs that were installed and measured locally were
`@optave/codegraph` and `sourcebook`.

Measured on `microsoft/vscode`:

- repo: `/tmp/related-vscode`
- target: `src/vs/platform/sandbox/common/terminalSandboxEngine.ts`
- history window: since `2026-05-08T14:03:23-07:00`
- Codegraph version: `3.10.0`
- Sourcebook version: `0.14.0`

| tool / command | measured task | time | storage |
|---|---|---:|---:|
| `related query` | Query related files on demand | `0.082s/query` median in the latest same-target `pack-fast` run | none |
| `codegraph build` | Build Codegraph DB before co-change works | `158.12s` | `528 MiB` |
| `codegraph co-change --analyze` | Populate co-change data | `2.84s` | same DB |
| `codegraph co-change <file>` x20 | Query co-change partners | `3.76s` total, `0.188s/query` | same DB |
| `sourcebook scan-history` | Scan history for co-change pairs | `3.15s` | output only |
| `sourcebook preflight --file` x20 | Suggest companion files before editing | `174.68s` total, `8.734s/query` | no persistent index used here |

For the same VS Code target, both `related` and Codegraph ranked the same top
companion file:

```text
src/vs/platform/sandbox/test/common/terminalSandboxEngine.test.ts
```

This is the practical advantage: `related` gets the high-value co-change answer
without asking the agent to wait for a source graph, database, hosted service,
or broad preflight scan. It does not replace those systems; it gives agents a
small first move that is fast, local, language-agnostic, and easy to compose
with grep, type checks, tests, or larger code-intelligence tools.

See [COMPARISON.md](COMPARISON.md) for a broader public-docs-based comparison
against nearby tools.

See [MEASUREMENTS.md](MEASUREMENTS.md) for empirical measurements across
accuracy, top-K behavior, history window size, large-commit filtering, query
latency, multiple repositories, and speed against on-demand Git history
implementations. It also documents the built-in holdout evaluation used to
compare co-change ranking against path and hot-file baselines.

To reproduce the comparison against public third-party CLIs on VS Code:

```sh
scripts/external_tool_compare.sh /tmp/related-vscode
```

To reproduce the speed comparison against similar local mechanisms:

```sh
scripts/speed_compare.py \
  --repo /tmp/related-vscode \
  --target package.json
```

## License

MIT
