#!/usr/bin/env python3
"""Create contamination-safe authoring inputs for the frozen real-repository suite.

The exporter reads only the pinned public row revision and the already-acquired clean base
checkouts.  It allowlists the fields written to the P0 bundle, so solution-bearing row fields
cannot be copied into an authoring artifact.  P1 authoring is intentionally fail-closed: without
a separately reviewed task-specific authoring record, this tool writes provenance with no P1
text and exits non-zero.  It never calls a model and never sends credentials to a subprocess.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
from typing import Any, Mapping, Sequence

import acquisition_preflight as preflight


SCHEMA_VERSION = 1
P0_BUNDLE_KIND = "p0_authoring_input_bundle"
P1_PROVENANCE_KIND = "p1_authoring_provenance"
P0_RECORD_FIELDS = {
    "instance_id",
    "repo",
    "base_commit",
    "p0_sha256",
    "problem_statement",
    "base_checkout",
}
P0_CHECKOUT_FIELDS = {"path", "head", "clean"}
P1_RECORD_FIELDS = {
    "instance_id",
    "repo",
    "base_commit",
    "p0_sha256",
    "p1_sha256",
    "status",
    "shared_across_variants",
}


class AuthoringInputError(RuntimeError):
    """An authoring input cannot be proven safe or complete."""


def _canonical_json(value: Mapping[str, Any]) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _assert_no_forbidden_keys(value: Any) -> None:
    """Reject forbidden field names recursively, without inspecting natural-language text."""
    if isinstance(value, Mapping):
        for key, nested in value.items():
            if str(key) in preflight.FORBIDDEN_MODEL_FIELDS:
                raise AuthoringInputError("authoring artifact contains a forbidden field")
            _assert_no_forbidden_keys(nested)
    elif isinstance(value, list):
        for nested in value:
            _assert_no_forbidden_keys(nested)


def _write_json_once(path: Path, value: Mapping[str, Any]) -> None:
    """Write one operator artifact without replacing an existing artifact."""
    path = path.resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    try:
        with path.open("x", encoding="utf-8") as destination:
            destination.write(encoded)
            destination.flush()
            os.fsync(destination.fileno())
    except FileExistsError as error:
        raise AuthoringInputError(f"refusing to replace existing artifact: {path.name}") from error


def _prepare_empty_output(path: Path) -> Path:
    path = path.expanduser().resolve()
    # Keeping authoring material outside this repository prevents accidental model visibility or
    # inclusion in a tracked benchmark artifact.
    if path == preflight.ROOT or preflight.ROOT in path.parents:
        raise AuthoringInputError("authoring output must be outside the repository")
    if path.exists() and not path.is_dir():
        raise AuthoringInputError("authoring output is not a directory")
    if path.exists() and any(path.iterdir()):
        raise AuthoringInputError("authoring output must be empty")
    path.mkdir(parents=True, exist_ok=True)
    return path


def _checkout_snapshot(task: Mapping[str, Any], cache: Path) -> dict[str, Any]:
    target = preflight.safe_child(cache, preflight.target_name(task))
    if not target.is_dir():
        raise AuthoringInputError(f"missing clean base checkout for {task['instance_id']}")
    try:
        head = preflight.run(["git", "-C", str(target), "rev-parse", "HEAD"]).stdout.strip()
        status = preflight.run(
            ["git", "-C", str(target), "status", "--porcelain", "--untracked-files=all"]
        ).stdout
    except preflight.AcquisitionError as error:
        raise AuthoringInputError(f"cannot inspect base checkout for {task['instance_id']}") from error
    if head != task["base_commit"] or status.strip():
        raise AuthoringInputError(f"base checkout is not clean and pinned for {task['instance_id']}")
    return {"path": str(target.resolve()), "head": head, "clean": True}


def _source_metadata(source: Mapping[str, Any]) -> dict[str, str]:
    return {
        "dataset": str(source["dataset"]),
        "config": str(source["config"]),
        "split": str(source["split"]),
        "revision": str(source["revision"]),
    }


def export_p0(manifest: Mapping[str, Any], cache: Path, output_dir: Path) -> dict[str, Any]:
    """Export only verified problem statements and clean base-checkout identities."""
    preflight.validate_manifest(manifest)
    cache = cache.expanduser().resolve()
    lock_name = "swebench-live-v1-acquired-345482f7"
    if cache.name != lock_name:
        raise AuthoringInputError("cache basename does not match the frozen acquisition")
    output_dir = _prepare_empty_output(output_dir)
    source = manifest["source"]
    rows = preflight.fetch_public_rows(source, 0, int(manifest["selection"]["source_rows_scanned"]))
    records: list[dict[str, Any]] = []
    for task in preflight.tasks(manifest):
        row = rows.get(int(task["dataset_row"]))
        if not isinstance(row, Mapping):
            raise AuthoringInputError(f"missing pinned source row for {task['instance_id']}")
        if any(row.get(field) != task[field] for field in ("instance_id", "repo", "base_commit")):
            raise AuthoringInputError(f"source identity mismatch for {task['instance_id']}")
        problem_statement = row.get("problem_statement")
        if not isinstance(problem_statement, str) or not problem_statement.strip():
            raise AuthoringInputError(f"empty problem statement for {task['instance_id']}")
        p0_sha256 = _sha256_text(problem_statement)
        if p0_sha256 != task["problem_statement_sha256"]:
            raise AuthoringInputError(f"problem statement digest mismatch for {task['instance_id']}")
        record = {
            "instance_id": str(task["instance_id"]),
            "repo": str(task["repo"]),
            "base_commit": str(task["base_commit"]),
            "p0_sha256": p0_sha256,
            "problem_statement": problem_statement,
            "base_checkout": _checkout_snapshot(task, cache),
        }
        if set(record) != P0_RECORD_FIELDS:
            raise AuthoringInputError("P0 record allowlist changed unexpectedly")
        _assert_no_forbidden_keys(record)
        records.append(record)
    bundle: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "kind": P0_BUNDLE_KIND,
        "source": _source_metadata(source),
        "task_count": len(records),
        "tasks": records,
    }
    _assert_no_forbidden_keys(bundle)
    bundle["bundle_sha256"] = hashlib.sha256(_canonical_json(bundle)).hexdigest()
    bundle_path = output_dir / "p0_authoring_inputs.json"
    _write_json_once(bundle_path, bundle)
    return {
        "status": "p0_exported",
        "bundle": str(bundle_path),
        "task_count": len(records),
        "bundle_sha256": bundle["bundle_sha256"],
        "model_calls": 0,
        "outcomes": "unrun",
    }


def _load_p0_bundle(path: Path, manifest: Mapping[str, Any], cache: Path) -> dict[str, Any]:
    try:
        bundle = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise AuthoringInputError("cannot read P0 bundle") from error
    if not isinstance(bundle, dict) or bundle.get("schema_version") != SCHEMA_VERSION or bundle.get("kind") != P0_BUNDLE_KIND:
        raise AuthoringInputError("P0 bundle schema or kind is invalid")
    _assert_no_forbidden_keys(bundle)
    recorded_digest = bundle.pop("bundle_sha256", None)
    if not isinstance(recorded_digest, str) or hashlib.sha256(_canonical_json(bundle)).hexdigest() != recorded_digest:
        raise AuthoringInputError("P0 bundle digest is invalid")
    # Restore the key for downstream provenance only after verifying the canonical digest.
    bundle["bundle_sha256"] = recorded_digest
    if bundle.get("source") != _source_metadata(manifest["source"]):
        raise AuthoringInputError("P0 bundle source revision is not the frozen revision")
    manifest_tasks = {str(item["instance_id"]): item for item in preflight.tasks(manifest)}
    records = bundle.get("tasks")
    if bundle.get("task_count") != len(manifest_tasks) or not isinstance(records, list) or len(records) != len(manifest_tasks):
        raise AuthoringInputError("P0 bundle task count is incomplete")
    seen: set[str] = set()
    for record in records:
        if not isinstance(record, Mapping) or set(record) != P0_RECORD_FIELDS:
            raise AuthoringInputError("P0 bundle contains a non-allowlisted task record")
        instance_id = str(record["instance_id"])
        task = manifest_tasks.get(instance_id)
        if task is None or instance_id in seen:
            raise AuthoringInputError("P0 bundle task identity is invalid")
        seen.add(instance_id)
        if record["repo"] != task["repo"] or record["base_commit"] != task["base_commit"]:
            raise AuthoringInputError(f"P0 bundle identity mismatch for {instance_id}")
        if record["p0_sha256"] != task["problem_statement_sha256"] or _sha256_text(str(record["problem_statement"])) != record["p0_sha256"]:
            raise AuthoringInputError(f"P0 bundle digest mismatch for {instance_id}")
        checkout = record["base_checkout"]
        if not isinstance(checkout, Mapping) or set(checkout) != P0_CHECKOUT_FIELDS or checkout.get("clean") is not True:
            raise AuthoringInputError(f"P0 bundle checkout identity is invalid for {instance_id}")
        expected = preflight.safe_child(cache, preflight.target_name(task))
        if checkout.get("path") != str(expected.resolve()) or checkout.get("head") != task["base_commit"]:
            raise AuthoringInputError(f"P0 bundle checkout does not match the frozen base for {instance_id}")
        _checkout_snapshot(task, cache)
    if seen != set(manifest_tasks):
        raise AuthoringInputError("P0 bundle does not cover every frozen task")
    return bundle


def author_p1_provenance(
    manifest: Mapping[str, Any], cache: Path, p0_bundle_path: Path, output_dir: Path
) -> dict[str, Any]:
    """Record the absence of a reviewed P1 and fail closed without emitting correction text."""
    preflight.validate_manifest(manifest)
    cache = cache.expanduser().resolve()
    if cache.name != "swebench-live-v1-acquired-345482f7":
        raise AuthoringInputError("cache basename does not match the frozen acquisition")
    bundle = _load_p0_bundle(p0_bundle_path.expanduser().resolve(), manifest, cache)
    output_dir = output_dir.expanduser().resolve()
    if output_dir == preflight.ROOT or preflight.ROOT in output_dir.parents:
        raise AuthoringInputError("authoring output must be outside the repository")
    output_dir.mkdir(parents=True, exist_ok=True)
    records = []
    for record in bundle["tasks"]:
        records.append(
            {
                "instance_id": record["instance_id"],
                "repo": record["repo"],
                "base_commit": record["base_commit"],
                "p0_sha256": record["p0_sha256"],
                "p1_sha256": None,
                "status": "not_generated",
                "shared_across_variants": True,
            }
        )
    provenance: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "kind": P1_PROVENANCE_KIND,
        "status": "blocked_fail_closed",
        "reason_code": "no_verified_task_specific_authoring_source",
        "input_kinds": ["p0_authoring_input", "clean_base_checkout"],
        "p1_text_materialized": False,
        "shared_p1_hash_required": True,
        "task_count": len(records),
        "tasks": records,
    }
    _assert_no_forbidden_keys(provenance)
    provenance["provenance_sha256"] = hashlib.sha256(_canonical_json(provenance)).hexdigest()
    provenance_path = output_dir / "p1_authoring_provenance.json"
    _write_json_once(provenance_path, provenance)
    return {
        "status": provenance["status"],
        "reason_code": provenance["reason_code"],
        "provenance": str(provenance_path),
        "task_count": len(records),
        "p1_materialized_count": 0,
        "model_calls": 0,
        "outcomes": "unrun",
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--export-p0", action="store_true", help="export verified P0 inputs")
    mode.add_argument("--author-p1", action="store_true", help="record fail-closed P1 provenance")
    parser.add_argument("--manifest", type=Path, default=preflight.DEFAULT_MANIFEST)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--p0-bundle", type=Path, help="P0 bundle for --author-p1")
    args = parser.parse_args(argv)
    try:
        manifest = preflight.load_json(args.manifest.expanduser().resolve())
        if args.export_p0:
            report = export_p0(manifest, args.cache_dir, args.output_dir)
        else:
            if args.p0_bundle is None:
                raise AuthoringInputError("--author-p1 requires --p0-bundle")
            report = author_p1_provenance(manifest, args.cache_dir, args.p0_bundle, args.output_dir)
    except (AuthoringInputError, preflight.AcquisitionError, OSError, KeyError, TypeError, ValueError) as error:
        print(json.dumps({"status": "authoring_preflight_failed", "error": str(error)}, sort_keys=True))
        return 1
    print(json.dumps(report, indent=2, sort_keys=True))
    return 2 if report.get("status") == "blocked_fail_closed" else 0


if __name__ == "__main__":
    raise SystemExit(main())
