#!/usr/bin/env python3
"""Audit DeepSeek evidence while preserving a declared post-run security redaction."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

try:
    from scripts import deepseek_repository_completion_probe as core
except ModuleNotFoundError:
    import deepseek_repository_completion_probe as core


RUNNER = "crates/evaluation-runtime/src/bin/repository_completion_remote.rs"
ALLOWED_ERRORS = {
    "runner source hash mismatch",
    f"dependency hash mismatch: {RUNNER}",
}


def audit(output: Path, repo_root: Path) -> dict[str, Any]:
    output = output.resolve()
    repo_root = repo_root.resolve()
    base = core.audit(output, repo_root)
    note = core.read_object(output / "post-run-security-redaction.json", "redaction note")
    freeze = core.read_object(output / "freeze.json", "freeze")
    manifest = core.read_object(output / "manifest.json", "manifest")
    runner = repo_root / RUNNER
    errors: list[str] = []
    if note.get("kind") != "post-run-security-redaction" or note.get("path") != RUNNER:
        errors.append("invalid redaction-note identity")
    if note.get("scope") != "test fixture literal only; production runner behavior unchanged":
        errors.append("redaction scope is not fixed")
    if note.get("frozen_sha256") != freeze.get("runner_source_sha256"):
        errors.append("redaction note does not reference the pre-call runner hash")
    if manifest.get("runner_source_sha256") != freeze.get("runner_source_sha256"):
        errors.append("manifest changed the frozen runner hash")
    if not runner.is_file() or note.get("redacted_sha256") != core.file_hash(runner):
        errors.append("redacted runner hash mismatch")
    if any(note.get(field) is not False for field in (
        "study_data_changed",
        "requests_changed",
        "events_changed",
        "results_changed",
    )):
        errors.append("redaction note permits a study-data change")
    base_errors = set(str(error) for error in base.get("errors", []))
    unexpected = sorted(base_errors - ALLOWED_ERRORS)
    missing = sorted(ALLOWED_ERRORS - base_errors)
    if unexpected:
        errors.extend(f"unexpected base audit error: {error}" for error in unexpected)
    if missing:
        errors.extend(f"expected declared mismatch is absent: {error}" for error in missing)
    return {
        "schema_version": 1,
        "audit_kind": "deepseek_integrity_with_declared_post_run_security_redaction",
        "valid": not errors,
        "errors": errors,
        "base_audit_errors": sorted(base_errors),
        "declared_allowed_errors": sorted(ALLOWED_ERRORS),
        "schedule_complete": base.get("schedule_complete"),
        "frozen_runner_sha256": note.get("frozen_sha256"),
        "redacted_runner_sha256": note.get("redacted_sha256"),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--repo-root", type=Path, required=True)
    args = parser.parse_args()
    report = audit(args.out, args.repo_root)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
