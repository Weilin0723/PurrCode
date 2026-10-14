#!/usr/bin/env python3
"""Create a write-once plan, freeze, and publication receipt for the FSE study."""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fse_confirmatory_analysis as analysis  # noqa: E402


def load_object(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise analysis.AnalysisError(f"{path}: {error}") from error
    if not isinstance(value, dict):
        raise analysis.AnalysisError(f"{path}: expected a JSON object")
    return value


def _digest(value: Any, where: str) -> str:
    return analysis._digest(value, where)


def _hashes(
    tasks: Sequence[Mapping[str, Any]],
    protocol: Mapping[str, Any],
    key: str,
    fallback: str,
) -> dict[str, str]:
    task_ids = {str(task.get("instance_id", "")) for task in tasks}
    values = protocol.get(key)
    if values is None:
        values = protocol.get(key.replace("_sha256", "_hashes"))
    if values is None and isinstance(protocol.get("task_requirements"), Mapping):
        values = {
            task_id: metadata.get(key)
            for task_id, metadata in protocol["task_requirements"].items()
            if isinstance(metadata, Mapping)
        }
    if values is None and isinstance(protocol.get("tasks"), list):
        values = {
            str(task.get("instance_id", "")): (
                task.get(key)
                or (
                    task.get("abridged_p0_sha256")
                    if key == "p0_sha256"
                    else task.get("p1_text_sha256")
                    if key == "p1_sha256"
                    else None
                )
            )
            for task in protocol["tasks"]
            if isinstance(task, Mapping)
        }
    if values is None:
        values = {
            str(task.get("instance_id", "")): (
                task.get("abridged_p0_sha256")
                if key == "p0_sha256" and task.get("abridged_p0_sha256")
                else task.get(fallback)
            )
            for task in tasks
        }
    if not isinstance(values, Mapping) or set(values) != task_ids:
        raise analysis.AnalysisError(
            f"requirement revision omits one {key} digest for every task"
        )
    return {
        task_id: _digest(value, f"requirement_revision.{key}.{task_id}")
        for task_id, value in sorted(values.items())
    }


def _trigger_policy(protocol: Mapping[str, Any]) -> dict[str, Any]:
    value = protocol.get("trigger_policy") or protocol.get("trigger")
    if value is None:
        if (
            protocol.get("injection_trigger")
            in {
                "ExecutionFinished with exit code 0 and any non-empty affected_paths",
                "first qualifying ExecutionFinished; identical P1 delivered after the first phase settles",
            }
            and protocol.get("production_path_filter") is False
            and protocol.get("acknowledgement_event")
            == analysis.TRIGGER_POLICY["acknowledgement_event"]
            and protocol.get("acknowledgement_required") is True
        ):
            return dict(analysis.TRIGGER_POLICY)
        required = (
            "event",
            "success_exit_code",
            "requires_nonempty_affected_paths",
            "acknowledgement_event",
            "acknowledgement_required",
            "undelivered_policy",
        )
        if not all(key in protocol for key in required):
            raise analysis.AnalysisError(
                "requirement revision omits the exact trigger policy; refusing to freeze"
            )
        value = {key: protocol[key] for key in required}
    if dict(value) != analysis.TRIGGER_POLICY:
        raise analysis.AnalysisError(
            "trigger policy must be ExecutionFinished success with non-empty affected_paths"
        )
    return dict(analysis.TRIGGER_POLICY)


def _requirement_revision(
    manifest: Mapping[str, Any],
    tasks: Sequence[Mapping[str, Any]],
    protocol: Mapping[str, Any] | None,
) -> dict[str, Any]:
    run_plan = manifest.get("run_plan")
    manifest_source = (
        run_plan.get("requirement_revision")
        if isinstance(run_plan, Mapping)
        else None
    )
    if isinstance(manifest_source, Mapping):
        source: Mapping[str, Any] = dict(manifest_source)
    else:
        source = {}
    if isinstance(protocol, Mapping):
        protocol_source = protocol.get("requirement_revision", protocol)
        if not isinstance(protocol_source, Mapping):
            raise analysis.AnalysisError("protocol requirement_revision is not an object")
        source = {**source, **protocol_source}
    if not source:
        raise analysis.AnalysisError(
            "manifest/protocol has no contamination-safe requirement-revision metadata"
        )
    p0 = _hashes(tasks, source, "p0_sha256", "problem_statement_sha256")
    p1 = _hashes(tasks, source, "p1_sha256", "p1_sha256")
    trigger = _trigger_policy(source)
    return {
        "p0_sha256": p0,
        "p1_sha256": p1,
        "trigger_policy": trigger,
        "p0_hashes_sha256": analysis.canonical_hash(p0),
        "p1_hashes_sha256": analysis.canonical_hash(p1),
        "trigger_policy_sha256": analysis.canonical_hash(trigger),
    }


def _validate_manifest_run_plan(manifest: Mapping[str, Any]) -> None:
    run_plan = manifest.get("run_plan")
    if not isinstance(run_plan, Mapping):
        return
    expected = {
        "repositories": 10,
        "tasks": 30,
        "arms": list(analysis.PRIMARY_ARMS),
        "request_variants": list(analysis.REQUEST_VARIANTS),
        "repeat_labels": list(analysis.REPEAT_LABELS),
        "runs_per_model": analysis.RUNS_PER_MODEL,
    }
    for field, value in expected.items():
        if run_plan.get(field) != value:
            raise analysis.AnalysisError(
                f"manifest.run_plan.{field}: does not match the frozen 120-cell design"
            )


def build(
    manifest: Mapping[str, Any],
    *,
    study_id: str,
    model: str,
    runner: Path,
    dependency_lock: Path,
    qualification: Path,
    model_freeze: Path,
    frozen_at: str,
    protocol: Mapping[str, Any] | None = None,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    source = manifest.get("source")
    tasks = manifest.get("tasks")
    if not isinstance(source, Mapping) or not isinstance(tasks, list):
        raise analysis.AnalysisError("dataset manifest omits source or tasks")
    if len(tasks) != 30:
        raise analysis.AnalysisError("confirmatory freeze requires exactly 30 tasks")
    _validate_manifest_run_plan(manifest)
    requirement_revision = _requirement_revision(manifest, tasks, protocol)
    qualification_sha256 = analysis.file_hash(Path(qualification))
    model_freeze_sha256 = analysis.file_hash(Path(model_freeze))
    repositories: dict[str, list[str]] = {}
    for task in tasks:
        if not isinstance(task, Mapping):
            raise analysis.AnalysisError("dataset manifest task is not an object")
        repository = str(task.get("repo", ""))
        task_id = str(task.get("instance_id", ""))
        repositories.setdefault(repository, []).append(task_id)
    manifest_sha256 = analysis.canonical_hash(manifest)
    plan: dict[str, Any] = {
        "schema_version": 1,
        "study_id": study_id,
        "primary_model": model,
        "requested_model": model,
        "primary_variant": analysis.PRIMARY_VARIANT,
        "arms": list(analysis.PRIMARY_ARMS),
        "request_variants": list(analysis.REQUEST_VARIANTS),
        "dataset_revision": str(source.get("revision", "")),
        "dataset_manifest_sha256": manifest_sha256,
        "schedule_sha256": "0" * 64,
        "runs_per_model": analysis.RUNS_PER_MODEL,
        "scheduled_runs_per_model": analysis.RUNS_PER_MODEL,
        "session_token_budget": analysis.SESSION_TOKEN_BUDGET,
        "qualification_sha256": qualification_sha256,
        "model_freeze_sha256": model_freeze_sha256,
        "primary_estimand": analysis.PRIMARY_ESTIMAND,
        "interaction_estimand": analysis.INTERACTION_ESTIMAND,
        "attrition_definition": "terminal_failure_or_oracle_unknown",
        "repositories": {key: sorted(value) for key, value in sorted(repositories.items())},
        "repeats": list(analysis.REPEAT_LABELS),
        "requirement_revision": requirement_revision,
        "bootstrap_samples": 100_000,
        "bootstrap_seed": 20261013,
        "alpha": 0.05,
        "guardrail_margin": 0.05,
        "attrition_limit": 0.10,
        "minimum_baseline_correct_delivery_risk": 0.10,
        "minimum_method_correct_delivery_risk": 0.10,
    }
    plan["schedule_sha256"] = analysis.canonical_hash(analysis.schedule_payload(plan))
    plan = analysis.load_plan_object(plan)
    freeze = {
        "schema_version": 1,
        "study_id": study_id,
        "analysis_plan_sha256": analysis.canonical_hash(plan),
        "frozen_at": frozen_at,
        "primary_model": plan["primary_model"],
        "requested_model": plan["requested_model"],
        "dataset_revision": plan["dataset_revision"],
        "dataset_manifest_sha256": manifest_sha256,
        "schedule_sha256": plan["schedule_sha256"],
        "scheduled_runs_per_model": plan["scheduled_runs_per_model"],
        "session_token_budget": plan["session_token_budget"],
        "qualification_sha256": qualification_sha256,
        "model_freeze_sha256": model_freeze_sha256,
        "p0_hashes_sha256": requirement_revision["p0_hashes_sha256"],
        "p1_hashes_sha256": requirement_revision["p1_hashes_sha256"],
        "trigger_policy_sha256": requirement_revision["trigger_policy_sha256"],
        "artifact_sha256": {
            "analysis_script": analysis.file_hash(Path(analysis.__file__)),
            "runner": analysis.file_hash(runner),
            "dependency_lock": analysis.file_hash(dependency_lock),
            "qualification": qualification_sha256,
            "model_freeze": model_freeze_sha256,
        },
    }
    analysis.load_freeze_object(freeze)
    receipt = {
        "schema_version": 1,
        "study_id": study_id,
        "freeze_sha256": analysis.canonical_hash(freeze),
        "primary_model": plan["primary_model"],
        "requested_model": plan["requested_model"],
        "schedule_sha256": plan["schedule_sha256"],
        "runs_per_model": plan["runs_per_model"],
        "scheduled_runs_per_model": plan["scheduled_runs_per_model"],
        "session_token_budget": plan["session_token_budget"],
        "qualification_sha256": qualification_sha256,
        "model_freeze_sha256": model_freeze_sha256,
        "p0_hashes_sha256": requirement_revision["p0_hashes_sha256"],
        "p1_hashes_sha256": requirement_revision["p1_hashes_sha256"],
        "trigger_policy_sha256": requirement_revision["trigger_policy_sha256"],
        "publication_status": "must_be_published_before_scheduled_run_0",
    }
    return plan, freeze, receipt


def write_once(target: Path, values: Mapping[str, Mapping[str, Any]]) -> None:
    target.mkdir()
    for name, value in values.items():
        path = target / name
        with path.open("x", encoding="utf-8") as handle:
            json.dump(value, handle, indent=2, sort_keys=True, ensure_ascii=False)
            handle.write("\n")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("target", type=Path)
    parser.add_argument("--study-id", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--dependency-lock", type=Path, required=True)
    parser.add_argument("--qualification", type=Path, required=True)
    parser.add_argument("--model-freeze", type=Path, required=True)
    parser.add_argument(
        "--protocol",
        type=Path,
        help="optional contamination-safe P0/P1 and trigger metadata JSON",
    )
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    try:
        values = build(
            load_object(args.manifest),
            study_id=args.study_id,
            model=args.model,
            runner=args.runner,
            dependency_lock=args.dependency_lock,
            qualification=args.qualification,
            model_freeze=args.model_freeze,
            frozen_at=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            protocol=load_object(args.protocol) if args.protocol else None,
        )
        artifacts = dict(zip(("analysis-plan.json", "freeze.json", "receipt.json"), values))
        if not args.dry_run:
            write_once(args.target, artifacts)
    except (analysis.AnalysisError, OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(artifacts["receipt.json"], indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
