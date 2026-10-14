#!/usr/bin/env python3
"""Compile one isolated Rust behavior oracle and report an explicit result."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Sequence


TIMEOUT_SECONDS = 60


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


def emit(passed: bool, stage: str, detail: str = "") -> int:
    payload = {"passed": passed, "stage": stage}
    if detail:
        payload["detail"] = detail[-4000:]
    print(json.dumps(payload, sort_keys=True))
    return 0 if passed else 1


def run(argv: Sequence[str], *, cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(argv),
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=TIMEOUT_SECONDS,
        check=False,
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--harness", required=True, type=Path)
    parser.add_argument("--crate-name", required=True)
    args = parser.parse_args(argv)

    workspace = args.workspace.resolve()
    harness = args.harness.resolve()
    crate_name = args.crate_name
    if not (workspace / "Cargo.toml").is_file():
        return emit(False, "workspace", "workspace has no Cargo.toml")
    if not (workspace / "src" / "lib.rs").is_file():
        return emit(False, "workspace", "workspace has no src/lib.rs")
    if not harness.is_file():
        return emit(False, "harness", f"missing harness: {harness}")
    if harness.is_relative_to(workspace):
        return emit(False, "isolation", "oracle harness must remain outside the candidate workspace")
    rustc = shutil.which("rustc")
    if rustc is None:
        return emit(False, "toolchain", "rustc is required")

    scratch = workspace.parent / "oracle-build"
    scratch.mkdir(parents=True, exist_ok=True)
    library = scratch / f"lib{crate_name}.rlib"
    binary = scratch / f"oracle-{crate_name}"
    env = clean_environment()
    try:
        library_run = run(
            [
                rustc,
                "--edition=2021",
                "--crate-name",
                crate_name,
                "--crate-type",
                "lib",
                str(workspace / "src" / "lib.rs"),
                "-o",
                str(library),
            ],
            cwd=workspace,
            env=env,
        )
        if library_run.returncode != 0:
            return emit(False, "library", library_run.stdout + library_run.stderr)
        harness_run = run(
            [
                rustc,
                "--edition=2021",
                str(harness),
                "--extern",
                f"{crate_name}={library}",
                "-L",
                f"dependency={library.parent}",
                "-o",
                str(binary),
            ],
            cwd=workspace,
            env=env,
        )
        if harness_run.returncode != 0:
            return emit(False, "harness", harness_run.stdout + harness_run.stderr)
        oracle_run = run([str(binary)], cwd=workspace, env=env)
    except subprocess.TimeoutExpired as error:
        return emit(False, "timeout", str(error))
    if oracle_run.returncode != 0:
        return emit(False, "behavior", oracle_run.stdout + oracle_run.stderr)
    return emit(True, "behavior")


if __name__ == "__main__":
    raise SystemExit(main())
