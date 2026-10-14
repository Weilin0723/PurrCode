#!/usr/bin/env python3
"""Analyze the frozen 2x2 requirement-revision study.

The primary contrast is C minus B in the correction variant.  Static and
correction rows are both retained so the revision-specific interaction can be
reported without treating repeated runs or tasks as iid observations.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import random
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


VALID_TERMINALS = frozenset(("valid", "error", "timeout", "invalid", "missing", "uncertain"))
ORACLE_STATUSES = frozenset(("correct", "incorrect", "unknown"))
PRIMARY_ARMS = ("B", "C")
REQUEST_VARIANTS = ("static", "correction")
REPEAT_LABELS = (0,)
RUNS_PER_MODEL = 120
SESSION_TOKEN_BUDGET = 1_000_000
PRIMARY_VARIANT = "correction"
PRIMARY_ESTIMAND = "repository_average_correction_c_minus_b_all_scheduled_risk_difference"
INTERACTION_ESTIMAND = "repository_average_revision_specific_difference_in_differences"
TRIGGER_POLICY = {
    "event": "ExecutionFinished",
    "success_exit_code": 0,
    "requires_nonempty_affected_paths": True,
    "acknowledgement_event": "requirement_revision_ack",
    "acknowledgement_required": True,
    "undelivered_policy": "retain_itt",
}


class AnalysisError(ValueError):
    pass


def _required(row: Mapping[str, Any], key: str, where: str) -> Any:
    if key not in row:
        raise AnalysisError(f"{where}: missing {key!r}")
    return row[key]


def _name(value: Any, where: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise AnalysisError(f"{where}: expected a non-empty string")
    return value


def _digest(value: Any, where: str) -> str:
    value = _name(value, where)
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise AnalysisError(f"{where}: expected a lowercase SHA-256 digest")
    return value


def _hash_map(value: Any, task_ids: set[str], where: str) -> dict[str, str]:
    if not isinstance(value, Mapping) or set(value) != task_ids:
        raise AnalysisError(f"{where}: expected one SHA-256 digest for every frozen task")
    return {
        task: _digest(digest, f"{where}.{task}")
        for task, digest in sorted(value.items())
    }


def _validate_trigger_policy(value: Any, where: str) -> dict[str, Any]:
    if not isinstance(value, Mapping) or dict(value) != TRIGGER_POLICY:
        raise AnalysisError(
            f"{where}: expected the frozen ExecutionFinished/non-empty-path trigger policy"
        )
    return dict(TRIGGER_POLICY)


def _validate_requirement_revision(
    value: Any, task_ids: set[str], where: str
) -> dict[str, Any]:
    if not isinstance(value, Mapping):
        raise AnalysisError(f"{where}: missing requirement-revision freeze")
    p0 = _hash_map(value.get("p0_sha256"), task_ids, f"{where}.p0_sha256")
    p1 = _hash_map(value.get("p1_sha256"), task_ids, f"{where}.p1_sha256")
    trigger = _validate_trigger_policy(value.get("trigger_policy"), f"{where}.trigger_policy")
    expected_digests = {
        "p0_hashes_sha256": canonical_hash(p0),
        "p1_hashes_sha256": canonical_hash(p1),
        "trigger_policy_sha256": canonical_hash(trigger),
    }
    for field, expected in expected_digests.items():
        if value.get(field) != expected:
            raise AnalysisError(f"{where}.{field}: digest does not bind the frozen metadata")
    return {
        "p0_sha256": p0,
        "p1_sha256": p1,
        "trigger_policy": trigger,
        **expected_digests,
    }


def validate_rows(
    values: Sequence[Mapping[str, Any]], *, source: str = "<results>"
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    seen: set[str] = set()
    seen_cells: set[tuple[str, str, str, str, int]] = set()
    for index, raw in enumerate(values, 1):
        where = f"{source}:{index}"
        if not isinstance(raw, Mapping):
            raise AnalysisError(f"{where}: expected an object")
        row = dict(raw)
        run_id = _name(_required(row, "run_id", where), f"{where}.run_id")
        if run_id in seen:
            raise AnalysisError(f"{where}: duplicate run_id {run_id!r}")
        seen.add(run_id)
        for field in ("repository", "base_task_id", "variant", "arm", "model"):
            row[field] = _name(_required(row, field, where), f"{where}.{field}")
        if row["arm"] not in PRIMARY_ARMS:
            raise AnalysisError(f"{where}.arm: expected B or C")
        if row["variant"] not in REQUEST_VARIANTS:
            raise AnalysisError(
                f"{where}.variant: expected static or correction request variant"
            )
        repeat = _required(row, "repeat", where)
        if isinstance(repeat, bool) or not isinstance(repeat, int) or repeat < 0:
            raise AnalysisError(f"{where}.repeat: expected a non-negative integer")
        terminal = _name(_required(row, "terminal_status", where), f"{where}.terminal_status")
        if terminal not in VALID_TERMINALS:
            raise AnalysisError(f"{where}.terminal_status: unsupported value {terminal!r}")
        oracle_status = _name(
            _required(row, "oracle_status", where), f"{where}.oracle_status"
        )
        if oracle_status not in ORACLE_STATUSES:
            raise AnalysisError(f"{where}.oracle_status: unsupported value {oracle_status!r}")
        claimed = _required(row, "claimed_complete", where)
        if claimed is not None and not isinstance(claimed, bool):
            raise AnalysisError(f"{where}.claimed_complete: expected boolean or null")
        if terminal == "valid" and not isinstance(claimed, bool):
            raise AnalysisError(f"{where}.claimed_complete: valid rows require a boolean claim")
        if terminal != "valid" and claimed is not None:
            raise AnalysisError(f"{where}.claimed_complete: non-valid rows require null")
        row["study_id"] = _name(_required(row, "study_id", where), f"{where}.study_id")
        row["plan_sha256"] = _digest(
            _required(row, "plan_sha256", where), f"{where}.plan_sha256"
        )
        for field in ("freeze_sha256", "dataset_manifest_sha256", "schedule_sha256"):
            row[field] = _digest(_required(row, field, where), f"{where}.{field}")
        requested = _required(row, "mid_run_correction_requested", where)
        triggered = _required(row, "mid_run_correction_triggered", where)
        acknowledged = _required(row, "mid_run_correction_acknowledged", where)
        if not all(isinstance(value, bool) for value in (requested, triggered, acknowledged)):
            raise AnalysisError(f"{where}: correction lifecycle fields must be booleans")
        if row["variant"] == "static" and (requested or triggered or acknowledged):
            raise AnalysisError(f"{where}: static rows cannot contain correction lifecycle events")
        if row["variant"] == "correction" and not requested:
            raise AnalysisError(f"{where}: correction rows must request P1 delivery")
        if triggered and not requested:
            raise AnalysisError(f"{where}: correction trigger without a requested correction")
        if acknowledged and not triggered:
            raise AnalysisError(f"{where}: correction acknowledgement precedes its trigger")
        for field in (
            "mid_run_correction_trigger_event_sha256",
            "mid_run_correction_message_event_sha256",
        ):
            digest = row.get(field)
            if digest is not None:
                row[field] = _digest(digest, f"{where}.{field}")
        if triggered and row.get("mid_run_correction_trigger_event_sha256") is None:
            raise AnalysisError(f"{where}: triggered correction lacks trigger evidence")
        if acknowledged and row.get("mid_run_correction_message_event_sha256") is None:
            raise AnalysisError(f"{where}: acknowledged correction lacks message evidence")
        if not triggered and row.get("mid_run_correction_trigger_event_sha256") is not None:
            raise AnalysisError(f"{where}: untriggered correction has trigger evidence")
        if not acknowledged and row.get("mid_run_correction_message_event_sha256") is not None:
            raise AnalysisError(f"{where}: unacknowledged correction has message evidence")
        cell = (
            row["base_task_id"],
            row["variant"],
            row["arm"],
            row["model"],
            repeat,
        )
        if cell in seen_cells:
            raise AnalysisError(f"{where}: duplicate scheduled cell {cell!r}")
        seen_cells.add(cell)
        rows.append(row)
    if not rows:
        raise AnalysisError(f"{source}: no rows")
    return rows


def load_rows(path: Path) -> list[dict[str, Any]]:
    values: list[Mapping[str, Any]] = []
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise AnalysisError(f"{path}: {error}") from error
    for index, line in enumerate(lines, 1):
        where = f"{path}:{index}"
        if not line.strip():
            raise AnalysisError(f"{where}: blank JSONL row")
        try:
            raw = json.loads(line)
        except json.JSONDecodeError as error:
            raise AnalysisError(f"{where}: invalid JSON: {error.msg}") from error
        if not isinstance(raw, dict):
            raise AnalysisError(f"{where}: expected an object")
        values.append(raw)
    return validate_rows(values, source=str(path))


def canonical_hash(value: Any) -> str:
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(encoded.encode()).hexdigest()


def file_hash(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise AnalysisError(f"{path}: {error}") from error


def load_plan(path: Path) -> dict[str, Any]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise AnalysisError(f"{path}: invalid JSON: {error.msg}") from error
    except OSError as error:
        raise AnalysisError(f"{path}: {error}") from error
    return load_plan_object(raw, source=str(path))


def load_plan_object(raw: Any, *, source: str = "<plan>") -> dict[str, Any]:
    if not isinstance(raw, dict) or raw.get("schema_version") != 1:
        raise AnalysisError(f"{source}: expected schema_version=1 object")
    for field in (
        "study_id",
        "primary_model",
        "requested_model",
        "primary_variant",
        "dataset_revision",
    ):
        _name(_required(raw, field, source), f"{source}.{field}")
    for field in (
        "dataset_manifest_sha256",
        "schedule_sha256",
        "qualification_sha256",
        "model_freeze_sha256",
    ):
        _digest(_required(raw, field, source), f"{source}.{field}")
    repositories = _required(raw, "repositories", source)
    if not isinstance(repositories, dict) or len(repositories) != 10:
        raise AnalysisError(f"{source}.repositories: exactly ten repository clusters are required")
    tasks: set[str] = set()
    for repository, values in repositories.items():
        _name(repository, f"{source}.repositories key")
        if not isinstance(values, list) or len(values) != 3:
            raise AnalysisError(
                f"{source}.repositories.{repository}: exactly three tasks are required"
            )
        for value in values:
            task = _name(value, f"{source}.repositories.{repository} task")
            if task in tasks:
                raise AnalysisError(f"{source}: task {task!r} appears more than once")
            tasks.add(task)
    if len(tasks) != 30:
        raise AnalysisError(f"{source}.repositories: confirmatory plan requires exactly 30 tasks")
    arms = _required(raw, "arms", source)
    if arms != list(PRIMARY_ARMS):
        raise AnalysisError(f"{source}.arms: confirmatory plan requires ['B', 'C']")
    request_variants = _required(raw, "request_variants", source)
    if request_variants != list(REQUEST_VARIANTS):
        raise AnalysisError(
            f"{source}.request_variants: confirmatory plan requires ['static', 'correction']"
        )
    repeats = _required(raw, "repeats", source)
    if (
        not isinstance(repeats, list)
        or repeats != list(REPEAT_LABELS)
        or any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in repeats)
        or len(set(repeats)) != len(repeats)
    ):
        raise AnalysisError(f"{source}.repeats: confirmatory plan requires [0]")
    runs_per_model = _required(raw, "runs_per_model", source)
    if runs_per_model != RUNS_PER_MODEL:
        raise AnalysisError(f"{source}.runs_per_model: confirmatory plan requires 120")
    scheduled_runs = _required(raw, "scheduled_runs_per_model", source)
    if scheduled_runs != RUNS_PER_MODEL:
        raise AnalysisError(f"{source}.scheduled_runs_per_model: confirmatory plan requires 120")
    if raw["requested_model"] != raw["primary_model"]:
        raise AnalysisError(f"{source}.requested_model: must match primary_model")
    session_token_budget = _required(raw, "session_token_budget", source)
    if (
        isinstance(session_token_budget, bool)
        or not isinstance(session_token_budget, int)
        or session_token_budget != SESSION_TOKEN_BUDGET
    ):
        raise AnalysisError(
            f"{source}.session_token_budget: confirmatory protocol requires {SESSION_TOKEN_BUDGET}"
        )
    if raw.get("primary_variant") != PRIMARY_VARIANT:
        raise AnalysisError(f"{source}.primary_variant: confirmatory plan requires correction")
    for field in ("bootstrap_samples", "bootstrap_seed"):
        value = _required(raw, field, source)
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise AnalysisError(f"{source}.{field}: expected a non-negative integer")
    if raw["bootstrap_samples"] < 10_000:
        raise AnalysisError(
            f"{source}.bootstrap_samples: confirmatory analysis requires at least 10000"
        )
    for field in (
        "alpha",
        "guardrail_margin",
        "attrition_limit",
        "minimum_baseline_correct_delivery_risk",
        "minimum_method_correct_delivery_risk",
    ):
        value = _required(raw, field, source)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not 0 < value < 1:
            raise AnalysisError(
                f"{source}.{field}: expected a number strictly between zero and one"
            )
    protocol_constants = {
        "alpha": 0.05,
        "guardrail_margin": 0.05,
        "attrition_limit": 0.10,
        "minimum_baseline_correct_delivery_risk": 0.10,
        "minimum_method_correct_delivery_risk": 0.10,
    }
    for field, expected in protocol_constants.items():
        if not math.isclose(float(raw[field]), expected, abs_tol=1e-12):
            raise AnalysisError(f"{source}.{field}: confirmatory protocol requires {expected}")
    if raw.get("primary_estimand") != PRIMARY_ESTIMAND:
        raise AnalysisError(f"{source}.primary_estimand: unsupported confirmatory estimand")
    if raw.get("interaction_estimand") != INTERACTION_ESTIMAND:
        raise AnalysisError(f"{source}.interaction_estimand: unsupported confirmatory estimand")
    if raw.get("attrition_definition") != "terminal_failure_or_oracle_unknown":
        raise AnalysisError(
            f"{source}.attrition_definition: expected terminal_failure_or_oracle_unknown"
        )
    _validate_requirement_revision(raw.get("requirement_revision"), tasks, f"{source}.requirement_revision")
    return dict(raw)


def load_freeze(path: Path) -> dict[str, Any]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise AnalysisError(f"{path}: invalid JSON: {error.msg}") from error
    except OSError as error:
        raise AnalysisError(f"{path}: {error}") from error
    return load_freeze_object(raw, source=str(path))


def load_freeze_object(raw: Any, *, source: str = "<freeze>") -> dict[str, Any]:
    if not isinstance(raw, dict) or raw.get("schema_version") != 1:
        raise AnalysisError(f"{source}: expected schema_version=1 object")
    for field in (
        "study_id",
        "frozen_at",
        "dataset_revision",
        "primary_model",
        "requested_model",
    ):
        _name(_required(raw, field, source), f"{source}.{field}")
    if raw["requested_model"] != raw["primary_model"]:
        raise AnalysisError(f"{source}.requested_model: must match primary_model")
    for field, expected in (
        ("scheduled_runs_per_model", RUNS_PER_MODEL),
        ("session_token_budget", SESSION_TOKEN_BUDGET),
    ):
        value = _required(raw, field, source)
        if isinstance(value, bool) or not isinstance(value, int) or value != expected:
            raise AnalysisError(f"{source}.{field}: expected {expected}")
    for field in (
        "analysis_plan_sha256",
        "dataset_manifest_sha256",
        "schedule_sha256",
        "p0_hashes_sha256",
        "p1_hashes_sha256",
        "trigger_policy_sha256",
        "qualification_sha256",
        "model_freeze_sha256",
    ):
        _digest(_required(raw, field, source), f"{source}.{field}")
    artifacts = _required(raw, "artifact_sha256", source)
    if not isinstance(artifacts, Mapping) or set(artifacts) != {
        "analysis_script",
        "runner",
        "dependency_lock",
        "qualification",
        "model_freeze",
    }:
        raise AnalysisError(
            f"{source}.artifact_sha256: expected analysis_script, runner, dependency_lock, qualification, and model_freeze"
        )
    for name, digest in artifacts.items():
        _digest(digest, f"{source}.artifact_sha256.{name}")
    return dict(raw)


def _indicator(row: Mapping[str, Any], endpoint: str) -> float:
    if row["terminal_status"] != "valid" or row["claimed_complete"] is not True:
        return 0.0
    if endpoint == "false_ready":
        return float(row["oracle_status"] in {"incorrect", "unknown"})
    if endpoint == "correct_delivery":
        return float(row["oracle_status"] == "correct")
    raise AssertionError(endpoint)


def _mean(values: Iterable[float]) -> float:
    values = list(values)
    if not values:
        raise AnalysisError("cannot average an empty group")
    return sum(values) / len(values)


def _task_differences(rows: Sequence[Mapping[str, Any]], endpoint: str) -> dict[str, dict[str, float]]:
    cells: dict[tuple[str, str], list[float]] = defaultdict(list)
    repeats: dict[tuple[str, str], set[int]] = defaultdict(set)
    repositories: dict[str, str] = {}
    for row in rows:
        task = str(row["base_task_id"])
        repository = str(row["repository"])
        previous = repositories.setdefault(task, repository)
        if previous != repository:
            raise AnalysisError(f"task {task!r} belongs to multiple repositories")
        cells[(task, str(row["arm"]))].append(_indicator(row, endpoint))
        repeats[(task, str(row["arm"]))].add(int(row["repeat"]))

    result: dict[str, dict[str, float]] = {}
    for task in sorted(repositories):
        missing = [arm for arm in PRIMARY_ARMS if (task, arm) not in cells]
        if missing:
            raise AnalysisError(f"task {task!r} is missing arm(s): {', '.join(missing)}")
        if repeats[(task, "B")] != repeats[(task, "C")]:
            raise AnalysisError(f"task {task!r} has unmatched repeat labels across B and C")
        b = _mean(cells[(task, "B")])
        c = _mean(cells[(task, "C")])
        result[task] = {"repository": repositories[task], "B": b, "C": c, "difference": c - b}
    return result


def _difference_in_differences(
    static_tasks: Mapping[str, Mapping[str, Any]],
    correction_tasks: Mapping[str, Mapping[str, Any]],
) -> dict[str, dict[str, float]]:
    if set(static_tasks) != set(correction_tasks):
        raise AnalysisError("static and correction schedules do not contain the same tasks")
    result: dict[str, dict[str, float]] = {}
    for task in sorted(static_tasks):
        static = static_tasks[task]
        correction = correction_tasks[task]
        if static["repository"] != correction["repository"]:
            raise AnalysisError(f"task {task!r} changes repository between request variants")
        static_difference = float(static["difference"])
        correction_difference = float(correction["difference"])
        result[task] = {
            "repository": static["repository"],
            "static_difference": static_difference,
            "correction_difference": correction_difference,
            "difference": correction_difference - static_difference,
        }
    return result


def _repo_differences(tasks: Mapping[str, Mapping[str, Any]]) -> dict[str, float]:
    grouped: dict[str, list[float]] = defaultdict(list)
    for task in tasks.values():
        grouped[str(task["repository"])].append(float(task["difference"]))
    return {repository: _mean(values) for repository, values in sorted(grouped.items())}


def exact_two_sided_sign_p(differences: Iterable[float]) -> float | None:
    nonzero = [value for value in differences if not math.isclose(value, 0.0, abs_tol=1e-12)]
    if not nonzero:
        return None
    wins = sum(value < 0 for value in nonzero)
    tail = min(wins, len(nonzero) - wins)
    numerator = sum(math.comb(len(nonzero), k) for k in range(tail + 1))
    return min(1.0, 2.0 * numerator / (2**len(nonzero)))


def _percentile(values: Sequence[float], probability: float) -> float:
    ordered = sorted(values)
    position = (len(ordered) - 1) * probability
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    weight = position - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def repository_cluster_bootstrap(
    tasks: Mapping[str, Mapping[str, Any]], *, samples: int, seed: int
) -> list[float]:
    if samples < 1:
        raise AnalysisError("bootstrap samples must be positive")
    repositories = _repo_differences(tasks)
    if len(repositories) < 2:
        raise AnalysisError("at least two repository clusters are required")
    generator = random.Random(seed)
    draws: list[float] = []
    for _ in range(samples):
        sampled = [generator.choice(list(repositories)) for _ in repositories]
        draws.append(_mean(repositories[repository] for repository in sampled))
    return draws


def _attrition(rows: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for variant in REQUEST_VARIANTS:
        for arm in PRIMARY_ARMS:
            cell = f"{variant}:{arm}"
            arm_rows = [
                row for row in rows if row["variant"] == variant and row["arm"] == arm
            ]
            if not arm_rows:
                raise AnalysisError(f"missing scheduled attrition cell {cell}")
            terminal_counts = {
                status: sum(row["terminal_status"] == status for row in arm_rows)
                for status in sorted(VALID_TERMINALS)
            }
            oracle_unknown = sum(row["oracle_status"] == "unknown" for row in arm_rows)
            trigger_failures = sum(
                row["variant"] == "correction"
                and not row["mid_run_correction_triggered"]
                for row in arm_rows
            )
            acknowledgement_failures = sum(
                row["variant"] == "correction"
                and row["mid_run_correction_triggered"]
                and not row["mid_run_correction_acknowledged"]
                for row in arm_rows
            )
            missing_claim = sum(
                row["terminal_status"] == "valid" and row["claimed_complete"] is None
                for row in arm_rows
            )
            claim_not_applicable = sum(
                row["terminal_status"] != "valid" and row["claimed_complete"] is None
                for row in arm_rows
            )
            failures = sum(
                row["terminal_status"] != "valid"
                or row["oracle_status"] == "unknown"
                or (
                    row["variant"] == "correction"
                    and (
                        not row["mid_run_correction_triggered"]
                        or not row["mid_run_correction_acknowledged"]
                    )
                )
                for row in arm_rows
            )
            result[cell] = {
                "failed": failures,
                "scheduled": len(arm_rows),
                "rate": failures / len(arm_rows),
                "terminal_failure_rate": sum(
                    row["terminal_status"] != "valid" for row in arm_rows
                )
                / len(arm_rows),
                "oracle_unknown_rate": oracle_unknown / len(arm_rows),
                "terminal_status_counts": terminal_counts,
                "oracle_unknown": oracle_unknown,
                "trigger_failures": trigger_failures,
                "acknowledgement_failures": acknowledgement_failures,
                "missing_claim": missing_claim,
                "claim_not_applicable": claim_not_applicable,
                "formula": "terminal failure OR oracle unknown OR correction trigger/ack failure",
            }
    # Keep a compact arm-level view for readers while decisions use every 2x2 cell.
    for arm in PRIMARY_ARMS:
        cells = [result[f"{variant}:{arm}"] for variant in REQUEST_VARIANTS]
        result[arm] = {
            "failed": sum(cell["failed"] for cell in cells),
            "scheduled": sum(cell["scheduled"] for cell in cells),
            "rate": sum(cell["failed"] for cell in cells) / sum(cell["scheduled"] for cell in cells),
            "by_variant": {
                variant: result[f"{variant}:{arm}"] for variant in REQUEST_VARIANTS
            },
        }
    return result


def _expected_cells(plan: Mapping[str, Any]) -> set[tuple[str, str, str, str, int]]:
    return {
        (task, variant, arm, str(plan["primary_model"]), repeat)
        for tasks in plan["repositories"].values()
        for task in tasks
        for variant in plan["request_variants"]
        for arm in PRIMARY_ARMS
        for repeat in plan["repeats"]
    }


def schedule_payload(plan: Mapping[str, Any]) -> list[dict[str, Any]]:
    return [
        {
            "base_task_id": task,
            "variant": variant,
            "arm": arm,
            "model": str(plan["primary_model"]),
            "repeat": repeat,
            "repository": repository,
        }
        for repository, tasks in sorted(plan["repositories"].items())
        for task in sorted(tasks)
        for variant in plan["request_variants"]
        for arm in PRIMARY_ARMS
        for repeat in plan["repeats"]
    ]


def _endpoint_report(
    tasks: Mapping[str, Mapping[str, Any]], *, samples: int, seed: int, alpha: float
) -> dict[str, Any]:
    repositories = _repo_differences(tasks)
    draws = repository_cluster_bootstrap(tasks, samples=samples, seed=seed)
    return {
        "B": _mean(
            _mean(task["B"] for task in tasks.values() if task["repository"] == repo)
            for repo in repositories
        ),
        "C": _mean(
            _mean(task["C"] for task in tasks.values() if task["repository"] == repo)
            for repo in repositories
        ),
        "difference": _mean(repositories.values()),
        "repository_cluster_bootstrap_interval": [
            _percentile(draws, alpha / 2.0),
            _percentile(draws, 1.0 - alpha / 2.0),
        ],
        "repository_cluster_bootstrap_confidence": 1.0 - alpha,
        "repository_cluster_bootstrap_one_sided_lower": _percentile(draws, alpha),
        "repository_sign_test_p_two_sided": exact_two_sided_sign_p(repositories.values()),
        "task_differences": tasks,
        "repository_differences": repositories,
    }


def _interaction_report(
    tasks: Mapping[str, Mapping[str, Any]], *, samples: int, seed: int, alpha: float
) -> dict[str, Any]:
    repositories = _repo_differences(tasks)
    draws = repository_cluster_bootstrap(tasks, samples=samples, seed=seed)
    return {
        "difference": _mean(repositories.values()),
        "repository_cluster_bootstrap_interval": [
            _percentile(draws, alpha / 2.0),
            _percentile(draws, 1.0 - alpha / 2.0),
        ],
        "repository_cluster_bootstrap_confidence": 1.0 - alpha,
        "repository_cluster_bootstrap_one_sided_lower": _percentile(draws, alpha),
        "repository_sign_test_p_two_sided": exact_two_sided_sign_p(repositories.values()),
        "task_differences": tasks,
        "repository_differences": repositories,
    }


def analyze(
    rows: Sequence[Mapping[str, Any]],
    plan: Mapping[str, Any],
    freeze: Mapping[str, Any],
    dataset_manifest: Mapping[str, Any],
    artifact_paths: Mapping[str, Path],
    *,
    expected_freeze_sha256: str,
) -> dict[str, Any]:
    plan = load_plan_object(dict(plan))
    freeze = load_freeze_object(dict(freeze))
    rows = validate_rows(rows)
    plan_sha256 = canonical_hash(plan)
    freeze_sha256 = canonical_hash(freeze)
    expected_freeze_sha256 = _digest(expected_freeze_sha256, "expected_freeze_sha256")
    if freeze_sha256 != expected_freeze_sha256:
        raise AnalysisError("freeze hash does not match the independently published digest")
    if freeze["study_id"] != plan["study_id"]:
        raise AnalysisError("freeze study_id does not match the analysis plan")
    if freeze["analysis_plan_sha256"] != plan_sha256:
        raise AnalysisError("analysis plan hash does not match the pre-run freeze")
    if canonical_hash(dataset_manifest) != plan["dataset_manifest_sha256"]:
        raise AnalysisError("dataset manifest content does not match its frozen digest")
    if canonical_hash(schedule_payload(plan)) != plan["schedule_sha256"]:
        raise AnalysisError("schedule digest is not derived from the frozen primary cells")
    requirement_revision = _validate_requirement_revision(
        plan["requirement_revision"],
        {
            task for tasks in plan["repositories"].values() for task in tasks
        },
        "analysis_plan.requirement_revision",
    )
    for field in (
        "dataset_revision",
        "dataset_manifest_sha256",
        "schedule_sha256",
        "primary_model",
        "requested_model",
        "scheduled_runs_per_model",
        "session_token_budget",
        "qualification_sha256",
        "model_freeze_sha256",
    ):
        if freeze[field] != plan[field]:
            raise AnalysisError(f"freeze {field} does not match the analysis plan")
    for field in (
        "p0_hashes_sha256",
        "p1_hashes_sha256",
        "trigger_policy_sha256",
    ):
        if freeze[field] != requirement_revision[field]:
            raise AnalysisError(f"freeze {field} does not match the frozen requirement revision")
    expected_artifact_names = {
        "analysis_script",
        "runner",
        "dependency_lock",
        "qualification",
        "model_freeze",
    }
    if set(artifact_paths) != expected_artifact_names:
        raise AnalysisError(
            "artifact paths must name analysis_script, runner, dependency_lock, qualification, and model_freeze"
        )
    artifact_hashes = {name: file_hash(Path(path)) for name, path in artifact_paths.items()}
    if artifact_hashes != freeze["artifact_sha256"]:
        raise AnalysisError("analysis artifacts do not match the frozen code/dependency digests")
    model = str(plan["primary_model"])
    variant = str(plan["primary_variant"])
    bootstrap_samples = int(plan["bootstrap_samples"])
    bootstrap_seed = int(plan["bootstrap_seed"])
    guardrail_margin = float(plan["guardrail_margin"])
    attrition_limit = float(plan["attrition_limit"])
    alpha = float(plan["alpha"])
    minimum_b_utility = float(plan["minimum_baseline_correct_delivery_risk"])
    minimum_c_utility = float(plan["minimum_method_correct_delivery_risk"])
    selected = [row for row in rows if row["model"] == model]
    actual_cells = {
        (row["base_task_id"], row["variant"], row["arm"], row["model"], row["repeat"])
        for row in selected
    }
    expected_cells = _expected_cells(plan)
    if actual_cells != expected_cells:
        missing = len(expected_cells - actual_cells)
        unexpected = len(actual_cells - expected_cells)
        raise AnalysisError(
            f"primary schedule mismatch: {missing} missing and {unexpected} unexpected cell(s)"
        )
    if len(selected) != RUNS_PER_MODEL:
        raise AnalysisError(
            f"primary schedule requires exactly {RUNS_PER_MODEL} rows, got {len(selected)}"
        )
    planned_repository_by_task = {
        task: repository for repository, tasks in plan["repositories"].items() for task in tasks
    }
    for row in selected:
        if (
            row["study_id"] != plan["study_id"]
            or row["plan_sha256"] != plan_sha256
            or row["freeze_sha256"] != freeze_sha256
            or row["dataset_manifest_sha256"] != plan["dataset_manifest_sha256"]
            or row["schedule_sha256"] != plan["schedule_sha256"]
        ):
            raise AnalysisError("result row is not bound to the frozen study artifacts")
        expected_repository = planned_repository_by_task.get(str(row["base_task_id"]))
        if row["repository"] != expected_repository:
            raise AnalysisError(
                f"task {row['base_task_id']!r} repository does not match the frozen plan"
            )
    attrition = _attrition(selected)
    operationally_eligible = all(
        attrition[f"{request_variant}:{arm}"]["rate"] <= attrition_limit
        for request_variant in REQUEST_VARIANTS
        for arm in PRIMARY_ARMS
    )

    endpoints: dict[str, Any] = {}
    for offset, endpoint in enumerate(("false_ready", "correct_delivery")):
        static_tasks = _task_differences(
            [row for row in selected if row["variant"] == "static"], endpoint
        )
        correction_tasks = _task_differences(
            [row for row in selected if row["variant"] == "correction"], endpoint
        )
        interaction_tasks = _difference_in_differences(static_tasks, correction_tasks)
        correction_report = _endpoint_report(
            correction_tasks,
            samples=bootstrap_samples,
            seed=bootstrap_seed + offset,
            alpha=alpha,
        )
        static_report = _endpoint_report(
            static_tasks,
            samples=bootstrap_samples,
            seed=bootstrap_seed + 10 + offset,
            alpha=alpha,
        )
        correction_variant_report = dict(correction_report)
        endpoints[endpoint] = correction_report
        endpoints[endpoint]["by_variant"] = {
            "static": static_report,
            "correction": correction_variant_report,
        }
        endpoints[endpoint]["interaction"] = _interaction_report(
            interaction_tasks,
            samples=bootstrap_samples,
            seed=bootstrap_seed + 20 + offset,
            alpha=alpha,
        )

    false_ready = endpoints["false_ready"]
    correct_delivery = endpoints["correct_delivery"]
    primary_supported = (
        operationally_eligible
        and false_ready["difference"] < 0
        and false_ready["repository_sign_test_p_two_sided"] is not None
        and false_ready["repository_sign_test_p_two_sided"] < alpha
        and false_ready["repository_cluster_bootstrap_interval"][1] < 0
    )
    guardrail_supported = (
        operationally_eligible
        and correct_delivery["difference"] >= -guardrail_margin
        and correct_delivery["repository_cluster_bootstrap_one_sided_lower"]
        >= -guardrail_margin
        and correct_delivery["B"] >= minimum_b_utility
        and correct_delivery["C"] >= minimum_c_utility
    )
    if not operationally_eligible:
        decision_status = "operationally_inconclusive"
    elif primary_supported and guardrail_supported:
        decision_status = "joint_method_supported"
    else:
        decision_status = "joint_method_not_supported"
    return {
        "schema_version": 1,
        "analysis_plan_sha256": plan_sha256,
        "freeze_sha256": freeze_sha256,
        "dataset_manifest_sha256": plan["dataset_manifest_sha256"],
        "schedule_sha256": plan["schedule_sha256"],
        "estimand": PRIMARY_ESTIMAND,
        "interaction_estimand": INTERACTION_ESTIMAND,
        "primary_model": model,
        "primary_variant": variant,
        "request_variants": list(REQUEST_VARIANTS),
        "runs_per_model": RUNS_PER_MODEL,
        "scheduled_runs_per_model": plan["scheduled_runs_per_model"],
        "session_token_budget": plan["session_token_budget"],
        "qualification_sha256": plan["qualification_sha256"],
        "model_freeze_sha256": plan["model_freeze_sha256"],
        "scheduled_rows": len(selected),
        "tasks": len(false_ready["task_differences"]),
        "repositories": len(false_ready["repository_differences"]),
        "attrition": attrition,
        "attrition_limit": attrition_limit,
        "alpha": alpha,
        "bootstrap": {
            "method": "equal-weight repository cluster percentile bootstrap",
            "samples": bootstrap_samples,
            "seed": bootstrap_seed,
        },
        "operationally_eligible": operationally_eligible,
        "requirement_revision": {
            "p0_hashes_sha256": requirement_revision["p0_hashes_sha256"],
            "p1_hashes_sha256": requirement_revision["p1_hashes_sha256"],
            "trigger_policy_sha256": requirement_revision["trigger_policy_sha256"],
            "trigger_policy": requirement_revision["trigger_policy"],
        },
        "endpoints": endpoints,
        "guardrail": {
            "endpoint": "correct_delivery",
            "noninferiority_margin": -guardrail_margin,
            "minimum_baseline_correct_delivery_risk": minimum_b_utility,
            "minimum_method_correct_delivery_risk": minimum_c_utility,
            "criterion": "observed difference and one-sided repository-cluster bootstrap lower bound are at least the margin, and both arms meet their frozen absolute utility floor",
            "passed": guardrail_supported,
        },
        "decision": {
            "status": decision_status,
            "false_ready_supported": primary_supported,
            "guardrail_supported": guardrail_supported,
            "joint_method_support": (
                primary_supported and guardrail_supported if operationally_eligible else None
            ),
            "inconclusive_if_ineligible": not operationally_eligible,
        },
        "notes": [
            "Repository is the equal-weight outer estimand, resampling unit, and sign-test cluster.",
            "The correction C-minus-B contrast is primary; static/correction interaction is revision-specific secondary evidence.",
            "Non-valid, untriggered, undelivered, and unacknowledged scheduled runs remain in ITT; attrition above the frozen limit suppresses support.",
            "Correct delivery is an all-scheduled risk, not post-treatment conditional coverage.",
        ],
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("results", type=Path)
    parser.add_argument("plan", type=Path)
    parser.add_argument("freeze", type=Path)
    parser.add_argument("dataset_manifest", type=Path)
    parser.add_argument("--runner-artifact", type=Path, required=True)
    parser.add_argument("--dependency-lock", type=Path, required=True)
    parser.add_argument("--qualification", type=Path, required=True)
    parser.add_argument("--model-freeze", type=Path, required=True)
    parser.add_argument(
        "--expected-freeze-sha256",
        required=True,
        help="digest published independently before scheduled run 0",
    )
    args = parser.parse_args(argv)
    try:
        dataset_manifest = json.loads(args.dataset_manifest.read_text(encoding="utf-8"))
        if not isinstance(dataset_manifest, dict):
            raise AnalysisError("dataset manifest must be a JSON object")
        result = analyze(
            load_rows(args.results),
            load_plan(args.plan),
            load_freeze(args.freeze),
            dataset_manifest,
            {
                "analysis_script": Path(__file__),
                "runner": args.runner_artifact,
                "dependency_lock": args.dependency_lock,
                "qualification": args.qualification,
                "model_freeze": args.model_freeze,
            },
            expected_freeze_sha256=args.expected_freeze_sha256,
        )
    except (AnalysisError, json.JSONDecodeError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
