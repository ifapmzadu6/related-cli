#!/usr/bin/env python3
"""Run paired Codex editing trials with and without related-cli.

Each case checks out the parent of a historical commit into two disposable
worktrees. Both arms receive the same task; the treatment arm must run
related-cli before inspecting other source files, while the control arm must
use ordinary source exploration. Results are scored against the historical
commit and optional hidden validation commands.
"""

from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from typing import Any


ARMS = ("without-related", "with-related")


@dataclass(frozen=True)
class TrialCase:
    case_id: str
    target_commit: str
    seed_file: str
    task: str
    expected_files: tuple[str, ...] | None
    shared_paths: tuple[str, ...]
    hidden_files: tuple[str, ...]
    fixture_files: tuple[tuple[Path, str], ...]
    validation_commands: tuple[tuple[str, ...], ...]


def run(
    args: list[str],
    *,
    cwd: Path,
    timeout: int | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
    )
    if check and result.returncode != 0:
        rendered = " ".join(shlex.quote(arg) for arg in args)
        raise RuntimeError(
            f"command failed ({result.returncode}): {rendered}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return run(["git", *args], cwd=repo, check=check)


def git_text(repo: Path, *args: str) -> str:
    return git(repo, *args).stdout.strip()


def checked_relative_path(value: str, *, field: str) -> str:
    path = Path(value)
    if path.is_absolute() or ".." in path.parts:
        raise RuntimeError(f"{field} must stay inside the worktree: {value}")
    return value


def checked_case_id(value: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", value):
        raise RuntimeError(f"invalid case id: {value}")
    return value


def load_cases(path: Path) -> tuple[str, list[TrialCase]]:
    raw = json.loads(path.read_text(encoding="utf-8"))
    cases = []
    case_ids: set[str] = set()
    for item in raw["cases"]:
        expected = item.get("expected_files")
        case_id = checked_case_id(item["id"])
        if case_id in case_ids:
            raise RuntimeError(f"duplicate case id: {case_id}")
        case_ids.add(case_id)
        cases.append(
            TrialCase(
                case_id=case_id,
                target_commit=item["target_commit"],
                seed_file=checked_relative_path(item["seed_file"], field="seed_file"),
                task=item["task"],
                expected_files=(
                    tuple(
                        checked_relative_path(value, field="expected_files")
                        for value in expected
                    )
                    if expected is not None
                    else None
                ),
                shared_paths=tuple(
                    checked_relative_path(value, field="shared_paths")
                    for value in item.get("shared_paths", [])
                ),
                hidden_files=tuple(
                    checked_relative_path(value, field="hidden_files")
                    for value in item.get("hidden_files", [])
                ),
                fixture_files=tuple(
                    (
                        (path.parent / fixture["source"]).resolve(),
                        checked_relative_path(
                            fixture["destination"], field="fixture destination"
                        ),
                    )
                    for fixture in item.get("fixture_files", [])
                ),
                validation_commands=tuple(
                    tuple(command) for command in item.get("validation_commands", [])
                ),
            )
        )
    return raw.get("repository", ""), cases


def ensure_repository(repo: Path) -> None:
    if not repo.is_dir():
        raise RuntimeError(f"repository does not exist: {repo}")
    git(repo, "rev-parse", "--show-toplevel")


def resolve_case(repo: Path, case: TrialCase) -> tuple[str, str, list[str]]:
    target = git_text(repo, "rev-parse", f"{case.target_commit}^{{commit}}")
    parents = git_text(repo, "show", "-s", "--format=%P", target).split()
    if len(parents) != 1:
        raise RuntimeError(
            f"case {case.case_id} target must have exactly one parent, found {len(parents)}"
        )
    base = parents[0]
    if case.expected_files is None:
        expected = git_text(repo, "diff", "--name-only", base, target).splitlines()
    else:
        expected = list(case.expected_files)
    if case.seed_file not in git_text(repo, "ls-tree", "-r", "--name-only", base).splitlines():
        raise RuntimeError(
            f"case {case.case_id} seed is not tracked at base commit: {case.seed_file}"
        )
    return base, target, expected


def prompt_for(case: TrialCase, arm: str, package: str) -> str:
    common = f"""You are running a controlled editing benchmark in a disposable Git worktree.
Implement the task below completely, but do not commit or push. Do not inspect commits, tags,
or branches that are not reachable from the checked-out HEAD. Keep the patch focused and run
relevant local checks when practical.

Seed file: {case.seed_file}

Task:
{case.task}
"""
    if arm == "with-related":
        treatment = f"""
Use the installed find-related-files skill. Before reading or searching other source files, run
this initial repository-inspection command exactly once:

env npm_config_loglevel=error npx -y --package {shlex.quote(package)} related query {shlex.quote(case.seed_file)} --top 10

Do not repeat an identical query. Use its results only as context hints, then inspect the relevant
files and implement the task. Before choosing edit targets, verify every explicitly named
component, screen, platform, layer, and test with direct path or source-text search. Default to
the single seed query; run at most one additional related query only if a genuinely independent
surface remains unresolved after direct search. The task text overrides the ranking: do not drop
a named target, or substitute a similarly named result, because of the related-file output.
"""
    else:
        treatment = """
Do not run related-cli or any Git-history/co-change based related-file lookup. Explore the source
normally using filenames, search, and direct file inspection, then implement the task.
"""
    return common + treatment


def create_worktree(repo: Path, path: Path, base: str, shared_paths: tuple[str, ...]) -> None:
    git(repo, "worktree", "add", "--detach", str(path), base)
    for relative in shared_paths:
        source = repo / relative
        destination = path / relative
        if not source.exists():
            raise RuntimeError(f"shared path does not exist: {source}")
        if destination.exists() or destination.is_symlink():
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.symlink_to(source, target_is_directory=source.is_dir())


def install_benchmark_skill(
    worktree: Path, skill: Path | None, package: str
) -> Path | None:
    if skill is None:
        return None
    if not (skill / "SKILL.md").is_file():
        raise RuntimeError(f"benchmark skill is missing SKILL.md: {skill}")
    destination = worktree / ".agents/skills/find-related-files"
    backup = worktree.parent / f"{worktree.name}-original-find-related-files"
    if backup.exists():
        shutil.rmtree(backup)
    if destination.exists():
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(destination), str(backup))
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(skill, destination)
    skill_file = destination / "SKILL.md"
    content = skill_file.read_text(encoding="utf-8")
    skill_file.write_text(
        content.replace("related-cli@latest", package), encoding="utf-8"
    )
    return backup


def skill_fingerprint(skill: Path | None) -> str | None:
    if skill is None:
        return None
    skill_file = skill / "SKILL.md"
    if not skill_file.is_file():
        raise RuntimeError(f"benchmark skill is missing SKILL.md: {skill}")
    digest = hashlib.sha256()
    for path in sorted(item for item in skill.rglob("*") if item.is_file()):
        digest.update(path.relative_to(skill).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def restore_benchmark_skill(worktree: Path, backup: Path | None) -> None:
    destination = worktree / ".agents/skills/find-related-files"
    if destination.exists():
        shutil.rmtree(destination)
    if backup is not None and backup.exists():
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(backup), str(destination))


def changed_files(worktree: Path, excluded_paths: tuple[str, ...]) -> list[str]:
    tracked = git_text(worktree, "diff", "--name-only").splitlines()
    untracked_raw = git(worktree, "ls-files", "--others", "--exclude-standard", "-z").stdout
    untracked = [item for item in untracked_raw.split("\0") if item]
    excluded = set(excluded_paths)
    return sorted(path for path in set(tracked + untracked) if path not in excluded)


def diff_with_untracked(worktree: Path, files: list[str]) -> str:
    patch = git(worktree, "diff", "--no-ext-diff", "--unified=0").stdout
    tracked = set(git_text(worktree, "ls-files").splitlines())
    for relative in files:
        if relative in tracked:
            continue
        result = run(
            ["git", "diff", "--no-index", "--unified=0", "--", "/dev/null", relative],
            cwd=worktree,
            check=False,
        )
        if result.returncode not in (0, 1):
            raise RuntimeError(result.stderr)
        patch += result.stdout
    return patch


def changed_line_units(patch: str) -> Counter[tuple[str, str, str]]:
    units: Counter[tuple[str, str, str]] = Counter()
    current = ""
    for line in patch.splitlines():
        if line.startswith("diff --git a/"):
            parts = line.split(" b/", 1)
            current = parts[1] if len(parts) == 2 else ""
        elif current and line.startswith("+") and not line.startswith("+++"):
            units[(current, "+", line[1:])] += 1
        elif current and line.startswith("-") and not line.startswith("---"):
            units[(current, "-", line[1:])] += 1
    return units


def overlap_metrics(
    candidate: Counter[tuple[str, str, str]],
    expected: Counter[tuple[str, str, str]],
) -> dict[str, float | int]:
    overlap = sum((candidate & expected).values())
    candidate_count = sum(candidate.values())
    expected_count = sum(expected.values())
    return {
        "matching_changed_lines": overlap,
        "candidate_changed_lines": candidate_count,
        "expected_changed_lines": expected_count,
        "changed_line_precision": overlap / candidate_count if candidate_count else 0.0,
        "changed_line_recall": overlap / expected_count if expected_count else 0.0,
    }


def parse_codex_jsonl(stdout: str) -> tuple[dict[str, int], str]:
    usage = {
        "input_tokens": 0,
        "cached_input_tokens": 0,
        "cache_write_input_tokens": 0,
        "output_tokens": 0,
        "reasoning_output_tokens": 0,
    }
    final_message = ""
    for line in stdout.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "turn.completed":
            for key in usage:
                usage[key] += int(event.get("usage", {}).get(key, 0))
        item = event.get("item", {})
        if event.get("type") == "item.completed" and item.get("type") == "agent_message":
            final_message = item.get("text", "")
    return usage, final_message


def related_query_metrics(stdout: str) -> dict[str, int]:
    commands = []
    for line in stdout.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        item = event.get("item", {})
        if (
            event.get("type") == "item.started"
            and item.get("type") == "command_execution"
            and re.search(r"\brelated query\b", item.get("command", ""))
        ):
            commands.append(item["command"])
    return {
        "related_query_commands": len(commands),
        "duplicate_related_query_commands": len(commands) - len(set(commands)),
    }


def copy_hidden_files(
    repo: Path,
    worktree: Path,
    target: str,
    files: tuple[str, ...],
    fixture_files: tuple[tuple[Path, str], ...],
) -> None:
    for relative in files:
        content = git(repo, "show", f"{target}:{relative}").stdout
        destination = worktree / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding="utf-8")
    for source, relative in fixture_files:
        if not source.is_file():
            raise RuntimeError(f"fixture file does not exist: {source}")
        destination = worktree / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(source.read_bytes())


def validate_trial(
    repo: Path,
    worktree: Path,
    target: str,
    case: TrialCase,
    timeout: int,
) -> list[dict[str, Any]]:
    checks = []
    diff_check = git(worktree, "diff", "--check", check=False)
    checks.append(
        {
            "command": ["git", "diff", "--check"],
            "exit_code": diff_check.returncode,
            "stdout": diff_check.stdout,
            "stderr": diff_check.stderr,
        }
    )
    copy_hidden_files(repo, worktree, target, case.hidden_files, case.fixture_files)
    for command in case.validation_commands:
        started = time.monotonic()
        try:
            result = run(list(command), cwd=worktree, timeout=timeout, check=False)
            checks.append(
                {
                    "command": list(command),
                    "exit_code": result.returncode,
                    "duration_seconds": round(time.monotonic() - started, 3),
                    "stdout": result.stdout[-8000:],
                    "stderr": result.stderr[-8000:],
                }
            )
        except subprocess.TimeoutExpired as error:
            checks.append(
                {
                    "command": list(command),
                    "exit_code": None,
                    "duration_seconds": round(time.monotonic() - started, 3),
                    "error": f"timed out after {error.timeout} seconds",
                }
            )
    return checks


def score_files(candidate: list[str], expected: list[str]) -> dict[str, float | int]:
    candidate_set = set(candidate)
    expected_set = set(expected)
    overlap = candidate_set & expected_set
    return {
        "matching_files": len(overlap),
        "candidate_files": len(candidate_set),
        "expected_files": len(expected_set),
        "file_precision": len(overlap) / len(candidate_set) if candidate_set else 0.0,
        "file_recall": len(overlap) / len(expected_set) if expected_set else 0.0,
    }


def trial(
    *,
    repo: Path,
    worktree: Path,
    case: TrialCase,
    base: str,
    target: str,
    expected_files: list[str],
    arm: str,
    package: str,
    model: str | None,
    codex_timeout: int,
    validation_timeout: int,
    artifacts: Path,
    treatment_skill: Path | None,
) -> dict[str, Any]:
    create_worktree(repo, worktree, base, case.shared_paths)
    skill_backup = install_benchmark_skill(worktree, treatment_skill, package)
    prompt = prompt_for(case, arm, package)
    command = [
        "codex",
        "exec",
        "--ephemeral",
        "--ignore-user-config",
        "--approve-for-me",
        "--json",
        "-C",
        str(worktree),
    ]
    if model:
        command.extend(["--model", model])
    command.append(prompt)

    started = time.monotonic()
    try:
        codex = run(command, cwd=worktree, timeout=codex_timeout, check=False)
        timed_out = False
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout.decode() if isinstance(error.stdout, bytes) else (error.stdout or "")
        stderr = error.stderr.decode() if isinstance(error.stderr, bytes) else (error.stderr or "")
        codex = subprocess.CompletedProcess(command, 124, stdout, stderr)
        timed_out = True
    duration = round(time.monotonic() - started, 3)

    artifact_prefix = artifacts / f"{case.case_id}-{arm}"
    artifact_prefix.with_suffix(".prompt.txt").write_text(prompt, encoding="utf-8")
    artifact_prefix.with_suffix(".jsonl").write_text(codex.stdout, encoding="utf-8")
    artifact_prefix.with_suffix(".stderr.log").write_text(codex.stderr, encoding="utf-8")

    restore_benchmark_skill(worktree, skill_backup)
    candidate_files = changed_files(worktree, case.shared_paths)
    candidate_patch = diff_with_untracked(worktree, candidate_files)
    expected_patch = git(
        repo,
        "diff",
        "--no-ext-diff",
        "--unified=0",
        base,
        target,
        "--",
        *expected_files,
    ).stdout
    artifact_prefix.with_suffix(".patch").write_text(candidate_patch, encoding="utf-8")

    usage, final_message = parse_codex_jsonl(codex.stdout)
    checks = validate_trial(repo, worktree, target, case, validation_timeout)
    result: dict[str, Any] = {
        "case": case.case_id,
        "arm": arm,
        "base_commit": base,
        "target_commit": target,
        "seed_file": case.seed_file,
        "duration_seconds": duration,
        "codex_exit_code": codex.returncode,
        "codex_timed_out": timed_out,
        "usage": usage,
        **related_query_metrics(codex.stdout),
        "non_cached_input_tokens": max(
            0, usage["input_tokens"] - usage["cached_input_tokens"]
        ),
        "candidate_file_paths": candidate_files,
        "expected_file_paths": expected_files,
        **score_files(candidate_files, expected_files),
        **overlap_metrics(changed_line_units(candidate_patch), changed_line_units(expected_patch)),
        "checks": checks,
        "all_checks_passed": codex.returncode == 0
        and all(check.get("exit_code") == 0 for check in checks),
        "final_message": final_message,
    }
    result["target_file_success"] = (
        result["all_checks_passed"]
        and result["expected_files"] > 0
        and result["matching_files"] == result["expected_files"]
    )
    result["exact_patch_success"] = (
        result["target_file_success"]
        and result["changed_line_precision"] == 1.0
        and result["changed_line_recall"] == 1.0
    )
    return result


def aggregate_arm(results: list[dict[str, Any]], arm: str) -> dict[str, float | int]:
    selected = [result for result in results if result["arm"] == arm]
    if not selected:
        return {
            "trials": 0,
            "target_file_successes": 0,
            "exact_patch_successes": 0,
            "mean_file_precision": 0.0,
            "mean_file_recall": 0.0,
            "mean_changed_line_precision": 0.0,
            "mean_changed_line_recall": 0.0,
            "non_cached_input_tokens": 0,
            "output_tokens": 0,
            "duration_seconds": 0.0,
            "median_duration_seconds": 0.0,
            "related_query_commands": 0,
            "duplicate_related_query_commands": 0,
        }
    return {
        "trials": len(selected),
        "target_file_successes": sum(bool(item["target_file_success"]) for item in selected),
        "exact_patch_successes": sum(bool(item["exact_patch_success"]) for item in selected),
        "mean_file_precision": statistics.mean(item["file_precision"] for item in selected),
        "mean_file_recall": statistics.mean(item["file_recall"] for item in selected),
        "mean_changed_line_precision": statistics.mean(
            item["changed_line_precision"] for item in selected
        ),
        "mean_changed_line_recall": statistics.mean(
            item["changed_line_recall"] for item in selected
        ),
        "non_cached_input_tokens": sum(item["non_cached_input_tokens"] for item in selected),
        "output_tokens": sum(item["usage"]["output_tokens"] for item in selected),
        "duration_seconds": sum(item["duration_seconds"] for item in selected),
        "median_duration_seconds": statistics.median(
            item["duration_seconds"] for item in selected
        ),
        "related_query_commands": sum(
            item.get("related_query_commands", 0) for item in selected
        ),
        "duplicate_related_query_commands": sum(
            item.get("duplicate_related_query_commands", 0) for item in selected
        ),
    }


def paired_outcomes(results: list[dict[str, Any]]) -> dict[str, int]:
    by_case: dict[str, dict[str, dict[str, Any]]] = {}
    for result in results:
        by_case.setdefault(result["case"], {})[result["arm"]] = result
    wins = losses = ties = incomplete = 0
    for arms in by_case.values():
        if any(arm not in arms for arm in ARMS):
            incomplete += 1
            continue
        control = bool(arms["without-related"]["target_file_success"])
        treatment = bool(arms["with-related"]["target_file_success"])
        if treatment and not control:
            wins += 1
        elif control and not treatment:
            losses += 1
        else:
            ties += 1
    return {
        "treatment_wins": wins,
        "treatment_losses": losses,
        "ties": ties,
        "incomplete": incomplete,
    }


def markdown_report(metadata: dict[str, Any], results: list[dict[str, Any]]) -> str:
    lines = [
        "# Agent A/B evaluation",
        "",
        f"- Repository: `{metadata['repository']}`",
        f"- related package: `{metadata['related_package']}`",
        f"- Codex: `{metadata['codex_version']}`",
        f"- Generated: `{metadata['generated_at']}`",
        "",
        "| Case | Arm | Target files | Exact patch | Files P/R | Changed lines P/R | Checks | Input tokens (non-cached) | Output tokens | Time |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for result in results:
        checks = "pass" if result["all_checks_passed"] else "fail"
        lines.append(
            "| {case} | {arm} | {target} | {exact} | {fp:.3f}/{fr:.3f} | {lp:.3f}/{lr:.3f} | {checks} | "
            "{input_tokens} ({non_cached}) | {output_tokens} | {duration:.1f}s |".format(
                case=result["case"],
                arm=result["arm"],
                target="pass" if result["target_file_success"] else "fail",
                exact="pass" if result["exact_patch_success"] else "fail",
                fp=result["file_precision"],
                fr=result["file_recall"],
                lp=result["changed_line_precision"],
                lr=result["changed_line_recall"],
                checks=checks,
                input_tokens=result["usage"]["input_tokens"],
                non_cached=result["non_cached_input_tokens"],
                output_tokens=result["usage"]["output_tokens"],
                duration=result["duration_seconds"],
            )
        )
    aggregates = {arm: aggregate_arm(results, arm) for arm in ARMS}
    lines.extend(
        [
            "",
            "## Aggregate",
            "",
            "| Arm | Target-file success | Exact patch | Mean files P/R | Mean changed lines P/R | Non-cached input | Output | Total time | Median time |",
            "|---|---:|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for arm in ARMS:
        item = aggregates[arm]
        lines.append(
            "| {arm} | {target}/{trials} | {exact}/{trials} | {fp:.3f}/{fr:.3f} | "
            "{lp:.3f}/{lr:.3f} | {input_tokens} | {output_tokens} | {duration:.1f}s | "
            "{median:.1f}s |".format(
                arm=arm,
                target=item["target_file_successes"],
                exact=item["exact_patch_successes"],
                trials=item["trials"],
                fp=item["mean_file_precision"],
                fr=item["mean_file_recall"],
                lp=item["mean_changed_line_precision"],
                lr=item["mean_changed_line_recall"],
                input_tokens=item["non_cached_input_tokens"],
                output_tokens=item["output_tokens"],
                duration=item["duration_seconds"],
                median=item["median_duration_seconds"],
            )
        )
    paired = paired_outcomes(results)
    lines.extend(
        [
            "",
            "Paired target-file outcomes for the treatment arm: "
            f"{paired['treatment_wins']} wins, {paired['treatment_losses']} losses, "
            f"{paired['ties']} ties, and {paired['incomplete']} incomplete cases.",
            "",
            "Related-query commands: "
            f"{aggregates['with-related']['related_query_commands']} total, including "
            f"{aggregates['with-related']['duplicate_related_query_commands']} exact duplicates.",
            "",
        ]
    )
    lines.extend(
        [
            "",
            "`Files P/R` compares changed file paths with the historical patch. `Changed lines P/R`",
            "compares added/deleted line multisets, so semantically equivalent implementations may score",
            "lower than exact reproductions. Checks include `git diff --check` and any case-specific hidden",
            "validation command.",
            "",
        ]
    )
    return "\n".join(lines)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path, help="local benchmark repository")
    parser.add_argument("--cases", required=True, type=Path, help="JSON case definition")
    parser.add_argument("--output", required=True, type=Path, help="artifact directory")
    parser.add_argument("--related-package", default="related-cli@0.4.1")
    parser.add_argument("--model", help="optional Codex model override")
    parser.add_argument("--max-cases", type=int, default=0, help="0 runs every case")
    parser.add_argument(
        "--case",
        action="append",
        dest="selected_cases",
        help="run only this case id; repeat to select multiple cases",
    )
    parser.add_argument(
        "--arm",
        action="append",
        choices=ARMS,
        dest="selected_arms",
        help="run only this arm; repeat to select both arms",
    )
    parser.add_argument("--codex-timeout", type=int, default=900)
    parser.add_argument("--validation-timeout", type=int, default=300)
    parser.add_argument(
        "--treatment-skill",
        type=Path,
        help="inject this skill into both arms while Codex runs, then restore the checkout",
    )
    parser.add_argument(
        "--resume",
        action="store_true",
        help="reuse completed case/arm results already present in the output directory",
    )
    parser.add_argument(
        "--validate-only",
        action="store_true",
        help="validate and resolve selected cases without starting Codex",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = args.repo.resolve()
    treatment_skill = args.treatment_skill.resolve() if args.treatment_skill else None
    treatment_skill_sha256 = skill_fingerprint(treatment_skill)
    ensure_repository(repo)
    repository_label, cases = load_cases(args.cases)
    if args.selected_cases:
        selected = set(args.selected_cases)
        known = {case.case_id for case in cases}
        unknown = selected - known
        if unknown:
            raise RuntimeError(f"unknown case ids: {', '.join(sorted(unknown))}")
        cases = [case for case in cases if case.case_id in selected]
    if args.max_cases > 0:
        cases = cases[: args.max_cases]
    if not cases:
        raise RuntimeError("no benchmark cases selected")

    if args.validate_only:
        for case in cases:
            base, target, expected_files = resolve_case(repo, case)
            print(
                f"{case.case_id}: base={base} target={target} "
                f"expected_files={len(expected_files)}"
            )
        return 0

    args.output.mkdir(parents=True, exist_ok=True)
    codex_version = run(["codex", "--version"], cwd=repo).stdout.strip()
    metadata = {
        "repository": repository_label or str(repo),
        "repository_head": git_text(repo, "rev-parse", "HEAD"),
        "cases_file_sha256": hashlib.sha256(args.cases.read_bytes()).hexdigest(),
        "selected_cases": [case.case_id for case in cases],
        "selected_arms": sorted(args.selected_arms or ARMS),
        "related_package": args.related_package,
        "treatment_skill": str(treatment_skill) if treatment_skill else None,
        "treatment_skill_sha256": treatment_skill_sha256,
        "codex_version": codex_version,
        "model_override": args.model,
        "generated_at": datetime.now(timezone.utc).isoformat(),
    }
    results: list[dict[str, Any]] = []
    results_path = args.output / "results.json"
    if args.resume and results_path.is_file():
        prior = json.loads(results_path.read_text(encoding="utf-8"))
        prior_metadata = prior.get("metadata", {})
        for key in (
            "repository",
            "repository_head",
            "cases_file_sha256",
            "selected_cases",
            "selected_arms",
            "related_package",
            "treatment_skill_sha256",
            "codex_version",
            "model_override",
        ):
            if prior_metadata.get(key) != metadata.get(key):
                raise RuntimeError(
                    f"cannot resume: metadata mismatch for {key}: "
                    f"{prior_metadata.get(key)!r} != {metadata.get(key)!r}"
                )
        metadata = prior_metadata
        results = list(prior.get("results", []))
    completed = {(result["case"], result["arm"]) for result in results}

    with tempfile.TemporaryDirectory(prefix="related-agent-ab-") as temporary:
        root = Path(temporary)
        for index, case in enumerate(cases):
            base, target, expected_files = resolve_case(repo, case)
            arms = list(ARMS)
            if index % 2 == 1:
                arms.reverse()
            if args.selected_arms:
                selected_arms = set(args.selected_arms)
                arms = [arm for arm in arms if arm in selected_arms]
            for arm in arms:
                if (case.case_id, arm) in completed:
                    print(f"skipping completed {case.case_id}/{arm}", flush=True)
                    continue
                worktree = root / f"{index:02d}-{case.case_id}-{arm}"
                print(f"running {case.case_id}/{arm}", flush=True)
                try:
                    result = trial(
                        repo=repo,
                        worktree=worktree,
                        case=case,
                        base=base,
                        target=target,
                        expected_files=expected_files,
                        arm=arm,
                        package=args.related_package,
                        model=args.model,
                        codex_timeout=args.codex_timeout,
                        validation_timeout=args.validation_timeout,
                        artifacts=args.output,
                        treatment_skill=treatment_skill,
                    )
                    results.append(result)
                    results_path.write_text(
                        json.dumps({"metadata": metadata, "results": results}, indent=2) + "\n",
                        encoding="utf-8",
                    )
                finally:
                    if worktree.exists():
                        git(repo, "worktree", "remove", "--force", str(worktree), check=False)

    payload = {"metadata": metadata, "results": results}
    results_path.write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )
    (args.output / "summary.md").write_text(
        markdown_report(metadata, results), encoding="utf-8"
    )
    print(args.output / "summary.md")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"agent_ab: {error}", file=sys.stderr)
        raise SystemExit(1)
