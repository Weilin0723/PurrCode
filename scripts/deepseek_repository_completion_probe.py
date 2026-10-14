#!/usr/bin/env python3
"""Prepare, analyze, and audit the frozen DeepSeek replication.

This module deliberately has no provider client.  ``prepare`` builds the
candidate snapshots and an immutable request bundle; an external Rust runner
may execute that bundle and write JSONL events.  ``analyze`` and ``audit`` are
offline projections over those events.  Credentials, credential files, and
environment variables are outside this script's input surface.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import math
import random
import re
import statistics
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

try:
    from . import repository_completion_probe as base
except ImportError:  # pragma: no cover - supports direct script execution
    import repository_completion_probe as base  # type: ignore[no-redef]


SCHEMA_VERSION = 1
PROVIDER = "deepseek"
MODEL = "deepseek-v4-pro"
MAX_OUTPUT_TOKENS = 128
CONDITIONS = tuple(base.CONDITIONS)
PRIMARY_CONDITION = base.PRIMARY_CONDITION
SEEDS = tuple(base.SEEDS)
EXPECTED_TASKS = 6
EXPECTED_CANDIDATES = 36
EXPECTED_CALLS = 144
MAX_ATTRITION_RATE = 0.10
BOOTSTRAP_SEED = 20261013
BOOTSTRAP_REPLICATES = 10_000
GUARDRAIL_MAX_LOSS = 0.05
TERMINAL_ERROR_EVENTS = frozenset(
    {
        "error",
        "model_request_failed",
        "provider_error",
        "model_request_error",
        "request_failed",
        "failed",
    }
)
TERMINAL_RESPONSE_EVENTS = frozenset(
    {
        "response",
        "model_response",
        "model_response_received",
        "model_request_finished",
        "request_finished",
        "completion",
        "completed",
    }
)
REQUEST_EVENTS = frozenset(
    {
        "request",
        "model_request_started",
        "provider_request_started",
        "request_started",
    }
)
INDEX_PATTERN = re.compile(r"(?:schedule[_ -]?index|schedule|call)[=: -]+(\d+)", re.I)


class DeepSeekProbeError(RuntimeError):
    """Raised for malformed or unverifiable research artifacts."""


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )


def json_hash(value: Any) -> str:
    return hashlib.sha256(canonical_json(value)).hexdigest()


def file_hash(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_object(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise DeepSeekProbeError(f"{label}: cannot read JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise DeepSeekProbeError(f"{label}: expected an object")
    return value


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def read_jsonl(path: Path, label: str) -> list[dict[str, Any]]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise DeepSeekProbeError(f"{label}: cannot read JSONL: {exc}") from exc
    values: list[dict[str, Any]] = []
    for line_number, line in enumerate(lines, 1):
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            raise DeepSeekProbeError(f"{label} line {line_number}: invalid JSON: {exc}") from exc
        if not isinstance(value, dict):
            raise DeepSeekProbeError(f"{label} line {line_number}: expected an object")
        values.append(value)
    return values


def _relative(path: Path, root: Path) -> str:
    try:
        return str(path.resolve().relative_to(root.resolve()))
    except ValueError:
        return str(path.resolve())


def _safe_artifact_path(output: Path, raw: Any, label: str) -> Path:
    if not isinstance(raw, str) or not raw:
        raise DeepSeekProbeError(f"{label}: missing path")
    path = Path(raw)
    if path.is_absolute() or ".." in path.parts:
        raise DeepSeekProbeError(f"{label}: path must stay inside output")
    resolved = (output / path).resolve()
    if output.resolve() not in resolved.parents:
        raise DeepSeekProbeError(f"{label}: path escapes output")
    return resolved


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _dependency_hashes(
    dataset: Mapping[str, Any], dataset_path: Path, fixture: Path, repo_root: Path
) -> dict[str, str]:
    script_path = Path(__file__).resolve()
    runner_path = (
        repo_root
        / "crates"
        / "evaluation-runtime"
        / "src"
        / "bin"
        / "repository_completion_remote.rs"
    )
    paths = set(
        base.dependency_paths(
            dataset,
            Path(base.__file__).resolve(),
            repo_root,
            dataset_path.resolve(),
        )
    )
    paths.update(
        {
            script_path,
            runner_path,
            dataset_path.resolve(),
            repo_root / "docs" / "research" / "repository-completion-deepseek-v1-plan.md",
        }
    )
    for raw_path in dataset.get("fixture_files", []):
        paths.add((fixture / base.safe_relative(str(raw_path))).resolve())
    result: dict[str, str] = {}
    for path in sorted(paths, key=lambda item: str(item)):
        if not path.is_file():
            raise DeepSeekProbeError(f"dependency is missing: {path}")
        result[_relative(path, repo_root)] = file_hash(path)
    return result


def _candidate_records(candidates: Sequence[Mapping[str, Any]]) -> list[dict[str, Any]]:
    return base.stable_candidate_records(candidates)


def build_deepseek_schedule(candidates: Sequence[Mapping[str, Any]]) -> list[dict[str, Any]]:
    """Build the frozen 2-condition x 36-candidate x 2-seed schedule."""

    schedule: list[dict[str, Any]] = []
    for condition in CONDITIONS:
        for candidate in candidates:
            for seed in SEEDS:
                schedule.append(
                    {
                        "schedule_index": len(schedule),
                        "model": MODEL,
                        "task_id": candidate["task_id"],
                        "candidate": candidate["candidate"],
                        "condition": condition,
                        "seed": int(seed),
                    }
                )
    return schedule


def request_payload(prompt: str) -> dict[str, Any]:
    """Return the logical request executed by the credential-owning Rust runner.

    Sampling controls and API seeds are deliberately absent: the configured
    gateway does not send them for OpenAI-compatible structured requests.
    Replicate labels remain schedule metadata rather than a claimed API seed.
    """

    return {
        "provider": PROVIDER,
        "model": MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "max_output_tokens": MAX_OUTPUT_TOKENS,
        "structured_output": {
            "type": "json_object",
            "required": ["verdict", "reasons"],
        },
        "thinking": "disabled_by_provider_gateway_for_structured_deepseek_requests",
    }


def _bundle_records(
    schedule: Sequence[Mapping[str, Any]],
    candidates: Sequence[Mapping[str, Any]],
) -> list[dict[str, Any]]:
    candidate_by_key = {
        (str(candidate["task_id"]), str(candidate["candidate"])): candidate
        for candidate in candidates
    }
    records: list[dict[str, Any]] = []
    for item in schedule:
        candidate = candidate_by_key[(str(item["task_id"]), str(item["candidate"]))]
        prompt = candidate["prompts"][str(item["condition"])]
        if not isinstance(prompt, str):
            raise DeepSeekProbeError("candidate prompt is not a string")
        forbidden = base.prompt_contains_forbidden_metadata(prompt)
        if forbidden:
            raise DeepSeekProbeError(
                f"prompt {item['schedule_index']} contains forbidden metadata: {forbidden}"
            )
        payload = request_payload(prompt)
        records.append(
            {
                "schedule_index": int(item["schedule_index"]),
                "task_id": item["task_id"],
                "candidate": item["candidate"],
                "condition": item["condition"],
                "seed": int(item["seed"]),
                "model": MODEL,
                "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
                "payload_sha256": json_hash(payload),
                "payload": payload,
            }
        )
    return records


def _write_bundle(path: Path, records: Sequence[Mapping[str, Any]]) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n" for record in records),
        encoding="utf-8",
    )
    return file_hash(path)


def _candidate_manifest(candidates: Sequence[Mapping[str, Any]]) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for candidate in candidates:
        records.append(
            {
                "task_id": candidate["task_id"],
                "candidate": candidate["candidate"],
                "tree_digest": candidate["tree_digest"],
                "baseline_tree_digest": candidate["baseline_tree_digest"],
                "changed_paths": candidate["changed_paths"],
                "metadata": candidate["metadata"],
                "external_accepted": candidate["external_accepted"],
                "dataset_expected_accepted": candidate["dataset_expected_accepted"],
                "oracle_isolated": candidate["oracle_isolated"],
                "preflight_match": candidate["preflight_match"],
                "prompt_sha256": candidate["prompt_sha256"],
                "prompt_forbidden_terms": candidate["prompt_forbidden_terms"],
            }
        )
    return records


def _validate_preflight(
    dataset: Mapping[str, Any], candidates: Sequence[Mapping[str, Any]], smoke: Mapping[str, Any]
) -> tuple[bool, list[str]]:
    errors: list[str] = []
    if len(candidates) != EXPECTED_CANDIDATES:
        errors.append(f"expected {EXPECTED_CANDIDATES} candidate snapshots, got {len(candidates)}")
    if smoke.get("go") is not True:
        errors.append("preflight smoke did not pass")
    for candidate in candidates:
        if candidate.get("preflight_match") is not True:
            errors.append(
                f"candidate preflight mismatch: {candidate.get('task_id')}/{candidate.get('candidate')}"
            )
        if candidate.get("prompt_forbidden_terms"):
            errors.append(
                f"candidate prompt forbidden terms: {candidate.get('task_id')}/{candidate.get('candidate')}"
            )
    if len(dataset.get("tasks", [])) != EXPECTED_TASKS:
        errors.append(f"expected {EXPECTED_TASKS} tasks")
    return not errors, errors


def prepare(
    output: Path,
    *,
    dataset_path: Path = base.DEFAULT_DATASET,
    fixture: Path | None = None,
) -> dict[str, Any]:
    """Create the offline, pre-call DeepSeek freeze and request bundle."""

    output = output.resolve()
    if output.exists():
        raise DeepSeekProbeError(f"output directory already exists: {output}")
    output.mkdir(parents=True)
    dataset, fixture_path, repo_root = base.load_inputs(dataset_path, fixture)
    candidates = base.build_candidates(dataset, fixture_path, repo_root)
    schedule = build_deepseek_schedule(candidates)
    if len(schedule) != EXPECTED_CALLS:
        raise DeepSeekProbeError(f"frozen schedule must contain {EXPECTED_CALLS} calls")
    candidate_records = _candidate_manifest(candidates)
    smoke = base.smoke_report(dataset, candidates)
    preflight_ok, preflight_errors = _validate_preflight(dataset, candidates, smoke)
    bundle = _bundle_records(schedule, candidates)
    bundle_path = output / "requests.jsonl"
    bundle_sha256 = _write_bundle(bundle_path, bundle)
    dependencies = _dependency_hashes(dataset, dataset_path, fixture_path, repo_root)
    plan = repo_root / "docs" / "research" / "repository-completion-deepseek-v1-plan.md"
    if not plan.is_file():
        raise DeepSeekProbeError(f"preregistration plan is missing: {plan}")
    freeze = {
        "schema_version": SCHEMA_VERSION,
        "freeze_kind": "deepseek-repository-completion-pre-call-freeze",
        "created_before_model_calls": True,
        "provider": PROVIDER,
        "model": MODEL,
        "dataset": _relative(dataset_path.resolve(), repo_root),
        "dataset_sha256": file_hash(dataset_path.resolve()),
        "fixture": _relative(fixture_path, repo_root),
        "fixture_tree_sha256": base.tree_digest(fixture_path),
        "script": _relative(Path(__file__).resolve(), repo_root),
        "script_sha256": file_hash(Path(__file__).resolve()),
        "runner_source": "crates/evaluation-runtime/src/bin/repository_completion_remote.rs",
        "runner_source_sha256": file_hash(
            repo_root
            / "crates"
            / "evaluation-runtime"
            / "src"
            / "bin"
            / "repository_completion_remote.rs"
        ),
        "base_probe_script_sha256": file_hash(Path(base.__file__).resolve()),
        "dependency_hashes": dependencies,
        "preregistration_plan": _relative(plan, repo_root),
        "preregistration_plan_sha256": file_hash(plan),
        "conditions": list(CONDITIONS),
        "primary_condition": PRIMARY_CONDITION,
        "seeds": list(SEEDS),
        "schedule_order": "condition block, then task,candidate,seed",
        "scheduled_calls": len(schedule),
        "schedule": schedule,
        "schedule_sha256": json_hash(schedule),
        "candidate_preflight_sha256": json_hash(candidate_records),
        "smoke_sha256": json_hash(smoke),
        "preflight_ok": preflight_ok,
        "preflight_errors": preflight_errors,
        "request_parameters": {
            "max_output_tokens": MAX_OUTPUT_TOKENS,
            "response_format": {"type": "json_object"},
            "thinking": "disabled_by_provider_gateway_for_structured_deepseek_requests",
            "temperature": "provider_default",
            "top_p": "provider_default",
            "api_seed": None,
        },
        "requests_bundle": {"path": "requests.jsonl", "sha256": bundle_sha256},
        "model_calls": 0,
    }
    freeze_path = output / "freeze.json"
    write_json(freeze_path, freeze)
    manifest = {
        "schema_version": SCHEMA_VERSION,
        "artifact_kind": "deepseek-repository-completion-replication",
        "status": "ready" if preflight_ok else "blocked_preflight",
        "created_at": datetime.now(timezone.utc).isoformat(),
        "provider": PROVIDER,
        "model": MODEL,
        "primary_analysis": True,
        "analysis_role": "primary_replication",
        "local_prepare_only": True,
        "network_calls": 0,
        "credential_inputs": [],
        "dataset": freeze["dataset"],
        "dataset_sha256": freeze["dataset_sha256"],
        "fixture": freeze["fixture"],
        "fixture_tree_sha256": freeze["fixture_tree_sha256"],
        "preregistration_plan": freeze["preregistration_plan"],
        "preregistration_plan_sha256": freeze["preregistration_plan_sha256"],
        "script": freeze["script"],
        "script_sha256": freeze["script_sha256"],
        "runner_source": freeze["runner_source"],
        "runner_source_sha256": freeze["runner_source_sha256"],
        "base_probe_script_sha256": freeze["base_probe_script_sha256"],
        "dependency_hashes": dependencies,
        "conditions": list(CONDITIONS),
        "primary_condition": PRIMARY_CONDITION,
        "seeds": list(SEEDS),
        "schedule_order": freeze["schedule_order"],
        "scheduled_calls": EXPECTED_CALLS,
        "schedule": schedule,
        "schedule_sha256": freeze["schedule_sha256"],
        "candidate_preflight": candidate_records,
        "candidate_preflight_sha256": freeze["candidate_preflight_sha256"],
        "smoke": smoke,
        "preflight_ok": preflight_ok,
        "preflight_errors": preflight_errors,
        "requests_bundle": freeze["requests_bundle"],
        "freeze": {"path": "freeze.json", "sha256": file_hash(freeze_path)},
        "runner_events": {"path": "events.jsonl", "sha256": None},
        "analysis_artifacts": {},
    }
    write_json(output / "manifest.json", manifest)
    write_json(output / "smoke.json", smoke)
    return {
        "status": manifest["status"],
        "output": str(output),
        "scheduled_calls": EXPECTED_CALLS,
        "requests_bundle": manifest["requests_bundle"],
        "freeze_sha256": manifest["freeze"]["sha256"],
        "preflight_ok": preflight_ok,
        "preflight_errors": preflight_errors,
    }


def _event_data(event: Mapping[str, Any]) -> dict[str, Any]:
    data = event.get("data")
    merged: dict[str, Any] = dict(data) if isinstance(data, dict) else {}
    for key, value in event.items():
        if key not in {"data", "event", "type"}:
            merged[key] = value
    return merged


def _event_kind(event: Mapping[str, Any]) -> str:
    raw = event.get("event", event.get("type", ""))
    return str(raw).lower()


def _event_index(event: Mapping[str, Any]) -> int | None:
    fields = _event_data(event)
    for key in ("schedule_index", "scheduleIndex", "call_index", "callIndex"):
        value = fields.get(key)
        if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
            return value
        if isinstance(value, str) and value.isdigit():
            return int(value)
    for key in ("role", "request_id", "requestId", "id"):
        value = fields.get(key)
        if isinstance(value, str):
            match = INDEX_PATTERN.search(value)
            if match:
                return int(match.group(1))
    return None


def _assign_events(
    events: Sequence[Mapping[str, Any]], schedule: Sequence[Mapping[str, Any]]
) -> tuple[dict[int, list[dict[str, Any]]], list[str]]:
    """Assign Rust events to schedule rows, using explicit indices when present."""

    expected = set(range(len(schedule)))
    assigned: dict[int, list[dict[str, Any]]] = {index: [] for index in expected}
    errors: list[str] = []
    request_indices: list[int] = []
    used_request_indices: set[int] = set()
    next_index = 0

    def fallback_index() -> int | None:
        nonlocal next_index
        while next_index in used_request_indices and next_index < len(schedule):
            next_index += 1
        if next_index >= len(schedule):
            return None
        value = next_index
        used_request_indices.add(value)
        next_index += 1
        return value

    for event in events:
        kind = _event_kind(event)
        if kind not in REQUEST_EVENTS:
            continue
        index = _event_index(event)
        if index is None:
            index = fallback_index()
        elif index in used_request_indices:
            errors.append(f"duplicate request schedule index {index}")
        else:
            used_request_indices.add(index)
        if index is None:
            errors.append("request event has no available schedule index")
            continue
        if index not in expected:
            errors.append(f"request event references unknown schedule index {index}")
        request_indices.append(index)
        event_copy = dict(event)
        event_copy["_assigned_index"] = index
        if index in assigned:
            assigned[index].append(event_copy)

    terminal_seen = 0
    for event in events:
        kind = _event_kind(event)
        if kind in REQUEST_EVENTS:
            continue
        if kind not in TERMINAL_ERROR_EVENTS and kind not in TERMINAL_RESPONSE_EVENTS:
            continue
        index = _event_index(event)
        if index is None:
            if terminal_seen < len(request_indices):
                index = request_indices[terminal_seen]
            else:
                index = terminal_seen
        terminal_seen += 1
        if index not in expected:
            errors.append(f"terminal event references unknown schedule index {index}")
            continue
        event_copy = dict(event)
        event_copy["_assigned_index"] = index
        assigned[index].append(event_copy)
    return assigned, errors


def _as_nonnegative_int(value: Any) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        return None
    return value


def _usage_from(value: Any) -> dict[str, int | None]:
    if not isinstance(value, Mapping):
        return {"prompt": None, "completion": None, "total": None}
    usage = value.get("usage") if isinstance(value.get("usage"), Mapping) else value
    prompt = None
    completion = None
    total = None
    for key in ("prompt_tokens", "input_tokens", "prompt_eval_count"):
        prompt = _as_nonnegative_int(usage.get(key))
        if prompt is not None:
            break
    for key in ("completion_tokens", "output_tokens", "eval_count"):
        completion = _as_nonnegative_int(usage.get(key))
        if completion is not None:
            break
    for key in ("total_tokens", "total"):
        total = _as_nonnegative_int(usage.get(key))
        if total is not None:
            break
    if total is None and prompt is not None and completion is not None:
        total = prompt + completion
    return {"prompt": prompt, "completion": completion, "total": total}


def _latency_from(value: Any) -> float | None:
    if not isinstance(value, Mapping):
        return None
    for key in ("latency_ms", "elapsed_ms", "duration_ms"):
        raw = value.get(key)
        if isinstance(raw, (int, float)) and not isinstance(raw, bool) and raw >= 0:
            return float(raw)
    raw_ns = value.get("duration_ns")
    if isinstance(raw_ns, (int, float)) and not isinstance(raw_ns, bool) and raw_ns >= 0:
        return float(raw_ns) / 1_000_000.0
    return None


def _verdict_from(value: Any) -> tuple[str, list[str]]:
    if isinstance(value, Mapping):
        if set(value) == {"verdict", "reasons"} and value.get("verdict") in base.VERDICTS:
            reasons = value.get("reasons")
            if isinstance(reasons, list) and all(isinstance(item, str) for item in reasons):
                return str(value["verdict"]), list(reasons)
            raise ValueError("reasons must be a string array")
        if "response" in value:
            return _verdict_from(value["response"])
        for key in ("output", "result", "structured", "value", "content", "text"):
            if key in value:
                try:
                    return _verdict_from(value[key])
                except ValueError:
                    pass
        raise ValueError("terminal event contains no exact verdict object")
    if isinstance(value, str):
        try:
            decoded = json.loads(value.strip())
        except json.JSONDecodeError as exc:
            raise ValueError(f"response is not JSON: {exc}") from exc
        return _verdict_from(decoded)
    raise ValueError("response is not an object or JSON string")


def _terminal_projection(events: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    if not events:
        return {
            "status": "missing_terminal",
            "model_verdict": None,
            "model_reasons": [],
            "tokens": {"prompt": None, "completion": None, "total": None},
            "latency_ms": None,
            "error": "no terminal event",
            "terminal_event": None,
        }
    terminal = [event for event in events if _event_kind(event) not in REQUEST_EVENTS]
    if len(terminal) != 1:
        return {
            "status": "invalid",
            "model_verdict": None,
            "model_reasons": [],
            "tokens": {"prompt": None, "completion": None, "total": None},
            "latency_ms": None,
            "error": "multiple terminal events" if terminal else "no terminal event",
            "terminal_event": None if not terminal else _event_kind(terminal[-1]),
        }
    event = terminal[0]
    kind = _event_kind(event)
    fields = _event_data(event)
    usage = _usage_from(fields)
    latency = _latency_from(fields)
    if kind in TERMINAL_ERROR_EVENTS or fields.get("status") in {"error", "failed", "timeout"}:
        return {
            "status": "error",
            "model_verdict": None,
            "model_reasons": [],
            "tokens": usage,
            "latency_ms": latency,
            "error": str(fields.get("message", fields.get("error", "provider error"))),
            "terminal_event": kind,
        }
    response_value: Any = (
        {"verdict": fields.get("verdict"), "reasons": fields.get("reasons")}
        if "verdict" in fields or "reasons" in fields
        else fields.get("response", fields)
    )
    try:
        verdict, reasons = _verdict_from(response_value)
    except ValueError as exc:
        return {
            "status": "invalid",
            "model_verdict": None,
            "model_reasons": [],
            "tokens": usage,
            "latency_ms": latency,
            "error": str(exc),
            "terminal_event": kind,
        }
    return {
        "status": "ok",
        "model_verdict": verdict,
        "model_reasons": reasons,
        "tokens": usage,
        "latency_ms": latency,
        "error": None,
        "terminal_event": kind,
    }


def _candidate_by_key(manifest: Mapping[str, Any]) -> dict[tuple[str, str], Mapping[str, Any]]:
    raw = manifest.get("candidate_preflight")
    if not isinstance(raw, list):
        raise DeepSeekProbeError("manifest.candidate_preflight is missing")
    result: dict[tuple[str, str], Mapping[str, Any]] = {}
    for item in raw:
        if not isinstance(item, Mapping):
            raise DeepSeekProbeError("candidate_preflight contains a non-object")
        key = (str(item.get("task_id")), str(item.get("candidate")))
        if key in result:
            raise DeepSeekProbeError(f"duplicate candidate metadata {key}")
        result[key] = item
    return result


def _rows_from_events(
    manifest: Mapping[str, Any], events: Sequence[Mapping[str, Any]]
) -> tuple[list[dict[str, Any]], list[str]]:
    schedule = manifest.get("schedule")
    if not isinstance(schedule, list):
        raise DeepSeekProbeError("manifest.schedule is missing")
    candidates = _candidate_by_key(manifest)
    assigned, errors = _assign_events(events, schedule)
    rows: list[dict[str, Any]] = []
    for item in schedule:
        index = int(item["schedule_index"])
        candidate = candidates.get((str(item["task_id"]), str(item["candidate"])))
        if candidate is None:
            raise DeepSeekProbeError(f"no candidate metadata for schedule index {index}")
        metadata = candidate.get("metadata")
        if not isinstance(metadata, Mapping):
            raise DeepSeekProbeError(f"candidate metadata missing for schedule index {index}")
        gate = base.deterministic_revision_gate(metadata)
        terminal = _terminal_projection(assigned.get(index, []))
        verdict = terminal["model_verdict"]
        model_ready = terminal["status"] == "ok" and verdict == "READY"
        terminal_observed = terminal["status"] != "missing_terminal"
        rows.append(
            {
                "schedule_index": index,
                "task_id": item["task_id"],
                "candidate": item["candidate"],
                "condition": item["condition"],
                "seed": int(item["seed"]),
                "external_accepted": candidate.get("external_accepted"),
                "dataset_expected_accepted": candidate.get("dataset_expected_accepted"),
                "deterministic_gate": gate,
                "status": terminal["status"],
                "model_verdict": verdict,
                "model_reasons": terminal["model_reasons"],
                "B": bool(model_ready) if terminal_observed else None,
                "C": (
                    bool(model_ready and gate == "READY")
                    if terminal_observed
                    else None
                ),
                "D": gate == "READY",
                "tokens": terminal["tokens"],
                "latency_ms": terminal["latency_ms"],
                "terminal_event": terminal["terminal_event"],
                "error": terminal["error"],
            }
        )
    return rows, errors


def _rate(successes: int, trials: int) -> float | None:
    return None if trials == 0 else successes / trials


def _metric(rows: Sequence[Mapping[str, Any]], arm: str) -> dict[str, Any]:
    valid = [row for row in rows if row.get("status") == "ok"]
    terminal = [row for row in rows if row.get(arm) is not None]
    incorrect_scheduled = [row for row in rows if row.get("external_accepted") is False]
    incorrect = [row for row in valid if row.get("external_accepted") is False]
    incorrect_terminal = [row for row in terminal if row.get("external_accepted") is False]
    accepted_scheduled = [row for row in rows if row.get("external_accepted") is True]
    accepted = [row for row in valid if row.get("external_accepted") is True]
    accepted_terminal = [row for row in terminal if row.get("external_accepted") is True]
    claims = [row for row in terminal if row.get(arm) is True]
    false_ready = [row for row in claims if row.get("external_accepted") is False]
    correct_delivery = [row for row in claims if row.get("external_accepted") is True]
    return {
        "scheduled_calls": len(rows),
        "valid_responses": len(valid),
        "completion_claims": len(claims),
        "external_incorrect_scheduled": len(incorrect_scheduled),
        "external_incorrect_valid": len(incorrect),
        "external_accepted_scheduled": len(accepted_scheduled),
        "external_accepted_valid": len(accepted),
        "false_ready": {
            "count": len(false_ready),
            "denominator_valid_external_incorrect": len(incorrect),
            "denominator_terminal_external_incorrect": len(incorrect_terminal),
            "denominator_scheduled_external_incorrect": len(incorrect_scheduled),
            "among_valid_external_incorrect": _rate(len(false_ready), len(incorrect)),
            "intent_to_treat": _rate(len(false_ready), len(incorrect_scheduled)),
        },
        "correct_delivery": _rate(len(correct_delivery), len(accepted_scheduled)),
        "coverage": _rate(len(correct_delivery), len(accepted_scheduled)),
        "coverage_among_valid": _rate(len(correct_delivery), len(accepted)),
        "accepted_terminal": len(accepted_terminal),
        "completion_claim_rate_valid": _rate(len(claims), len(valid)),
        "completion_claim_rate_scheduled": _rate(len(claims), len(rows)),
    }


def _cluster_metrics(rows: Sequence[Mapping[str, Any]], condition: str) -> list[dict[str, Any]]:
    selected = [row for row in rows if row.get("condition") == condition]
    task_ids = sorted({str(row.get("task_id")) for row in selected})
    clusters: list[dict[str, Any]] = []
    for task_id in task_ids:
        task_rows = [row for row in selected if str(row.get("task_id")) == task_id]
        b = _metric(task_rows, "B")
        c = _metric(task_rows, "C")
        b_rate = b["false_ready"]["intent_to_treat"]
        c_rate = c["false_ready"]["intent_to_treat"]
        diff = None if b_rate is None or c_rate is None else c_rate - b_rate
        clusters.append({"task_id": task_id, "B": b, "C": c, "risk_difference_C_minus_B": diff})
    return clusters


def _exact_sign_p(wins: int, losses: int) -> float | None:
    discordant = wins + losses
    if discordant == 0:
        return None
    tail = sum(math.comb(discordant, index) for index in range(min(wins, losses) + 1))
    return min(1.0, 2.0 * tail / (2**discordant))


def _percentile(values: Sequence[float], probability: float) -> float:
    ordered = sorted(values)
    position = (len(ordered) - 1) * probability
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def _bootstrap(clusters: Sequence[Mapping[str, Any]]) -> list[float] | None:
    if not clusters or any(cluster.get("risk_difference_C_minus_B") is None for cluster in clusters):
        return None
    rng = random.Random(BOOTSTRAP_SEED)
    estimates: list[float] = []
    for _ in range(BOOTSTRAP_REPLICATES):
        sample = [clusters[rng.randrange(len(clusters))] for _ in clusters]
        b_n = sum(
            cluster["B"]["false_ready"]["count"] for cluster in sample
        )
        c_n = sum(
            cluster["C"]["false_ready"]["count"] for cluster in sample
        )
        b_d = sum(
            cluster["B"]["false_ready"]["denominator_scheduled_external_incorrect"]
            for cluster in sample
        )
        c_d = sum(
            cluster["C"]["false_ready"]["denominator_scheduled_external_incorrect"]
            for cluster in sample
        )
        if b_d == 0 or c_d == 0:
            continue
        estimates.append(c_n / c_d - b_n / b_d)
    if not estimates:
        return None
    return [_percentile(estimates, 0.025), _percentile(estimates, 0.975)]


def _sign_flip(clusters: Sequence[Mapping[str, Any]]) -> list[float] | None:
    differences = [cluster.get("risk_difference_C_minus_B") for cluster in clusters]
    if not differences or any(value is None for value in differences):
        return None
    values = []
    for signs in itertools.product((-1.0, 1.0), repeat=len(differences)):
        values.append(sum(float(value) * sign for value, sign in zip(differences, signs)) / len(differences))
    return [_percentile(values, 0.025), _percentile(values, 0.975)]


def comparison(
    rows: Sequence[Mapping[str, Any]], condition: str, *, infer: bool, exploratory: bool = False
) -> dict[str, Any]:
    selected = [row for row in rows if row.get("condition") == condition]
    clusters = _cluster_metrics(rows, condition)
    b = _metric(selected, "B")
    c = _metric(selected, "C")
    differences = [cluster["risk_difference_C_minus_B"] for cluster in clusters]
    wins = sum(value is not None and value < 0 for value in differences)
    losses = sum(value is not None and value > 0 for value in differences)
    ties = sum(value == 0 for value in differences if value is not None)
    complete_clusters = len(differences) == EXPECTED_TASKS and all(value is not None for value in differences)
    eligible = bool(infer and complete_clusters)
    p_value = _exact_sign_p(wins, losses) if eligible else None
    b_rate = b["false_ready"]["intent_to_treat"]
    c_rate = c["false_ready"]["intent_to_treat"]
    return {
        "estimand": "rubric-condition C minus B false-ready risk among externally rejected candidates, clustered by base task",
        "condition": condition,
        "exploratory": exploratory,
        "inference_eligible": eligible,
        "risk_difference_C_minus_B": None if b_rate is None or c_rate is None else c_rate - b_rate,
        "significant": bool(eligible and p_value is not None and p_value < 0.05),
        "bootstrap95": _bootstrap(clusters) if eligible else None,
        "exact_sign_flip_reference95": _sign_flip(clusters) if eligible else None,
        "exact_sign_flip": {
            "method": "exact paired-cluster sign-flip descriptive null-reference interval (not a confidence interval)",
            "assignments": 1 << len(clusters),
            "reference_interval95": _sign_flip(clusters) if eligible else None,
        },
        "sign_test": {
            "wins_C": wins,
            "losses_C": losses,
            "ties": ties,
            "p_value": p_value,
            "method": "two-sided exact sign test over six base-task clusters",
        },
        "aggregate": {"B": b, "C": c},
        "clusters": clusters,
    }


def _unique_candidate_rows(rows: Sequence[Mapping[str, Any]]) -> list[Mapping[str, Any]]:
    selected: dict[tuple[str, str], Mapping[str, Any]] = {}
    for row in sorted(rows, key=lambda item: int(item["schedule_index"])):
        selected.setdefault((str(row["task_id"]), str(row["candidate"])), row)
    return list(selected.values())


def _usage_latency(rows: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    usage_rows = [row["tokens"] for row in rows if isinstance(row.get("tokens"), Mapping)]
    prompt = [value["prompt"] for value in usage_rows if isinstance(value.get("prompt"), int)]
    completion = [value["completion"] for value in usage_rows if isinstance(value.get("completion"), int)]
    total = [value["total"] for value in usage_rows if isinstance(value.get("total"), int)]
    latencies = [float(row["latency_ms"]) for row in rows if isinstance(row.get("latency_ms"), (int, float))]
    return {
        "usage": {
            "prompt_tokens": sum(prompt) if prompt else None,
            "completion_tokens": sum(completion) if completion else None,
            "total_tokens": sum(total) if total else None,
            "measured_prompt_calls": len(prompt),
            "measured_completion_calls": len(completion),
            "measured_total_calls": len(total),
            "missing_total_calls": len(rows) - len(total),
        },
        "latency_ms": {
            "mean": statistics.fmean(latencies) if latencies else None,
            "median": statistics.median(latencies) if latencies else None,
            "p95": _percentile(latencies, 0.95) if latencies else None,
            "measured_calls": len(latencies),
            "missing_calls": len(rows) - len(latencies),
        },
    }


def summarize(
    manifest: Mapping[str, Any], rows: Sequence[Mapping[str, Any]], event_errors: Sequence[str]
) -> dict[str, Any]:
    scheduled = int(manifest.get("scheduled_calls", 0))
    counts = {status: sum(row.get("status") == status for row in rows) for status in {
        "ok", "error", "invalid", "missing_terminal"
    }}
    terminal_failures = scheduled - counts["ok"]
    attrition_rate = terminal_failures / scheduled if scheduled else 1.0
    schedule_complete = len(rows) == scheduled and {
        int(row["schedule_index"]) for row in rows
    } == set(range(scheduled))
    operationally_inconclusive = attrition_rate > MAX_ATTRITION_RATE or bool(event_errors)
    infer = bool(schedule_complete and not operationally_inconclusive)
    primary = comparison(rows, PRIMARY_CONDITION, infer=infer)
    plain = comparison(rows, next(condition for condition in CONDITIONS if condition != PRIMARY_CONDITION), infer=infer, exploratory=True)
    d_rows = _unique_candidate_rows(rows)
    guardrail_rows = [row for row in rows if row.get("condition") == PRIMARY_CONDITION]
    b_guard = _metric(guardrail_rows, "B")
    c_guard = _metric(guardrail_rows, "C")
    b_cov = b_guard["coverage"]
    c_cov = c_guard["coverage"]
    coverage_delta = None if b_cov is None or c_cov is None else c_cov - b_cov
    guardrail_pass = None if coverage_delta is None else coverage_delta >= -GUARDRAIL_MAX_LOSS
    usage_latency = _usage_latency(rows)
    return {
        "schema_version": SCHEMA_VERSION,
        "artifact_kind": "deepseek-repository-completion-replication",
        "status": "inconclusive" if operationally_inconclusive else "complete" if schedule_complete else "incomplete",
        "operationally_inconclusive": operationally_inconclusive,
        "inference_eligible": infer,
        "schedule_complete": schedule_complete,
        "scheduled_calls": scheduled,
        "completed_records": len(rows),
        "valid_responses": counts["ok"],
        "attrition": {
            "error": counts["error"],
            "invalid": counts["invalid"],
            "missing_terminal": counts["missing_terminal"],
            "failed_or_invalid_or_missing": terminal_failures,
            "rate": attrition_rate,
            "threshold": MAX_ATTRITION_RATE,
            "event_errors": list(event_errors),
        },
        "status_counts": counts,
        "tasks": len({str(row.get("task_id")) for row in rows}),
        "candidate_snapshots": len(manifest.get("candidate_preflight", [])),
        "d_evaluations": len(d_rows),
        "d_metrics": _metric(d_rows, "D"),
        "model": MODEL,
        "primary_condition": PRIMARY_CONDITION,
        "primary_comparison": primary,
        "plain_secondary": plain,
        "guardrail": {
            "definition": "C correct-delivery coverage must not fall by more than 5pp versus B",
            "B_correct_coverage": b_cov,
            "C_correct_coverage": c_cov,
            "C_minus_B": coverage_delta,
            "maximum_allowed_loss": GUARDRAIL_MAX_LOSS,
            "pass": guardrail_pass,
        },
        **usage_latency,
        "no_conclusion_if_inconclusive": operationally_inconclusive,
    }


def analysis_document(summary: Mapping[str, Any]) -> dict[str, Any]:
    primary = summary["primary_comparison"]
    if summary["operationally_inconclusive"]:
        interpretation = "Operationally inconclusive because terminal attrition or event integrity exceeded the preregistered threshold."
    elif primary["significant"]:
        interpretation = "The preregistered rubric-condition C-minus-B false-ready contrast is statistically significant; interpret it jointly with the 5pp correct-coverage guardrail."
    else:
        interpretation = "The preregistered rubric-condition contrast is not statistically significant under the frozen six-cluster analysis."
    return {
        "schema_version": SCHEMA_VERSION,
        "analysis_kind": "deepseek-repository-completion-analysis",
        "model": MODEL,
        "estimand": primary["estimand"],
        "primary": primary,
        "plain_secondary": summary["plain_secondary"],
        "guardrail": summary["guardrail"],
        "attrition": summary["attrition"],
        "usage": summary["usage"],
        "latency_ms": summary["latency_ms"],
        "interpretation": interpretation,
    }


def analyze(output: Path, events_path: Path | None = None) -> dict[str, Any]:
    output = output.resolve()
    manifest_path = output / "manifest.json"
    manifest = read_object(manifest_path, "manifest")
    if manifest.get("preflight_ok") is not True:
        raise DeepSeekProbeError("cannot analyze a failed preflight")
    event_file = (events_path or output / "events.jsonl").resolve()
    events = read_jsonl(event_file, "runner events")
    rows, event_errors = _rows_from_events(manifest, events)
    summary = summarize(manifest, rows, event_errors)
    analysis = analysis_document(summary)
    write_json(output / "results.json", {"schema_version": SCHEMA_VERSION, "results": rows})
    write_json(output / "summary.json", summary)
    write_json(output / "analysis.json", analysis)
    results_hash = file_hash(output / "results.json")
    summary_hash = file_hash(output / "summary.json")
    analysis_hash = file_hash(output / "analysis.json")
    manifest["runner_events"] = {"path": str(event_file), "sha256": file_hash(event_file)}
    manifest["analysis_artifacts"] = {
        "results_sha256": results_hash,
        "summary_sha256": summary_hash,
        "analysis_sha256": analysis_hash,
    }
    manifest["status"] = summary["status"]
    manifest["completed_records"] = len(rows)
    write_json(manifest_path, manifest)
    return summary


def _compare_json(expected: Any, actual: Any, label: str, errors: list[str]) -> None:
    if expected != actual:
        errors.append(f"{label} mismatch")


def _audit_bundle(output: Path, manifest: Mapping[str, Any], errors: list[str]) -> list[dict[str, Any]]:
    bundle_ref = manifest.get("requests_bundle")
    if not isinstance(bundle_ref, Mapping):
        errors.append("manifest.requests_bundle missing")
        return []
    try:
        path = _safe_artifact_path(output, bundle_ref.get("path"), "requests bundle")
    except DeepSeekProbeError as exc:
        errors.append(str(exc))
        return []
    if not path.is_file():
        errors.append("requests bundle is missing")
        return []
    if file_hash(path) != bundle_ref.get("sha256"):
        errors.append("requests bundle hash mismatch")
    records = read_jsonl(path, "requests bundle")
    if len(records) != EXPECTED_CALLS:
        errors.append(f"requests bundle must contain {EXPECTED_CALLS} records")
    expected_schedule = manifest.get("schedule")
    by_index: dict[int, Mapping[str, Any]] = {}
    for record in records:
        index = record.get("schedule_index")
        if not isinstance(index, int) or index in by_index:
            errors.append(f"requests bundle has invalid/duplicate index {index!r}")
            continue
        by_index[index] = record
        if index < 0 or index >= len(expected_schedule or []):
            errors.append(f"requests bundle index out of range {index}")
            continue
        item = expected_schedule[index]
        for key in ("task_id", "candidate", "condition", "seed"):
            if record.get(key) != item.get(key):
                errors.append(f"request {index} {key} mismatch")
        payload = record.get("payload")
        if not isinstance(payload, Mapping):
            errors.append(f"request {index} payload missing")
            continue
        if record.get("payload_sha256") != json_hash(payload):
            errors.append(f"request {index} payload hash mismatch")
        prompt = payload.get("messages", [{}])[0].get("content") if isinstance(payload.get("messages"), list) and payload.get("messages") else None
        if not isinstance(prompt, str):
            errors.append(f"request {index} prompt missing")
            continue
        if record.get("prompt_sha256") != hashlib.sha256(prompt.encode("utf-8")).hexdigest():
            errors.append(f"request {index} prompt hash mismatch")
        if base.prompt_contains_forbidden_metadata(prompt):
            errors.append(f"request {index} contains forbidden metadata")
        if (
            payload.get("provider") != PROVIDER
            or payload.get("model") != MODEL
            or record.get("model") != MODEL
        ):
            errors.append(f"request {index} model mismatch")
        candidate = _candidate_by_key(manifest).get((str(item.get("task_id")), str(item.get("candidate"))))
        if candidate is None:
            errors.append(f"request {index} candidate metadata missing")
        else:
            expected_prompt_hash = (candidate.get("prompt_sha256") or {}).get(str(item.get("condition")))
            if expected_prompt_hash != record.get("prompt_sha256"):
                errors.append(f"request {index} candidate prompt hash mismatch")
    if set(by_index) != set(range(len(expected_schedule or []))):
        errors.append("requests bundle does not cover the complete schedule")
    return [dict(record) for _, record in sorted(by_index.items())]


def audit(output: Path, repo_root: Path | None = None) -> dict[str, Any]:
    """Independently verify a prepared/analyzed artifact without networking."""

    output = output.resolve()
    repo_root = (repo_root or _repo_root()).resolve()
    report: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "audit_kind": "deepseek_repository_completion_integrity",
        "valid": False,
        "errors": [],
    }
    errors = report["errors"]
    try:
        manifest = read_object(output / "manifest.json", "manifest")
        freeze = read_object(output / "freeze.json", "freeze")
        smoke = read_object(output / "smoke.json", "smoke")
        results_root = read_object(output / "results.json", "results")
        summary = read_object(output / "summary.json", "summary")
        analysis = read_object(output / "analysis.json", "analysis")
    except (DeepSeekProbeError, OSError) as exc:
        errors.append(str(exc))
        return report
    schedule = manifest.get("schedule")
    if not isinstance(schedule, list) or len(schedule) != EXPECTED_CALLS:
        errors.append("manifest schedule must contain 144 records")
        schedule = schedule if isinstance(schedule, list) else []
    if manifest.get("model") != MODEL or freeze.get("model") != MODEL:
        errors.append("model is not frozen to deepseek-v4-pro")
    runner_source = repo_root / "crates/evaluation-runtime/src/bin/repository_completion_remote.rs"
    if (
        manifest.get("runner_source")
        != "crates/evaluation-runtime/src/bin/repository_completion_remote.rs"
        or freeze.get("runner_source") != manifest.get("runner_source")
        or not runner_source.is_file()
        or manifest.get("runner_source_sha256") != file_hash(runner_source)
        or freeze.get("runner_source_sha256") != manifest.get("runner_source_sha256")
    ):
        errors.append("runner source hash mismatch")
    _compare_json(manifest.get("schedule_sha256"), json_hash(schedule), "manifest schedule hash", errors)
    _compare_json(freeze.get("schedule"), schedule, "freeze schedule", errors)
    _compare_json(freeze.get("schedule_sha256"), json_hash(schedule), "freeze schedule hash", errors)
    _compare_json(
        freeze.get("candidate_preflight_sha256"),
        manifest.get("candidate_preflight_sha256"),
        "freeze candidate preflight hash reference",
        errors,
    )
    _compare_json(
        freeze.get("dependency_hashes"),
        manifest.get("dependency_hashes"),
        "freeze dependency hash reference",
        errors,
    )
    _compare_json(
        freeze.get("requests_bundle"),
        manifest.get("requests_bundle"),
        "freeze request bundle reference",
        errors,
    )
    if freeze.get("created_before_model_calls") is not True:
        errors.append("freeze does not prove pre-call creation")
    if manifest.get("freeze", {}).get("sha256") != file_hash(output / "freeze.json"):
        errors.append("freeze artifact hash mismatch")
    if manifest.get("smoke") != smoke:
        errors.append("manifest smoke mismatch")
    if smoke.get("go") is not True or manifest.get("preflight_ok") is not True:
        errors.append("preflight smoke is not passing")
    candidate_records = manifest.get("candidate_preflight")
    if not isinstance(candidate_records, list) or len(candidate_records) != EXPECTED_CANDIDATES:
        errors.append("candidate preflight must contain 36 records")
        candidate_records = candidate_records if isinstance(candidate_records, list) else []
    _compare_json(
        manifest.get("candidate_preflight_sha256"), json_hash(candidate_records), "candidate preflight hash", errors
    )
    _compare_json(
        freeze.get("candidate_preflight_sha256"),
        json_hash(candidate_records),
        "freeze candidate preflight hash",
        errors,
    )
    _audit_bundle(output, manifest, errors)
    for raw_path, expected_hash in (manifest.get("dependency_hashes") or {}).items():
        path = Path(str(raw_path))
        if not path.is_absolute():
            path = repo_root / path
        if not path.is_file() or file_hash(path) != expected_hash:
            errors.append(f"dependency hash mismatch: {raw_path}")
    try:
        event_ref = manifest.get("runner_events")
        if not isinstance(event_ref, Mapping) or not event_ref.get("path"):
            errors.append("runner event provenance is missing")
            events: list[dict[str, Any]] = []
        else:
            event_path = Path(str(event_ref["path"]))
            if not event_path.is_absolute():
                event_path = output / event_path
            if not event_path.is_file():
                errors.append("runner events are missing")
                events = []
            else:
                if event_ref.get("sha256") != file_hash(event_path):
                    errors.append("runner events hash mismatch")
                events = read_jsonl(event_path, "runner events")
        recomputed_rows, event_errors = _rows_from_events(manifest, events)
        if event_errors:
            errors.extend(f"event assignment: {error}" for error in event_errors)
        raw_results = results_root.get("results")
        if not isinstance(raw_results, list):
            errors.append("results.results must be an array")
            raw_results = []
        _compare_json(raw_results, recomputed_rows, "results recomputation", errors)
        recomputed_summary = summarize(manifest, recomputed_rows, event_errors)
        _compare_json(summary, recomputed_summary, "summary recomputation", errors)
        _compare_json(analysis, analysis_document(recomputed_summary), "analysis recomputation", errors)
    except (DeepSeekProbeError, OSError, TypeError, ValueError) as exc:
        errors.append(f"recomputation failed: {exc}")
    artifact_hashes = manifest.get("analysis_artifacts")
    if isinstance(artifact_hashes, Mapping):
        for filename, key in (("results.json", "results_sha256"), ("summary.json", "summary_sha256"), ("analysis.json", "analysis_sha256")):
            path = output / filename
            if not path.is_file() or file_hash(path) != artifact_hashes.get(key):
                errors.append(f"{filename} hash mismatch")
    else:
        errors.append("analysis artifact hashes are missing")
    report["valid"] = not errors
    report["schedule_complete"] = len(schedule) == EXPECTED_CALLS
    return report


# Descriptive aliases keep the three lifecycle stages discoverable to callers
# that prefer verb-noun names, without introducing a second implementation.
prepare_artifact = prepare
analyze_events = analyze
audit_artifacts = audit


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    prepare_parser = subparsers.add_parser("prepare")
    prepare_parser.add_argument("--out", required=True, type=Path)
    prepare_parser.add_argument("--dataset", type=Path, default=base.DEFAULT_DATASET)
    prepare_parser.add_argument("--fixture", type=Path)
    analyze_parser = subparsers.add_parser("analyze")
    analyze_parser.add_argument("--out", required=True, type=Path)
    analyze_parser.add_argument("--events", type=Path)
    audit_parser = subparsers.add_parser("audit")
    audit_parser.add_argument("--out", required=True, type=Path)
    audit_parser.add_argument("--repo-root", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare":
            report = prepare(args.out, dataset_path=args.dataset, fixture=args.fixture)
            print(json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True))
            return 0 if report["preflight_ok"] else 1
        if args.command == "analyze":
            report = analyze(args.out, args.events)
            print(json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True))
            return 0 if report["status"] == "complete" else 1
        report = audit(args.out, args.repo_root)
        print(json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True))
        return 0 if report["valid"] else 1
    except (DeepSeekProbeError, OSError, ValueError, KeyError) as exc:
        print(f"deepseek repository completion probe failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
