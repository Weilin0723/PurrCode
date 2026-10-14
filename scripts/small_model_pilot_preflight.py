#!/usr/bin/env python3
"""Preflight the frozen small-model B/C pilot against external behavior oracles."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Mapping, Sequence


REQUIRED_MUTATIONS = {"baseline", "gold", "stale", "overbroad", "test_tampering"}
REQUIRED_CANDIDATES = (
    "baseline",
    "gold",
    "stale",
    "overbroad",
    "test_tampering",
    "gold_stale_evidence",
)
REQUIRED_PROMPT_CONDITIONS = ("revision_rubric_json", "plain_json")
REQUIRED_MODELS = {"primary": "llama3.2:3b", "secondary": "llama3.2:1b"}
DEFAULT_DATASET = (
    Path(__file__).resolve().parents[1]
    / "benchmarks"
    / "research"
    / "small-model-pilot"
    / "dataset.json"
)


def clean_environment() -> dict[str, str]:
    blocked_fragments = (
        "API_KEY",
        "APIKEY",
        "AUTH_TOKEN",
        "CREDENTIAL",
        "PASSWORD",
        "PRIVATE_KEY",
        "SECRET",
        "TOKEN",
    )
    return {
        key: value
        for key, value in os.environ.items()
        if not any(fragment in key.upper() for fragment in blocked_fragments)
        and key not in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"}
    }


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def safe_relative(path_value: str) -> Path:
    path = Path(path_value)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"path must be relative and confined: {path_value}")
    return path


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected an object")
    return value


def validate_dataset(data: Mapping[str, Any], dataset_path: Path) -> None:
    if data.get("schema_version") != 2 or data.get("status") != "design_only":
        raise ValueError(f"{dataset_path}: expected schema_version=2 and status=design_only")
    if not isinstance(data.get("description"), str) or not data["description"]:
        raise ValueError(f"{dataset_path}: description must be non-empty")
    if data.get("arms") != ["B", "C"]:
        raise ValueError(f"{dataset_path}: this preflight only accepts B/C")
    if data.get("independent_unit") != "base_task_cluster":
        raise ValueError(f"{dataset_path}: independent unit must be base_task_cluster")
    if data.get("candidate_types") != list(REQUIRED_CANDIDATES):
        raise ValueError(f"{dataset_path}: candidate_types must be {list(REQUIRED_CANDIDATES)}")
    if data.get("prompt_conditions") != list(REQUIRED_PROMPT_CONDITIONS):
        raise ValueError(f"{dataset_path}: prompt_conditions must be {list(REQUIRED_PROMPT_CONDITIONS)}")
    if data.get("models") != REQUIRED_MODELS:
        raise ValueError(f"{dataset_path}: models must declare the frozen primary and secondary models")
    if data.get("model_roles") != {name: role for role, name in REQUIRED_MODELS.items()}:
        raise ValueError(f"{dataset_path}: model_roles do not match models")
    derivation = data.get("candidate_derivations", {}).get("gold_stale_evidence")
    if not isinstance(derivation, dict) or derivation.get("source_candidate") != "gold" or derivation.get("independent_snapshot") is not True:
        raise ValueError(f"{dataset_path}: gold_stale_evidence derivation is missing or ambiguous")
    decoding = data.get("decoding")
    if not isinstance(decoding, dict) or decoding.get("temperature") != 0.0 or decoding.get("top_p") != 1.0:
        raise ValueError(f"{dataset_path}: decoding must freeze temperature=0 and top_p=1")
    if decoding.get("stop") != ["\n\n"] or decoding.get("format") != "json":
        raise ValueError(f"{dataset_path}: decoding stop/format is not frozen")
    run_plan = data.get("run_plan")
    if not isinstance(run_plan, dict) or run_plan.get("scheduled_model_calls_per_model") != 144 or run_plan.get("scheduled_model_calls") != 288 or run_plan.get("unique_d_evaluations") != 36:
        raise ValueError(f"{dataset_path}: run_plan must declare 144 calls/model, 288 total, and 36 D evaluations")
    if run_plan.get("schedule_blocks") != [
        "llama3.2:3b/revision_rubric_json",
        "llama3.2:3b/plain_json",
        "llama3.2:1b/revision_rubric_json",
        "llama3.2:1b/plain_json",
    ]:
        raise ValueError(f"{dataset_path}: run_plan schedule blocks are not frozen")
    fixture_files = data.get("fixture_files")
    if not isinstance(fixture_files, list) or not fixture_files:
        raise ValueError(f"{dataset_path}: fixture_files must be non-empty")
    for path_value in fixture_files:
        if not isinstance(path_value, str):
            raise ValueError(f"{dataset_path}: fixture file path must be a string")
        safe_relative(path_value)

    tasks = data.get("tasks")
    if not isinstance(tasks, list) or len(tasks) != 6:
        raise ValueError(f"{dataset_path}: expected exactly six tasks")
    ids: set[str] = set()
    for index, task in enumerate(tasks):
        where = f"{dataset_path}.tasks[{index}]"
        if not isinstance(task, dict):
            raise ValueError(f"{where}: expected an object")
        task_id = task.get("id")
        if not isinstance(task_id, str) or not task_id or task_id in ids:
            raise ValueError(f"{where}.id: expected a unique non-empty string")
        ids.add(task_id)
        for field in ("initial_request", "mid_run_correction", "hard_requirements", "forbidden_or_preservation", "trigger", "oracle", "mutations"):
            if field not in task:
                raise ValueError(f"{where}: missing {field}")
        trigger = task["trigger"]
        if trigger.get("kind") != "first_successful_file_mutation" or trigger.get("events") != ["WriteFile", "DeleteFile"]:
            raise ValueError(f"{where}.trigger: expected the scaffold-neutral file mutation trigger")
        oracle = task["oracle"]
        for field in ("runner", "harness", "command"):
            if field not in oracle:
                raise ValueError(f"{where}.oracle: missing {field}")
            if field != "command":
                safe_relative(oracle[field])
        if oracle["command"] != ["python3", "{runner}", "--workspace", "{workspace}", "--harness", "{harness}"]:
            raise ValueError(f"{where}.oracle.command: unexpected command template")
        for path_value in [*task.get("test_integrity", []), *task.get("unchanged_paths", [])]:
            safe_relative(path_value)
        mutations = task["mutations"]
        if set(mutations) != REQUIRED_MUTATIONS:
            raise ValueError(f"{where}.mutations: expected {sorted(REQUIRED_MUTATIONS)}")
        for mutation_name, mutation in mutations.items():
            if not isinstance(mutation.get("operations"), list) or not isinstance(mutation.get("expect_accepted"), bool):
                raise ValueError(f"{where}.mutations.{mutation_name}: malformed mutation")
            for operation in mutation["operations"]:
                if not isinstance(operation, dict) or operation.get("op") not in {"replace_once", "replace_lines", "remove_line"}:
                    raise ValueError(f"{where}.mutations.{mutation_name}: unsupported operation")
                safe_relative(operation["path"])


def validate_fixture(data: Mapping[str, Any], repo_root: Path) -> Path:
    fixture = repo_root / safe_relative(str(data["source_fixture"]))
    if not fixture.is_dir():
        raise ValueError(f"fixture is missing: {fixture}")
    for path_value in data["fixture_files"]:
        path = fixture / safe_relative(path_value)
        if not path.is_file():
            raise ValueError(f"fixture file is missing: {path}")
    expected_hashes = data.get("fixture_sha256")
    if expected_hashes is not None:
        if not isinstance(expected_hashes, dict):
            raise ValueError("fixture_sha256 must be an object")
        for path_value in data["fixture_files"]:
            expected = expected_hashes.get(path_value)
            actual = sha256_file(fixture / safe_relative(path_value))
            if expected != actual:
                raise ValueError(f"fixture hash mismatch for {path_value}: expected {expected}, got {actual}")
    return fixture


def lines_from_text(text: str) -> list[str]:
    return text.splitlines()


def write_lines(path: Path, lines: Sequence[str], had_final_newline: bool) -> None:
    text = "\n".join(lines)
    if had_final_newline:
        text += "\n"
    path.write_text(text, encoding="utf-8")


def replace_lines(path: Path, old: Sequence[str], new: Sequence[str]) -> None:
    text = path.read_text(encoding="utf-8")
    lines = lines_from_text(text)
    old_lines = list(old)
    matches = [index for index in range(len(lines) - len(old_lines) + 1) if lines[index : index + len(old_lines)] == old_lines]
    if len(matches) != 1:
        raise ValueError(f"{path}: expected one line-block match, got {len(matches)}")
    index = matches[0]
    write_lines(path, [*lines[:index], *new, *lines[index + len(old_lines) :]], text.endswith("\n"))


def remove_line(path: Path, line: str) -> None:
    if "\n" in line:
        remove_lines(path, line.splitlines())
        return
    replace_lines(path, [line], [])


def remove_lines(path: Path, lines_to_remove: Sequence[str]) -> None:
    text = path.read_text(encoding="utf-8")
    lines = lines_from_text(text)
    target = list(lines_to_remove)
    matches = [index for index in range(len(lines) - len(target) + 1) if lines[index : index + len(target)] == target]
    if len(matches) != 1:
        raise ValueError(f"{path}: expected one removable line-block, got {len(matches)}")
    index = matches[0]
    write_lines(path, [*lines[:index], *lines[index + len(target) :]], text.endswith("\n"))


def apply_operations(workspace: Path, operations: Sequence[Mapping[str, Any]]) -> None:
    for operation in operations:
        path = workspace / safe_relative(str(operation["path"]))
        if operation["op"] == "replace_once":
            text = path.read_text(encoding="utf-8")
            old = str(operation["old"])
            if text.count(old) != 1:
                raise ValueError(f"{path}: expected one text match")
            path.write_text(text.replace(old, str(operation["new"]), 1), encoding="utf-8")
        elif operation["op"] == "replace_lines":
            replace_lines(path, operation["old"], operation["new"])
        elif operation["op"] == "remove_line":
            remove_line(path, str(operation["line"]))
        else:
            raise ValueError(f"unsupported operation: {operation['op']}")


def is_subsequence(needles: Sequence[str], haystack: Sequence[str]) -> bool:
    position = 0
    for needle in needles:
        try:
            position = haystack.index(needle, position) + 1
        except ValueError:
            return False
    return True


def test_integrity_passes(fixture: Path, workspace: Path, paths: Sequence[str]) -> bool:
    for path_value in paths:
        baseline = (fixture / safe_relative(path_value)).read_text(encoding="utf-8")
        candidate = (workspace / safe_relative(path_value)).read_text(encoding="utf-8")
        marker = "#[cfg(test)]"
        if marker not in baseline or marker not in candidate:
            return False
        baseline_tests = baseline[baseline.index(marker) :].splitlines()
        candidate_tests = candidate[candidate.index(marker) :].splitlines()
        if not is_subsequence(baseline_tests, candidate_tests):
            return False
    return True


def unchanged_passes(fixture: Path, workspace: Path, paths: Sequence[str]) -> bool:
    return all(
        (fixture / safe_relative(path_value)).read_bytes() == (workspace / safe_relative(path_value)).read_bytes()
        for path_value in paths
    )


def run_oracle(repo_root: Path, workspace: Path, oracle: Mapping[str, Any]) -> dict[str, Any]:
    """Return ACCEPT/REJECT only for an explicit valid oracle result.

    UNKNOWN is reserved for infrastructure failures (missing runner, timeout,
    malformed output, or a non-zero runner exit) and must never be coerced to
    external rejection.
    """

    def unknown(detail: str) -> dict[str, Any]:
        return {"label": "UNKNOWN", "explicit": False, "passed": None, "detail": detail}

    runner = repo_root / safe_relative(str(oracle["runner"]))
    harness = repo_root / safe_relative(str(oracle["harness"]))
    if not runner.is_file() or not harness.is_file():
        return unknown("oracle runner or harness is missing")
    try:
        harness.relative_to(workspace)
        return unknown("oracle harness is inside agent worktree")
    except ValueError:
        pass
    try:
        completed = subprocess.run(
            [sys.executable, str(runner), "--workspace", str(workspace), "--harness", str(harness)],
            cwd=repo_root,
            env=clean_environment(),
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        return unknown(f"oracle runner timed out: {error}")
    detail = (completed.stdout + completed.stderr).strip()
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError:
        return unknown(f"oracle output is invalid JSON: {detail[-4000:]}")
    if not isinstance(payload, dict) or not isinstance(payload.get("passed"), bool):
        return unknown(f"oracle output lacks an explicit boolean passed field: {detail[-4000:]}")
    # The frozen Rust runner deliberately exits 1 for an explicit behavioral
    # rejection.  Compile/toolchain/timeout stages are infrastructure
    # failures and remain UNKNOWN even when they print a false payload.
    if completed.returncode != 0 and payload.get("stage") != "behavior":
        return unknown(f"oracle runner failed with return code {completed.returncode}: {detail[-4000:]}")
    passed = bool(payload["passed"])
    return {
        "label": "ACCEPT" if passed else "REJECT",
        "explicit": True,
        "passed": passed,
        "detail": detail[-4000:],
    }


def run_preflight(dataset_path: Path) -> dict[str, Any]:
    dataset_path = dataset_path.resolve()
    repo_root = dataset_path.parents[3]
    data = read_json(dataset_path)
    validate_dataset(data, dataset_path)
    fixture = validate_fixture(data, repo_root)
    results: list[dict[str, Any]] = []
    for task in data["tasks"]:
        for mutation_name in REQUIRED_CANDIDATES:
            source_name = "gold" if mutation_name == "gold_stale_evidence" else mutation_name
            mutation = task["mutations"][source_name]
            with tempfile.TemporaryDirectory(prefix=f"small-model-pilot-{task['id']}-") as temporary:
                workspace = Path(temporary) / "agent-worktree"
                shutil.copytree(fixture, workspace)
                apply_operations(workspace, mutation["operations"])
                oracle_result = run_oracle(repo_root, workspace, task["oracle"])
                behavior_passed = oracle_result["passed"] is True
                oracle_unknown = oracle_result["label"] == "UNKNOWN"
                detail = str(oracle_result["detail"])
                preservation_passed = unchanged_passes(fixture, workspace, task["unchanged_paths"])
                tests_passed = test_integrity_passes(fixture, workspace, task["test_integrity"])
                accepted = None if oracle_unknown else behavior_passed and preservation_passed and tests_passed
                expected = bool(mutation["expect_accepted"])
                results.append(
                    {
                        "task_id": task["id"],
                        "mutation": mutation_name,
                        "behavior_oracle_label": oracle_result["label"],
                        "behavior_oracle_passed": oracle_result["passed"],
                        "unchanged_paths_passed": preservation_passed,
                        "test_integrity_passed": tests_passed,
                        "accepted": accepted,
                        "expected_accepted": expected,
                        "match": accepted is not None and accepted == expected,
                        "oracle_unknown": oracle_unknown,
                        "detail": detail,
                    }
                )
    return {
        "schema_version": 2,
        "status": "design_only",
        "benchmark_kind": data["benchmark_kind"],
        "tasks": len(data["tasks"]),
        "mutations_per_task": len(REQUIRED_CANDIDATES),
        "candidate_snapshots": len(results),
        "candidate_types": list(REQUIRED_CANDIDATES),
        "prompt_conditions": list(REQUIRED_PROMPT_CONDITIONS),
        "models": dict(REQUIRED_MODELS),
        "results": results,
        "all_expected": all(item["match"] for item in results),
        "oracle_unknown": sum(item["oracle_unknown"] for item in results),
        "accepted_gold": sum(item["accepted"] for item in results if item["mutation"] == "gold"),
        "scheduled_model_calls": 0,
        "claim": "Preflight only; no model run or efficacy result is represented.",
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", default=DEFAULT_DATASET, type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args(argv)
    report = run_preflight(args.dataset)
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0 if report["all_expected"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
