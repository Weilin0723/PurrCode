#!/usr/bin/env python3
"""Build and verify the controlled-span requirement-revision asset.

The asset is derived from the verified P0 export and a reviewed, allowlisted span-selection file.
For each task, static receives the complete original statement; correction receives the exact
statement with one original constraint span withheld, then the identical span is restored after
the frozen execution trigger.  The reconstructed final text is byte-for-byte equal to the
original statement.  No model or provider is contacted.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
from typing import Any, Mapping, Sequence

import acquisition_preflight as preflight
from authoring_inputs import _load_p0_bundle


ROOT = Path(__file__).resolve().parent
DEFAULT_SELECTION = ROOT / "span_selection.json"
DEFAULT_ASSET = ROOT / "requirement_revision.json"
SCHEMA_VERSION = 2
ASSET_KIND = "controlled_span_requirement_revision"
REQUIRED_TASK_FIELDS = {
    "instance_id",
    "repo",
    "base_commit",
    "full_statement",
    "full_statement_sha256",
    "abridged_p0",
    "abridged_p0_sha256",
    "withheld_span",
    "withheld_span_sha256",
    "span_offsets",
    "selection_rationale",
    "p1_text",
    "p1_text_sha256",
    "static_final_requirement_sha256",
    "correction_final_requirement_sha256",
    "shared_final_requirement_hash",
}


class RequirementRevisionError(RuntimeError):
    """The controlled-span asset is incomplete or cannot be verified."""


def canonical_json(value: Mapping[str, Any]) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def assert_no_forbidden_keys(value: Any) -> None:
    if isinstance(value, Mapping):
        for key, nested in value.items():
            if str(key) in preflight.FORBIDDEN_MODEL_FIELDS:
                raise RequirementRevisionError("requirement asset contains a forbidden field")
            assert_no_forbidden_keys(nested)
    elif isinstance(value, list):
        for nested in value:
            assert_no_forbidden_keys(nested)


def load_selection(path: Path) -> dict[str, Any]:
    try:
        selection = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RequirementRevisionError("cannot read span-selection audit") from error
    if not isinstance(selection, dict) or selection.get("schema_version") != 1:
        raise RequirementRevisionError("span-selection audit schema is invalid")
    entries = selection.get("tasks")
    if not isinstance(selection.get("selection_rule"), str) or not selection["selection_rule"].strip():
        raise RequirementRevisionError("span-selection rule is missing")
    if not isinstance(entries, list) or len(entries) != 30:
        raise RequirementRevisionError("span-selection audit must contain thirty tasks")
    seen: set[str] = set()
    for entry in entries:
        if not isinstance(entry, Mapping):
            raise RequirementRevisionError("span-selection entry is malformed")
        instance_id = entry.get("instance_id")
        span = entry.get("span")
        rationale = entry.get("rationale")
        if (
            not isinstance(instance_id, str)
            or instance_id in seen
            or not isinstance(span, str)
            or not span
            or not isinstance(rationale, str)
            or not rationale.strip()
        ):
            raise RequirementRevisionError("span-selection entries must be unique and non-empty")
        seen.add(instance_id)
    return selection


def _task_selection(selection: Mapping[str, Any]) -> dict[str, Mapping[str, Any]]:
    return {str(entry["instance_id"]): entry for entry in selection["tasks"]}


def _build_task(task: Mapping[str, Any], p0: Mapping[str, Any], selected: Mapping[str, Any]) -> dict[str, Any]:
    instance_id = str(task["instance_id"])
    full_statement = p0.get("problem_statement")
    if not isinstance(full_statement, str) or not full_statement:
        raise RequirementRevisionError(f"missing full statement for {instance_id}")
    if p0.get("p0_sha256") != task["problem_statement_sha256"] or sha256_text(full_statement) != task["problem_statement_sha256"]:
        raise RequirementRevisionError(f"full statement digest mismatch for {instance_id}")
    span = selected["span"]
    occurrence_count = full_statement.count(span)
    start = full_statement.find(span)
    if occurrence_count != 1 or start < 0:
        raise RequirementRevisionError(f"span is not unique for {instance_id}")
    end = start + len(span)
    abridged = full_statement[:start] + full_statement[end:]
    final_requirement = abridged[:start] + span + abridged[start:]
    if final_requirement != full_statement:
        raise RequirementRevisionError(f"controlled-span reconstruction failed for {instance_id}")
    full_hash = sha256_text(full_statement)
    abridged_hash = sha256_text(abridged)
    span_hash = sha256_text(span)
    return {
        "instance_id": instance_id,
        "repo": str(task["repo"]),
        "base_commit": str(task["base_commit"]),
        "full_statement": full_statement,
        "full_statement_sha256": full_hash,
        "abridged_p0": abridged,
        "abridged_p0_sha256": abridged_hash,
        "withheld_span": span,
        "withheld_span_sha256": span_hash,
        "span_offsets": {
            "unit": "unicode_codepoints",
            "start": start,
            "end": end,
        },
        "selection_rationale": str(selected["rationale"]),
        "p1_text": span,
        "p1_text_sha256": span_hash,
        "static_final_requirement_sha256": full_hash,
        "correction_final_requirement_sha256": sha256_text(final_requirement),
        "shared_final_requirement_hash": full_hash == sha256_text(final_requirement),
    }


def build_asset(
    manifest: Mapping[str, Any], cache: Path, p0_bundle_path: Path, selection_path: Path
) -> dict[str, Any]:
    preflight.validate_manifest(manifest)
    selection = load_selection(selection_path.resolve())
    p0_bundle = _load_p0_bundle(p0_bundle_path.resolve(), manifest, cache.resolve())
    p0_by_id = {str(item["instance_id"]): item for item in p0_bundle["tasks"]}
    manifest_by_id = {str(item["instance_id"]): item for item in preflight.tasks(manifest)}
    selected_by_id = _task_selection(selection)
    if set(p0_by_id) != set(manifest_by_id) or set(selected_by_id) != set(manifest_by_id):
        raise RequirementRevisionError("P0 and span-selection coverage do not match the frozen tasks")
    records = [
        _build_task(manifest_by_id[instance_id], p0_by_id[instance_id], selected_by_id[instance_id])
        for instance_id in (str(item["instance_id"]) for item in preflight.tasks(manifest))
    ]
    asset: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "kind": ASSET_KIND,
        "source": {
            "dataset": str(manifest["source"]["dataset"]),
            "config": str(manifest["source"]["config"]),
            "split": str(manifest["source"]["split"]),
            "revision": str(manifest["source"]["revision"]),
        },
        "selection_rule": str(selection["selection_rule"]),
        "task_count": len(records),
        "delivery": {
            "static": "complete full_statement at call 0",
            "correction_initial": "abridged_p0 at call 0",
            "correction_revision": "identical p1_text after the first successful ExecutionFinished",
            "injection_trigger": "ExecutionFinished with exit code 0 and any non-empty affected_paths",
            "acknowledgement_event": "requirement_revision_ack",
            "final_requirement": "reconstructing abridged_p0 with p1_text at the recorded offset equals full_statement",
        },
        "tasks": records,
    }
    assert_no_forbidden_keys(asset)
    asset["asset_sha256"] = hashlib.sha256(canonical_json(asset)).hexdigest()
    return asset


def write_once(path: Path, asset: Mapping[str, Any]) -> None:
    path = path.resolve()
    if path != DEFAULT_ASSET.resolve():
        raise RequirementRevisionError("requirement asset output must be the owned frozen path")
    try:
        with path.open("x", encoding="utf-8") as destination:
            destination.write(json.dumps(asset, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
            destination.flush()
            os.fsync(destination.fileno())
    except FileExistsError as error:
        raise RequirementRevisionError("refusing to replace existing requirement asset") from error


def verify_asset(manifest: Mapping[str, Any], path: Path = DEFAULT_ASSET) -> dict[str, Any]:
    preflight.validate_manifest(manifest)
    try:
        asset = json.loads(path.resolve().read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RequirementRevisionError("cannot read requirement revision asset") from error
    if not isinstance(asset, dict) or asset.get("schema_version") != SCHEMA_VERSION or asset.get("kind") != ASSET_KIND:
        raise RequirementRevisionError("requirement revision schema or kind is invalid")
    assert_no_forbidden_keys(asset)
    recorded_digest = asset.pop("asset_sha256", None)
    if not isinstance(recorded_digest, str) or hashlib.sha256(canonical_json(asset)).hexdigest() != recorded_digest:
        raise RequirementRevisionError("requirement revision asset digest is invalid")
    asset["asset_sha256"] = recorded_digest
    source = manifest["source"]
    expected_source = {
        "dataset": str(source["dataset"]),
        "config": str(source["config"]),
        "split": str(source["split"]),
        "revision": str(source["revision"]),
    }
    if asset.get("source") != expected_source or asset.get("task_count") != 30:
        raise RequirementRevisionError("requirement revision source or count is invalid")
    records = asset.get("tasks")
    manifest_by_id = {str(item["instance_id"]): item for item in preflight.tasks(manifest)}
    if not isinstance(records, list) or len(records) != len(manifest_by_id):
        raise RequirementRevisionError("requirement revision task coverage is incomplete")
    seen: set[str] = set()
    for record in records:
        if not isinstance(record, Mapping) or set(record) != REQUIRED_TASK_FIELDS:
            raise RequirementRevisionError("requirement revision task fields are invalid")
        instance_id = str(record["instance_id"])
        task = manifest_by_id.get(instance_id)
        if task is None or instance_id in seen:
            raise RequirementRevisionError("requirement revision task identity is invalid")
        seen.add(instance_id)
        if record["repo"] != task["repo"] or record["base_commit"] != task["base_commit"]:
            raise RequirementRevisionError(f"requirement revision identity mismatch for {instance_id}")
        full = record["full_statement"]
        abridged = record["abridged_p0"]
        span = record["withheld_span"]
        if not all(isinstance(value, str) and value for value in (full, abridged, span, record["p1_text"])):
            raise RequirementRevisionError(f"empty requirement text for {instance_id}")
        offsets = record["span_offsets"]
        if (
            not isinstance(offsets, Mapping)
            or offsets.get("unit") != "unicode_codepoints"
            or not isinstance(offsets.get("start"), int)
            or not isinstance(offsets.get("end"), int)
            or offsets["start"] < 0
            or offsets["end"] <= offsets["start"]
            or full[offsets["start"] : offsets["end"]] != span
            or full.count(span) != 1
        ):
            raise RequirementRevisionError(f"requirement span offsets are invalid for {instance_id}")
        if abridged != full[: offsets["start"]] + full[offsets["end"] :]:
            raise RequirementRevisionError(f"abridged P0 does not match span removal for {instance_id}")
        if record["p1_text"] != span:
            raise RequirementRevisionError(f"P1 is not the original span for {instance_id}")
        full_hash = sha256_text(full)
        if full_hash != task["problem_statement_sha256"] or record["full_statement_sha256"] != full_hash:
            raise RequirementRevisionError(f"full statement hash mismatch for {instance_id}")
        if record["abridged_p0_sha256"] != sha256_text(abridged):
            raise RequirementRevisionError(f"abridged P0 hash mismatch for {instance_id}")
        span_hash = sha256_text(span)
        if record["withheld_span_sha256"] != span_hash or record["p1_text_sha256"] != span_hash:
            raise RequirementRevisionError(f"withheld span hash mismatch for {instance_id}")
        final = abridged[: offsets["start"]] + record["p1_text"] + abridged[offsets["start"] :]
        if final != full or record["static_final_requirement_sha256"] != full_hash or record["correction_final_requirement_sha256"] != full_hash:
            raise RequirementRevisionError(f"final requirement hash mismatch for {instance_id}")
        if record["shared_final_requirement_hash"] is not True:
            raise RequirementRevisionError(f"static/correction final hash is not shared for {instance_id}")
        if not isinstance(record["selection_rationale"], str) or not record["selection_rationale"].strip():
            raise RequirementRevisionError(f"selection rationale is missing for {instance_id}")
    if seen != set(manifest_by_id):
        raise RequirementRevisionError("requirement revision does not cover all tasks")
    return {
        "status": "verified",
        "asset": str(path.resolve()),
        "asset_sha256": recorded_digest,
        "task_count": len(seen),
        "p1_materialized_count": len(seen),
        "p1_hashes_verified": True,
        "model_calls": 0,
        "outcomes": "unrun",
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--build", action="store_true", help="build the frozen owned requirement asset")
    mode.add_argument("--verify", action="store_true", help="verify the frozen requirement asset")
    parser.add_argument("--manifest", type=Path, default=preflight.DEFAULT_MANIFEST)
    parser.add_argument("--cache-dir", type=Path, help="clean acquired cache used by the P0 bundle")
    parser.add_argument("--p0-bundle", type=Path, help="verified P0 bundle used for --build")
    parser.add_argument("--selection", type=Path, default=DEFAULT_SELECTION)
    parser.add_argument("--asset", type=Path, default=DEFAULT_ASSET)
    args = parser.parse_args(argv)
    try:
        manifest = preflight.load_json(args.manifest.resolve())
        if args.build:
            if args.cache_dir is None or args.p0_bundle is None:
                raise RequirementRevisionError("--build requires --cache-dir and --p0-bundle")
            asset = build_asset(manifest, args.cache_dir, args.p0_bundle, args.selection)
            write_once(args.asset, asset)
            report = {
                "status": "built",
                "asset": str(args.asset.resolve()),
                "asset_sha256": asset["asset_sha256"],
                "task_count": asset["task_count"],
                "model_calls": 0,
                "outcomes": "unrun",
            }
        else:
            report = verify_asset(manifest, args.asset)
    except (RequirementRevisionError, preflight.AcquisitionError, OSError, KeyError, TypeError, ValueError) as error:
        print(json.dumps({"status": "requirement_revision_failed", "error": str(error)}, sort_keys=True))
        return 1
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
