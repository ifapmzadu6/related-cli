#!/usr/bin/env python3
import argparse
import collections
import json
import math
import subprocess
import time
from pathlib import Path


def run(args, cwd=None):
    return subprocess.check_output(args, cwd=cwd)


def git(repo, *args):
    return run(["git", "-C", repo, *args])


def parse_git_log(raw):
    commits = []
    for record in raw.decode("utf-8", errors="replace").split("\x1e"):
        record = record.strip()
        if not record:
            continue
        lines = record.splitlines()
        fields = lines[0].split("\x1f", 3)
        if len(fields) != 4:
            continue
        files = sorted({normalize_path(line) for line in lines[1:] if normalize_path(line)})
        commits.append(
            {
                "hash": fields[0],
                "time": int(fields[1]),
                "date": fields[2],
                "subject": fields[3],
                "files": files,
            }
        )
    return commits


def normalize_path(path):
    return "/".join(part for part in path.strip().replace("\\", "/").split("/") if part and part != ".")


def pair_key(a, b):
    return (a, b) if a <= b else (b, a)


def time_decay(latest, when, half_life_days):
    if half_life_days <= 0:
        return 1.0
    age_days = max(0, latest - when) / 86_400.0
    return math.exp(-math.log(2) * age_days / half_life_days)


def rank_direct_from_commits(commits, target, max_files, half_life_days):
    latest = max((commit["time"] for commit in commits), default=0)
    file_weight = collections.defaultdict(float)
    pair_weight = collections.defaultdict(float)
    pair_count = collections.defaultdict(int)
    last_seen = {}

    for commit in commits:
        files = commit["files"]
        if not files or len(files) > max_files:
            continue
        decay = time_decay(latest, commit["time"], half_life_days)
        for file in files:
            file_weight[file] += decay
            if commit["date"] > last_seen.get(file, ""):
                last_seen[file] = commit["date"]
        if target not in files:
            continue
        edge_weight = decay / math.log2(len(files) + 1)
        for other in files:
            if other == target:
                continue
            key = pair_key(target, other)
            pair_weight[key] += edge_weight
            pair_count[key] += 1

    results = []
    target_weight = file_weight.get(target, 0.0)
    for key, weight in pair_weight.items():
        other = key[1] if key[0] == target else key[0]
        other_weight = file_weight.get(other, 0.0)
        if target_weight <= 0 or other_weight <= 0:
            score = weight
        else:
            score = weight / math.sqrt(target_weight * other_weight)
        results.append((score, other, pair_count[key], last_seen.get(other, "")))
    results.sort(key=lambda item: (-item[0], item[1]))
    return results


def build_graph_from_commits(commits, max_files, half_life_days):
    latest = max((commit["time"] for commit in commits), default=0)
    file_weight = collections.defaultdict(float)
    pair_weight = collections.defaultdict(float)
    pair_count = collections.defaultdict(int)
    last_seen = {}
    adj = collections.defaultdict(lambda: collections.defaultdict(float))
    degree = collections.defaultdict(float)

    for commit in commits:
        files = commit["files"]
        if not files or len(files) > max_files:
            continue
        decay = time_decay(latest, commit["time"], half_life_days)
        for file in files:
            file_weight[file] += decay
            if commit["date"] > last_seen.get(file, ""):
                last_seen[file] = commit["date"]
        if len(files) < 2:
            continue
        edge_weight = decay / math.log2(len(files) + 1)
        for i, left in enumerate(files):
            for right in files[i + 1 :]:
                key = pair_key(left, right)
                pair_weight[key] += edge_weight
                pair_count[key] += 1
                adj[left][right] += edge_weight
                adj[right][left] += edge_weight
                degree[left] += edge_weight
                degree[right] += edge_weight

    return {
        "file_weight": file_weight,
        "pair_weight": pair_weight,
        "pair_count": pair_count,
        "last_seen": last_seen,
        "adj": adj,
        "degree": degree,
    }


def rank_pagerank_from_graph(graph, target):
    alpha = 0.85
    rank = {target: 1.0}
    for _ in range(30):
        nxt = {target: 1.0 - alpha}
        for node, value in rank.items():
            degree = graph["degree"].get(node, 0.0)
            if degree == 0.0:
                nxt[target] = nxt.get(target, 0.0) + alpha * value
                continue
            for neighbor, weight in graph["adj"].get(node, {}).items():
                nxt[neighbor] = nxt.get(neighbor, 0.0) + alpha * value * weight / degree
        rank = nxt

    results = []
    for path, score in rank.items():
        if path == target or score <= 0:
            continue
        key = pair_key(target, path)
        results.append(
            (
                score,
                path,
                graph["pair_count"].get(key, 0),
                graph["last_seen"].get(path, ""),
            )
        )
    results.sort(key=lambda item: (-item[0], item[1]))
    return results


def on_demand_log_scan(repo, target, max_commits, max_files, half_life_days):
    raw = git(
        repo,
        "log",
        "--name-only",
        "--diff-filter=ACMRT",
        f"--max-count={max_commits}",
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s",
    )
    commits = parse_git_log(raw)
    return rank_direct_from_commits(commits, target, max_files, half_life_days)


def on_demand_log_pagerank(repo, target, max_commits, max_files, half_life_days):
    raw = git(
        repo,
        "log",
        "--name-only",
        "--diff-filter=ACMRT",
        f"--max-count={max_commits}",
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s",
    )
    commits = parse_git_log(raw)
    graph = build_graph_from_commits(commits, max_files, half_life_days)
    return rank_pagerank_from_graph(graph, target)


def git_show_loop(repo, target, max_commits, max_files, half_life_days):
    hashes = git(repo, "rev-list", f"--max-count={max_commits}", "HEAD").decode().splitlines()
    commits = []
    for commit_hash in hashes:
        raw = git(
            repo,
            "show",
            "--name-only",
            "--diff-filter=ACMRT",
            "--pretty=format:%H%x1f%ct%x1f%cI%x1f%s",
            "--no-renames",
            commit_hash,
        )
        parsed = parse_git_log(b"\x1e" + raw)
        if parsed:
            commits.append(parsed[0])
    return rank_direct_from_commits(commits, target, max_files, half_life_days)


def related_query(
    binary,
    repo,
    target,
    mode,
    top,
    max_commits,
    max_files_per_commit,
    half_life_days,
):
    raw = run(
        [
            binary,
            "query",
            target,
            "--repo",
            repo,
            "--mode",
            mode,
            "--top",
            str(top),
            "--max-commits",
            str(max_commits),
            "--max-files-per-commit",
            str(max_files_per_commit),
            "--half-life-days",
            str(half_life_days),
            "--json",
        ]
    )
    data = json.loads(raw)
    return [(item["score"], item["path"], item.get("cochanges", 0), item.get("last_seen", "")) for item in data["related"]]


def measure(label, runs, fn):
    started = time.perf_counter()
    result = None
    for _ in range(runs):
        result = fn()
    elapsed = time.perf_counter() - started
    top = result[0][1] if result else ""
    return {
        "label": label,
        "runs": runs,
        "total": elapsed,
        "per_query": elapsed / runs if runs else 0,
        "top_result": top,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default="/tmp/related-vscode")
    parser.add_argument("--binary", default="target/release/related")
    parser.add_argument("--target", default="package.json")
    parser.add_argument("--top", type=int, default=10)
    parser.add_argument("--max-commits", type=int, default=1000)
    parser.add_argument("--max-files-per-commit", type=int, default=80)
    parser.add_argument("--half-life-days", type=float, default=365.0)
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--slow-runs", type=int, default=1)
    parser.add_argument("--output", default="")
    args = parser.parse_args()

    rows = [
        measure(
            "related-direct-on-demand",
            args.runs,
            lambda: related_query(
                args.binary,
                args.repo,
                args.target,
                "direct",
                args.top,
                args.max_commits,
                args.max_files_per_commit,
                args.half_life_days,
            ),
        ),
        measure(
            "related-pagerank-on-demand",
            args.runs,
            lambda: related_query(
                args.binary,
                args.repo,
                args.target,
                "pagerank",
                args.top,
                args.max_commits,
                args.max_files_per_commit,
                args.half_life_days,
            ),
        ),
        measure(
            "on-demand-git-log-scan",
            args.runs,
            lambda: on_demand_log_scan(
                args.repo,
                args.target,
                args.max_commits,
                args.max_files_per_commit,
                args.half_life_days,
            )[: args.top],
        ),
        measure(
            "on-demand-git-log-pagerank",
            args.runs,
            lambda: on_demand_log_pagerank(
                args.repo,
                args.target,
                args.max_commits,
                args.max_files_per_commit,
                args.half_life_days,
            )[: args.top],
        ),
    ]
    if args.slow_runs > 0:
        rows.append(
            measure(
                "git-show-per-commit-loop",
                args.slow_runs,
                lambda: git_show_loop(
                    args.repo,
                    args.target,
                    args.max_commits,
                    args.max_files_per_commit,
                    args.half_life_days,
                )[: args.top],
            )
        )

    lines = [
        "# Speed Comparison",
        "",
        f"- repo: `{args.repo}`",
        f"- target: `{args.target}`",
        f"- max commits: `{args.max_commits}`",
        f"- max files per commit: `{args.max_files_per_commit}`",
        "- persistent index: `none`",
        "",
        "| mechanism | runs | total seconds | seconds/query | top result |",
        "|---|---:|---:|---:|---|",
    ]
    for row in rows:
        lines.append(
            f"| {row['label']} | {row['runs']} | {row['total']:.4f} | {row['per_query']:.4f} | `{row['top_result']}` |"
        )
    output = "\n".join(lines) + "\n"
    if args.output:
        Path(args.output).write_text(output)
    print(output, end="")


if __name__ == "__main__":
    main()
