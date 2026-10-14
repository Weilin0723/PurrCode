#!/usr/bin/env python3
"""Compile and run an external Rust behavior oracle against a fixture copy."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Sequence


TIMEOUT_SECONDS = 90


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


def is_inside(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def run_command(argv: Sequence[str], *, cwd: Path, env: dict[str, str], timeout: int) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(argv),
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )


def result(*, passed: bool, stage: str, detail: str = "") -> int:
    payload = {"passed": passed, "stage": stage}
    if detail:
        payload["detail"] = detail[-4000:]
    print(json.dumps(payload, sort_keys=True))
    return 0 if passed else 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--harness", required=True, type=Path)
    args = parser.parse_args(argv)

    workspace = args.workspace.resolve()
    harness = args.harness.resolve()
    if not (workspace / "Cargo.toml").is_file():
        return result(passed=False, stage="workspace", detail="workspace has no Cargo.toml")
    if not harness.is_file():
        return result(passed=False, stage="harness", detail=f"missing harness: {harness}")
    if is_inside(harness, workspace):
        return result(passed=False, stage="isolation", detail="oracle harness must remain outside agent worktree")

    rustc = shutil.which("rustc")
    if rustc is None:
        return result(passed=False, stage="toolchain", detail="rustc is required")

    target_dir = workspace.parent / "oracle-target"
    binary = workspace.parent / "oracle-harness"
    env = clean_environment()
    target_dir.mkdir(parents=True, exist_ok=True)
    library = target_dir / "libpurrcode_alignment_fixture.rlib"
    try:
        library_run = run_command(
            [
                rustc,
                "--edition=2021",
                "--crate-name",
                "purrcode_alignment_fixture",
                "--crate-type",
                "lib",
                str(workspace / "src" / "lib.rs"),
                "-o",
                str(library),
            ],
            cwd=workspace,
            env=env,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        return result(passed=False, stage="library-timeout", detail=str(error))
    if library_run.returncode != 0:
        return result(passed=False, stage="library", detail=library_run.stdout + library_run.stderr)
    try:
        compile_run = run_command(
            [
                rustc,
                "--edition=2021",
                str(harness),
                "--extern",
                f"purrcode_alignment_fixture={library}",
                "-L",
                f"dependency={library.parent}",
                "-o",
                str(binary),
            ],
            cwd=workspace,
            env=env,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        return result(passed=False, stage="rustc-timeout", detail=str(error))
    if compile_run.returncode != 0:
        return result(passed=False, stage="rustc", detail=compile_run.stdout + compile_run.stderr)

    try:
        oracle_run = run_command([str(binary)], cwd=workspace, env=env, timeout=TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        return result(passed=False, stage="behavior-timeout", detail=str(error))
    if oracle_run.returncode != 0:
        return result(passed=False, stage="behavior", detail=oracle_run.stdout + oracle_run.stderr)
    return result(passed=True, stage="behavior")


if __name__ == "__main__":
    raise SystemExit(main())
