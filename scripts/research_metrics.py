#!/usr/bin/env python3
"""Measure independent outcomes from existing AlignmentReport JSON files.

The external correctness signal is intentionally mechanical and narrow:
``expected_requirements > 0 and satisfied_requirements ==
expected_requirements and not violated_forbidden``.  This script does not
run a repository, infer behaviour, or turn a delivery gate into an oracle.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


SCHEMA_VERSION = 1
WILSON_Z = 1.959963984540054
VALID_KINDS = frozenset(("simple", "complex", "alignment_trap"))


class ReportError(ValueError):
    """Raised when an input is not a valid schema-version-one report."""


@dataclass(frozen=True)
class Outcome:
    task_id: str
    kind: str
    reported_done: bool
    delivery_state: str | None
    satisfied_requirements: int
    expected_requirements: int
    violated_forbidden: bool

    @property
    def externally_correct(self) -> bool:
        return (
            self.expected_requirements > 0
            and self.satisfied_requirements == self.expected_requirements
            and not self.violated_forbidden
        )


def _location(source: str, suffix: str) -> str:
    return f"{source}{suffix}"


def _object(value: Any, where: str) -> Mapping[str, Any]:
    if not isinstance(value, dict):
        raise ReportError(f"{where}: expected an object")
    return value


def _required(obj: Mapping[str, Any], key: str, where: str) -> Any:
    if key not in obj:
        raise ReportError(f"{where}: missing field {key!r}")
    return obj[key]


def _bool(value: Any, where: str) -> bool:
    if not isinstance(value, bool):
        raise ReportError(f"{where}: expected a boolean")
    return value


def _string(value: Any, where: str, *, nonempty: bool = False) -> str:
    if not isinstance(value, str):
        raise ReportError(f"{where}: expected a string")
    if nonempty and not value:
        raise ReportError(f"{where}: expected a non-empty string")
    return value


def _count(value: Any, where: str) -> int:
    # bool is an int subclass in Python, but is not a JSON count.
    if isinstance(value, bool) or not isinstance(value, int):
        raise ReportError(f"{where}: expected a non-negative integer")
    if value < 0:
        raise ReportError(f"{where}: expected a non-negative integer")
    return value


def _schema(value: Any, where: str) -> None:
    if isinstance(value, bool) or not isinstance(value, int) or value != SCHEMA_VERSION:
        raise ReportError(f"{where}: expected schema_version={SCHEMA_VERSION}")


RUN_COUNT_FIELDS = (
    "hard_requirements_total",
    "hard_requirements_verified",
    "hard_requirements_violated",
    "hard_requirements_unknown",
    "blocking_findings_at_completion",
    "scope_escapes",
    "corrections_issued",
    "corrections_reflected",
    "findings_repaired",
    "findings_raised",
    "correction_cycles",
    "review_tokens",
    "implementation_tokens",
    "elapsed_seconds",
)


def _parse_outcomes(data: Any, source: str) -> list[Outcome]:
    root = _object(data, source)
    _schema(_required(root, "schema_version", source), _location(source, ".schema_version"))
    raw_outcomes = _required(root, "outcomes", source)
    if not isinstance(raw_outcomes, list):
        raise ReportError(f"{source}.outcomes: expected an array")

    outcomes: list[Outcome] = []
    seen_ids: set[str] = set()
    for index, raw in enumerate(raw_outcomes):
        where = f"{source}.outcomes[{index}]"
        item = _object(raw, where)
        _schema(_required(item, "schema_version", where), f"{where}.schema_version")
        task_id = _string(_required(item, "task_id", where), f"{where}.task_id", nonempty=True)
        if task_id in seen_ids:
            raise ReportError(f"{where}.task_id: duplicate task id {task_id!r} in one report")
        seen_ids.add(task_id)
        kind = _string(_required(item, "kind", where), f"{where}.kind", nonempty=True)
        if kind not in VALID_KINDS:
            allowed = ", ".join(sorted(VALID_KINDS))
            raise ReportError(f"{where}.kind: expected one of {allowed}")

        run_where = f"{where}.run"
        run = _object(_required(item, "run", where), run_where)
        reported_done = _bool(
            _required(run, "reported_done", run_where), f"{run_where}.reported_done"
        )
        delivery_state_value = _required(run, "delivery_state", run_where)
        if delivery_state_value is not None and not isinstance(delivery_state_value, str):
            raise ReportError(f"{run_where}.delivery_state: expected a string or null")
        delivery_state = delivery_state_value
        for field in RUN_COUNT_FIELDS:
            _count(_required(run, field, run_where), f"{run_where}.{field}")

        satisfied = _count(
            _required(item, "satisfied_requirements", where),
            f"{where}.satisfied_requirements",
        )
        expected = _count(
            _required(item, "expected_requirements", where),
            f"{where}.expected_requirements",
        )
        if satisfied > expected:
            raise ReportError(
                f"{where}.satisfied_requirements: cannot exceed expected_requirements"
            )
        violated = _bool(
            _required(item, "violated_forbidden", where), f"{where}.violated_forbidden"
        )
        outcomes.append(
            Outcome(
                task_id=task_id,
                kind=kind,
                reported_done=reported_done,
                delivery_state=delivery_state,
                satisfied_requirements=satisfied,
                expected_requirements=expected,
                violated_forbidden=violated,
            )
        )
    return outcomes


def _wilson(successes: int, trials: int) -> dict[str, float] | None:
    if trials == 0:
        return None
    proportion = successes / trials
    z_squared = WILSON_Z * WILSON_Z
    denominator = 1.0 + z_squared / trials
    centre = (proportion + z_squared / (2.0 * trials)) / denominator
    half_width = (
        WILSON_Z
        * math.sqrt(
            proportion * (1.0 - proportion) / trials
            + z_squared / (4.0 * trials * trials)
        )
        / denominator
    )
    return {
        "lower": max(0.0, centre - half_width),
        "upper": min(1.0, centre + half_width),
    }


def _rate(successes: int, trials: int) -> dict[str, Any]:
    if successes < 0 or trials < 0 or successes > trials:
        raise ValueError("invalid rate counts")
    return {
        "successes": successes,
        "trials": trials,
        "rate": None if trials == 0 else successes / trials,
        "wilson95": _wilson(successes, trials),
    }


def _task_metrics(outcomes: Sequence[Outcome]) -> dict[str, Any]:
    scored = [outcome for outcome in outcomes if outcome.expected_requirements > 0]
    correct = [outcome for outcome in scored if outcome.externally_correct]
    claims = [outcome for outcome in scored if outcome.reported_done]
    false_completion = [outcome for outcome in claims if not outcome.externally_correct]
    correct_unclaimed = [outcome for outcome in correct if not outcome.reported_done]
    claimed_correct = [outcome for outcome in correct if outcome.reported_done]
    unscored_claims = [outcome for outcome in outcomes if outcome.reported_done and outcome.expected_requirements == 0]

    external_correctness = _rate(len(correct), len(scored))
    false_all = _rate(len(false_completion), len(scored))
    false_claimed = _rate(len(false_completion), len(claims))
    correct_unclaimed_all = _rate(len(correct_unclaimed), len(scored))
    correct_unclaimed_correct = _rate(len(correct_unclaimed), len(correct))
    correct_delivery = _rate(len(claimed_correct), len(scored))
    completion_coverage = _rate(len(claimed_correct), len(correct))
    claim_coverage = _rate(len(claims), len(scored))
    return {
        "tasks": len(outcomes),
        "scored_tasks": len(scored),
        "external_correct_tasks": len(correct),
        "claimed_tasks": len(claims),
        "false_completion_tasks": len(false_completion),
        "correct_unclaimed_tasks": len(correct_unclaimed),
        "claimed_correct_tasks": len(claimed_correct),
        "unscored_claims": len(unscored_claims),
        "external_correctness": external_correctness,
        "false_completion": {
            "count": len(false_completion),
            "rate_all_scored": false_all,
            "rate_claimed": false_claimed,
        },
        "correct_unclaimed": {
            "count": len(correct_unclaimed),
            "rate_all_scored": correct_unclaimed_all,
            "rate_external_correct": correct_unclaimed_correct,
        },
        "correct_delivery": correct_delivery,
        "completion_coverage": completion_coverage,
        "claim_coverage": claim_coverage,
    }


def _gate_consistency(outcomes: Sequence[Outcome]) -> dict[str, Any]:
    claimed = [outcome for outcome in outcomes if outcome.reported_done]
    ready = [outcome for outcome in outcomes if outcome.delivery_state == "ready"]
    claimed_and_ready = [
        outcome for outcome in outcomes if outcome.reported_done and outcome.delivery_state == "ready"
    ]
    claimed_without_ready = [
        outcome for outcome in outcomes if outcome.reported_done and outcome.delivery_state != "ready"
    ]
    ready_without_claim = [
        outcome for outcome in outcomes if not outcome.reported_done and outcome.delivery_state == "ready"
    ]
    consistent = [
        outcome for outcome in outcomes if outcome.reported_done == (outcome.delivery_state == "ready")
    ]
    states = Counter(
        "missing" if outcome.delivery_state is None else outcome.delivery_state
        for outcome in outcomes
    )
    return {
        "tasks": len(outcomes),
        "claimed_tasks": len(claimed),
        "ready_tasks": len(ready),
        "claimed_and_ready": len(claimed_and_ready),
        "claimed_without_ready": len(claimed_without_ready),
        "ready_without_claim": len(ready_without_claim),
        "missing_gate": sum(outcome.delivery_state is None for outcome in outcomes),
        "states": dict(sorted(states.items())),
        "consistency": _rate(len(consistent), len(outcomes)),
        "ready_is_not_external_truth": True,
    }


def analyze_report(data: Any, source: str = "<input>") -> dict[str, Any]:
    """Validate one report and return metrics without merging it with others."""

    outcomes = _parse_outcomes(data, source)
    kinds = sorted({outcome.kind for outcome in outcomes})
    return {
        "source": source,
        "schema_version": SCHEMA_VERSION,
        "overall": _task_metrics(outcomes),
        "by_kind": {
            kind: {
                "metrics": _task_metrics([outcome for outcome in outcomes if outcome.kind == kind]),
                "gate_consistency": _gate_consistency(
                    [outcome for outcome in outcomes if outcome.kind == kind]
                ),
            }
            for kind in kinds
        },
        "gate_consistency": _gate_consistency(outcomes),
    }


def analyze_file(path: str | Path) -> dict[str, Any]:
    path = Path(path)
    try:
        with path.open("r", encoding="utf-8") as handle:
            data = json.load(handle)
    except json.JSONDecodeError as exc:
        raise ReportError(f"{path}: invalid JSON: {exc.msg}") from exc
    except OSError as exc:
        raise ReportError(f"{path}: cannot read report: {exc}") from exc
    return analyze_report(data, str(path))


def analyze_files(paths: Iterable[str | Path]) -> dict[str, Any]:
    """Analyze each input independently; never pool task counts across files."""

    reports = [analyze_file(path) for path in paths]
    return {
        "schema_version": SCHEMA_VERSION,
        "meta": {
            "scope": "existing AlignmentReport mechanical outcomes",
            "external_correctness": "expected_requirements > 0 && satisfied_requirements == expected_requirements && !violated_forbidden",
            "completion_claim": "run.reported_done",
            "completion_measurement_limitation": "Existing reports use SessionStatus::Completed; user-visible completion claims are not measured.",
            "gate_ready_is_not_truth": True,
            "behavior_oracle": False,
            "wilson95": "Within each report and kind stratum; assumes task independence. Intervals are not valid for cross-file pooled samples.",
            "aggregation": "per-report only; input reports are never merged",
        },
        "reports": reports,
    }


def _format_rate(rate: Mapping[str, Any]) -> str:
    if rate["rate"] is None:
        return "n/a"
    return f"{rate['rate']:.1%}"


def render_text(result: Mapping[str, Any]) -> str:
    lines = [
        "research metrics (existing mechanical AlignmentReport outcomes)",
        "gate=ready is reported separately and is not used as external correctness truth",
    ]
    for report in result["reports"]:
        overall = report["overall"]
        lines.append(f"\n{report['source']}: {overall['tasks']} task(s), {overall['scored_tasks']} scored")
        lines.append(
            "  external correctness: "
            f"{_format_rate(overall['external_correctness'])} "
            f"({overall['external_correct_tasks']}/{overall['scored_tasks']})"
        )
        false = overall["false_completion"]
        lines.append(
            "  false completion: "
            f"{_format_rate(false['rate_all_scored'])} of all scored; "
            f"{_format_rate(false['rate_claimed'])} of claims"
        )
        lines.append(
            "  correct but unclaimed: "
            f"{overall['correct_unclaimed_tasks']} · completion coverage "
            f"{_format_rate(overall['completion_coverage'])}"
        )
        gate = report["gate_consistency"]
        lines.append(
            "  gate consistency: "
            f"{_format_rate(gate['consistency'])}; ready={gate['ready_tasks']}, "
            f"claimed_without_ready={gate['claimed_without_ready']}"
        )
        for kind, details in report["by_kind"].items():
            metrics = details["metrics"]
            lines.append(
                f"  kind={kind}: external correctness "
                f"{_format_rate(metrics['external_correctness'])} "
                f"({metrics['external_correct_tasks']}/{metrics['scored_tasks']})"
            )
    return "\n".join(lines) + "\n"


def _json_text(result: Mapping[str, Any]) -> str:
    return json.dumps(result, indent=2, sort_keys=True) + "\n"


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Measure independent mechanical metrics from AlignmentReport JSON files."
    )
    parser.add_argument("reports", nargs="+", metavar="REPORT.json")
    parser.add_argument(
        "--out",
        metavar="DEST",
        help="use 'json' for JSON on stdout, or write JSON to DEST",
    )
    args = parser.parse_args(argv)
    try:
        result = analyze_files(args.reports)
        if args.out == "json":
            sys.stdout.write(_json_text(result))
        elif args.out:
            try:
                Path(args.out).write_text(_json_text(result), encoding="utf-8")
            except OSError as exc:
                raise ReportError(f"{args.out}: cannot write output: {exc}") from exc
        else:
            sys.stdout.write(render_text(result))
    except ReportError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
