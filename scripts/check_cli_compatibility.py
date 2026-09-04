#!/usr/bin/env python3
"""Compare two related binaries against the same isolated Git history."""

import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def check_compatibility(baseline, candidate, repo):
    env = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}
    commits = 0
    checks = 0

    def git(*args):
        subprocess.run(["git", "-C", str(repo), *args], env=env, check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def commit(files):
        nonlocal commits
        for name, content in files.items():
            path = repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        commits += 1
        git("add", ".")
        git("commit", "-m", f"fixture {commits}")

    def compare(*args):
        nonlocal checks
        outputs = []
        for binary in (baseline, candidate):
            result = subprocess.run([str(binary), *args], cwd=repo, env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=30)
            outputs.append((result.returncode, result.stdout, result.stderr))
        if outputs[0] != outputs[1]:
            raise AssertionError(f"CLI mismatch for {args!r}\nbaseline={outputs[0]!r}\ncandidate={outputs[1]!r}")
        checks += 1

    def queries():
        for output in ("text", "json"):
            for mode in ("direct", "pagerank", "path", "hot"):
                compare("query", "src/a.md", "--mode", mode, "--format", output,
                        "--evidence", "3", "--top", "3")
            for accuracy in ("fast", "exact"):
                compare("explain", "src/a.md", "tests/b.md", "--accuracy", accuracy,
                        "--format", output)
                compare("query", "src/a.md", "--accuracy", accuracy, "--format", output,
                        "--exclude", "tests/*", "--top", "1")
        for backend in ("git", "git-remove-empty", "pack-fast", "pack-scan", "hybrid",
                        "gix", "git-batch", "git-batch-parallel", "git-diff-tree",
                        "git-diff-tree-parallel", "git-rev-list"):
            compare("query", "src/a.md", "--history-backend", backend, "--format", "json",
                    "--jobs", "2", "--evidence", "2")
        for task in ("audit", "query"):
            for modes in ("direct", "direct,pagerank,path,hot"):
                compare("eval", "--task", task, "--test-commits", "5", "--train-commits", "25",
                        "--modes", modes, "--format", "json")

    def audits():
        for accuracy in ("fast", "exact"):
            for output in ("text", "json"):
                for scope in ((), ("--staged",), ("--range", "audit-base..HEAD"),
                              ("--range", "audit-base...HEAD")):
                    compare("audit", *scope, "--accuracy", accuracy, "--format", output,
                            "--evidence", "2", "--fail-on-confidence", "high")
                compare("audit", "--mode", "pagerank", "--min-confidence", "low",
                        "--accuracy", accuracy, "--format", output)
                compare("diff", "--accuracy", accuracy, "--format", output)
                compare("diff", "--staged", "--accuracy", accuracy, "--format", output)

    git("init")
    git("config", "user.email", "compatibility@example.invalid")
    git("config", "user.name", "Compatibility Test")
    for i in range(30):
        files = {"src/a.md": f"a {i}\n", "tests/b.md": f"b {i}\n"}
        if i % 3 == 0:
            files["docs/日本語.md"] = f"docs {i}\n"
        commit(files)
        if i == 24:
            git("tag", "audit-base")
    commit({"unrelated.md": "unrelated\n"})
    for args in ((), ("--help",), ("--version",), ("unknown",),
                 ("query", "missing.md"), ("query", "src/a.md", "--top", "0"),
                 ("query", "src/a.md", "--half-life-days", "NaN"),
                 ("audit", "--staged", "--range", "audit-base..HEAD"),
                 ("audit", "--min-confidence", "high", "--fail-on-confidence", "low"),
                 ("audit", "--accuracy", "exact", "--history-backend", "git"),
                 ("eval", "--task", "invalid")):
        compare(*args)
    for command in ("query", "audit", "diff", "explain", "eval"):
        compare(command, "--help")
    queries()
    (repo / "src/a.md").write_text("staged\n")
    git("add", "src/a.md")
    audits()
    (repo / "src/a.md").write_text("unstaged\n")
    (repo / "new.md").write_text("untracked\n")
    audits()
    git("checkout", "HEAD", "--", "src/a.md")
    git("mv", "src/a.md", "src/renamed.md")
    (repo / "src/renamed.md").write_text("edited rename\n")
    audits()
    commit({})
    git("mv", "src/renamed.md", "src/a.md")
    commit({})
    git("rm", "tests/b.md")
    commit({})
    (repo / "src/a.md").write_text("after deletion\n")
    audits()
    # Exercise packed objects as well as loose-object reads.
    git("gc", "--prune=now")
    queries()
    audits()
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    args = parser.parse_args()
    baseline, candidate = args.baseline.resolve(), args.candidate.resolve()
    with tempfile.TemporaryDirectory(prefix="related-compatibility-") as directory:
        checks = check_compatibility(baseline, candidate, Path(directory))
    print(f"CLI compatibility passed: {checks} identical exit codes, stdout, and stderr")


if __name__ == "__main__":
    main()
