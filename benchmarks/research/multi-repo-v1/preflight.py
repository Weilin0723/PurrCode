#!/usr/bin/env python3
"""Validate the authored multi-repository suite without contacting a model.

The command creates temporary candidate worktrees, runs public Cargo tests and
isolated behavior oracles, and checks the preservation/test-integrity contract.
It prints a preflight summary only; it does not write model evidence or claim
any live outcome.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Mapping, Sequence


CANDIDATES = ("baseline", "gold", "stale", "overbroad", "test_tampering", "gold_stale_evidence")


class PreflightError(RuntimeError):
    pass


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise PreflightError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise PreflightError(f"{path}: expected an object")
    return value


def clean_environment() -> dict[str, str]:
    blocked = (
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
        if not any(fragment in key.upper() for fragment in blocked)
        and key not in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"}
    }


def safe_path(root: Path, relative: str) -> Path:
    path = (root / relative).resolve()
    if path != root.resolve() and root.resolve() not in path.parents:
        raise PreflightError(f"path escapes root: {relative}")
    return path


def apply_operations(workspace: Path, operations: Sequence[Mapping[str, Any]]) -> None:
    for operation in operations:
        path = safe_path(workspace, str(operation["path"]))
        kind = operation["op"]
        if kind == "replace_once":
            text = path.read_text(encoding="utf-8")
            old = str(operation["old"])
            if text.count(old) != 1:
                raise PreflightError(f"{path}: expected exactly one replacement match")
            path.write_text(text.replace(old, str(operation["new"]), 1), encoding="utf-8")
        elif kind == "remove_line":
            lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
            target = str(operation["line"])
            matches = [index for index, line in enumerate(lines) if line.rstrip("\n") == target]
            if len(matches) != 1:
                raise PreflightError(f"{path}: expected exactly one removable line")
            del lines[matches[0]]
            path.write_text("".join(lines), encoding="utf-8")
        else:
            raise PreflightError(f"unsupported operation: {kind}")


def run(argv: Sequence[str], *, cwd: Path, timeout: int = 90) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            list(argv),
            cwd=cwd,
            env=clean_environment(),
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise PreflightError(f"command timed out: {' '.join(argv)}") from error


def public_tests_pass(workspace: Path) -> bool:
    cargo = shutil.which("cargo")
    if cargo is None:
        raise PreflightError("cargo is required for suite preflight")
    result = run([cargo, "test", "--manifest-path", str(workspace / "Cargo.toml"), "--quiet"], cwd=workspace)
    return result.returncode == 0


def test_integrity_passes(base: Path, candidate: Path, paths: Sequence[str]) -> bool:
    for relative in paths:
        base_text = safe_path(base, relative).read_text(encoding="utf-8")
        candidate_text = safe_path(candidate, relative).read_text(encoding="utf-8")
        marker = "#[cfg(test)]"
        if marker not in base_text or marker not in candidate_text:
            return False
        base_lines = base_text[base_text.index(marker) :].splitlines()
        candidate_lines = candidate_text[candidate_text.index(marker) :].splitlines()
        position = 0
        for line in base_lines:
            try:
                position = candidate_lines.index(line, position) + 1
            except ValueError:
                return False
    return True


def unchanged_passes(base: Path, candidate: Path, paths: Sequence[str]) -> bool:
    return all(safe_path(base, relative).read_bytes() == safe_path(candidate, relative).read_bytes() for relative in paths)


def tree_digest(root: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if not path.is_file() or "target" in path.parts or ".git" in path.parts:
            continue
        relative = path.relative_to(root).as_posix()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def gate_for(candidate: str) -> str:
    if candidate in {"baseline", "stale", "gold_stale_evidence"}:
        return "BLOCKED"
    return "READY"


def oracle_passes(suite_root: Path, workspace: Path, task: Mapping[str, Any], crate_name: str) -> bool:
    runner = suite_root / "oracles" / "run_rust_oracle.py"
    harness = suite_root / str(task["oracle"])
    result = run(
        [
            sys.executable,
            str(runner),
            "--workspace",
            str(workspace),
            "--harness",
            str(harness),
            "--crate-name",
            crate_name,
        ],
        cwd=suite_root,
    )
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise PreflightError(f"oracle returned invalid JSON: {result.stdout[-2000:]}") from error
    if not isinstance(payload, dict) or not isinstance(payload.get("passed"), bool):
        raise PreflightError(f"oracle returned no explicit boolean: {result.stdout[-2000:]}")
    if result.returncode not in {0, 1}:
        raise PreflightError(f"oracle infrastructure failed: {result.stdout[-2000:]}{result.stderr[-2000:]}")
    return bool(payload["passed"])


def candidate_operations(task: Mapping[str, Any], candidate: str) -> tuple[Sequence[Mapping[str, Any]], bool]:
    source = "gold" if candidate == "gold_stale_evidence" else candidate
    mutation = task["mutations"][source]
    return mutation["operations"], bool(mutation["expected_external_accepted"])


def validate(suite_path: Path) -> dict[str, Any]:
    suite_path = suite_path.resolve()
    suite_root = suite_path.parent
    suite = read_json(suite_path)
    repositories = {str(item["id"]): item for item in suite.get("repositories", [])}
    tasks = suite.get("tasks")
    if suite.get("status") != "design_only":
        raise PreflightError("suite must remain design_only until a separate live freeze exists")
    if not isinstance(tasks, list) or len(tasks) != 6:
        raise PreflightError("suite must contain exactly six task clusters")
    if len(repositories) != 6:
        raise PreflightError("suite must contain six independent repository roots")

    counts = {"expected_accepted": 0, "actual_accepted": 0, "gate_ready": 0}
    rows = []
    for task in tasks:
        repository = repositories.get(str(task["repository"]))
        if repository is None:
            raise PreflightError(f"task references unknown repository: {task['repository']}")
        source = safe_path(suite_root, str(repository["path"]))
        if not source.is_dir():
            raise PreflightError(f"missing repository: {source}")
        for relative in repository["fixture_files"]:
            if not safe_path(source, str(relative)).is_file():
                raise PreflightError(f"missing fixture file: {source / str(relative)}")
        for candidate in CANDIDATES:
            operations, expected = candidate_operations(task, candidate)
            with tempfile.TemporaryDirectory(prefix=f"multi-repo-{task['id']}-{candidate}-") as temporary:
                workspace = Path(temporary) / "candidate"
                shutil.copytree(source, workspace)
                apply_operations(workspace, operations)
                public_pass = public_tests_pass(workspace)
                behavior_pass = oracle_passes(suite_root, workspace, task, str(repository["crate_name"]))
                unchanged = unchanged_passes(source, workspace, [str(item) for item in task["unchanged_paths"]])
                integrity = test_integrity_passes(source, workspace, [str(item) for item in task["test_integrity"]])
                actual = bool(public_pass and behavior_pass and unchanged and integrity)
                gate = gate_for(candidate)
                if expected:
                    counts["expected_accepted"] += 1
                if actual:
                    counts["actual_accepted"] += 1
                if gate == "READY":
                    counts["gate_ready"] += 1
                if actual != expected:
                    raise PreflightError(
                        f"{task['id']}/{candidate}: expected accepted={expected}, got {actual}; "
                        f"public={public_pass}, behavior={behavior_pass}, unchanged={unchanged}, integrity={integrity}"
                    )
                rows.append(
                    {
                        "task_id": task["id"],
                        "repository": repository["id"],
                        "candidate": candidate,
                        "external_accepted": actual,
                        "gate": gate,
                    }
                )
    if counts["expected_accepted"] != 12 or counts["actual_accepted"] != 12:
        raise PreflightError(f"expected exactly 12 accepted candidate snapshots: {counts}")
    return {
        "suite": suite_path.relative_to(suite_root.parent).as_posix(),
        "status": "preflight_pass",
        "design_only": True,
        "repositories": len(repositories),
        "tasks": len(tasks),
        "candidate_snapshots": len(rows),
        "counts": counts,
        "model_calls": 0,
        "outcomes": "not collected",
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", type=Path, default=Path(__file__).with_name("suite.json"))
    args = parser.parse_args(argv)
    try:
        report = validate(args.suite)
    except (OSError, PreflightError, KeyError, TypeError, ValueError) as error:
        print(json.dumps({"status": "preflight_failed", "error": str(error)}, sort_keys=True))
        return 1
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
