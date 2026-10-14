#!/usr/bin/env python3
"""Run the frozen v3 held-out component comparison.

The primary comparison is the deterministic revision/artifact gate versus the
configured local reviewer, clustered by paired base task.  This is a synthetic
component study, not a repository coding benchmark.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import random
import sys
from pathlib import Path
from typing import Any, Mapping, Sequence

try:
    from .deterministic_review_gate_v2 import verdict as deterministic_verdict_v2
    from .local_review_probe import (
        DEFAULT_BASE_URL,
        EventLog,
        OutputExistsError,
        create_output_dir,
        server_metadata,
        sha256_file,
        utc_now,
        validate_local_base_url,
        write_json,
    )
    from .local_review_probe_v2 import (
        SEEDS,
        build_prompt_v2,
        run_schedule_v2,
        validate_prompt_data,
    )
except ImportError:
    from deterministic_review_gate_v2 import verdict as deterministic_verdict_v2
    from local_review_probe import (
        DEFAULT_BASE_URL,
        EventLog,
        OutputExistsError,
        create_output_dir,
        server_metadata,
        sha256_file,
        utc_now,
        validate_local_base_url,
        write_json,
    )
    from local_review_probe_v2 import (
        SEEDS,
        build_prompt_v2,
        run_schedule_v2,
        validate_prompt_data,
    )


SCHEMA_VERSION = 3
PRIMARY_MODEL = "llama3.2:3b"
CONDITION = "revision_rubric_json"
EXPECTED_PAIRS = 16
DEFAULT_FIXTURE = (
    Path(__file__).resolve().parents[1]
    / "benchmarks"
    / "research"
    / "review-traces-v3.json"
)


def load_fixture_v3(path: Path) -> list[dict[str, Any]]:
    root = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(root, dict) or root.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(f"{path}: expected schema_version={SCHEMA_VERSION}")
    raw_cases = root.get("cases")
    if not isinstance(raw_cases, list) or len(raw_cases) != EXPECTED_PAIRS * 2:
        raise ValueError(f"{path}: expected exactly {EXPECTED_PAIRS * 2} cases")

    cases: list[dict[str, Any]] = []
    ids: set[str] = set()
    pairs: dict[str, list[str]] = {}
    for index, raw in enumerate(raw_cases):
        where = f"{path}.cases[{index}]"
        if not isinstance(raw, dict):
            raise ValueError(f"{where}: expected an object")
        expected_keys = {
            "id",
            "pair_id",
            "data_kind",
            "prompt_data",
            "expected_verdict",
            "label_rationale",
        }
        if set(raw) != expected_keys:
            raise ValueError(f"{where}: unexpected keys")
        case = dict(raw)
        case_id = case["id"]
        pair_id = case["pair_id"]
        if not isinstance(case_id, str) or not case_id or case_id in ids:
            raise ValueError(f"{where}.id: expected a unique non-empty string")
        if not isinstance(pair_id, str) or not pair_id:
            raise ValueError(f"{where}.pair_id: expected a non-empty string")
        if case["expected_verdict"] not in {"ready", "blocked"}:
            raise ValueError(f"{where}.expected_verdict: expected ready/blocked")
        ids.add(case_id)
        pairs.setdefault(pair_id, []).append(case["expected_verdict"])
        case["prompt_data"] = validate_prompt_data(case["prompt_data"], f"{where}.prompt_data")
        cases.append(case)

    if len(pairs) != EXPECTED_PAIRS:
        raise ValueError(f"{path}: expected exactly {EXPECTED_PAIRS} pair ids")
    for pair_id, labels in pairs.items():
        if sorted(labels) != ["blocked", "ready"]:
            raise ValueError(f"{path}: pair {pair_id!r} must contain one ready and one blocked case")
    return cases


def build_schedule_v3(cases: Sequence[Mapping[str, Any]], schedule_seed: int) -> list[dict[str, Any]]:
    schedule = [
        {"case_id": str(case["id"]), "condition": CONDITION, "seed": seed}
        for case in cases
        for seed in SEEDS
    ]
    random.Random(schedule_seed).shuffle(schedule)
    for index, item in enumerate(schedule):
        item["schedule_index"] = index
    return schedule


def exact_two_sided_sign_test(wins: int, losses: int) -> float | None:
    discordant = wins + losses
    if discordant == 0:
        return None
    tail = sum(math.comb(discordant, index) for index in range(min(wins, losses) + 1))
    return min(1.0, 2.0 * tail / (2**discordant))


def deterministic_verdict(prompt_data: dict[str, Any]) -> tuple[str, list[str]]:
    verdict, reasons = deterministic_verdict_v2(prompt_data)
    if prompt_data["artifact"]["revision"] != prompt_data["request"]["revision"]:
        reasons = [*reasons, "stale artifact revision"]
        verdict = "BLOCKED"
    return verdict, reasons


def summarize_v3(
    results: Sequence[Mapping[str, Any]], cases: Sequence[Mapping[str, Any]], model: str
) -> dict[str, Any]:
    by_case = {str(case["id"]): case for case in cases}
    valid = [row for row in results if row["status"] == "ok"]
    correct = [row for row in valid if row["observed"] == row["expected"]]
    pair_rows: dict[str, list[Mapping[str, Any]]] = {}
    for row in results:
        pair_rows.setdefault(str(by_case[str(row["case_id"])]["pair_id"]), []).append(row)

    clusters = []
    wins = losses = ties = 0
    for pair_id in sorted(pair_rows):
        rows = pair_rows[pair_id]
        model_correct = sum(
            row["status"] == "ok" and row["observed"] == row["expected"] for row in rows
        )
        model_score = model_correct / len(rows)
        gate_correct = sum(
            deterministic_verdict(case["prompt_data"])[0]
            == str(case["expected_verdict"]).upper()
            for case in cases
            if case["pair_id"] == pair_id
        )
        gate_score = gate_correct / 2
        comparison = "win" if gate_score > model_score else "loss" if gate_score < model_score else "tie"
        wins += comparison == "win"
        losses += comparison == "loss"
        ties += comparison == "tie"
        clusters.append(
            {
                "pair_id": pair_id,
                "gate_score": gate_score,
                "model_score": model_score,
                "comparison": comparison,
            }
        )

    p_value = exact_two_sided_sign_test(wins, losses)
    return {
        "schema_version": SCHEMA_VERSION,
        "benchmark_kind": "held-out-synthetic-review-gate-component-v3",
        "live_coding": False,
        "repository_execution": False,
        "primary_model": PRIMARY_MODEL,
        "model": model,
        "primary_analysis": model == PRIMARY_MODEL,
        "scheduled_calls": len(results),
        "unique_traces": len(cases),
        "independent_base_task_clusters": len(pair_rows),
        "valid_responses": len(valid),
        "scheduled_accuracy": len(correct) / len(results),
        "cluster_comparison": {
            "wins": wins,
            "losses": losses,
            "ties": ties,
            "test": "two-sided exact sign test; repeated seeds and paired traces collapsed within base task",
            "p_value": p_value,
            "alpha": 0.05,
            "significant": bool(model == PRIMARY_MODEL and p_value is not None and p_value < 0.05),
        },
        "clusters": clusters,
        "limits": [
            "Synthetic component evidence only; no repository code was executed.",
            "Statistical significance does not establish coding-agent efficacy or external validity.",
        ],
    }


def run_probe_v3(
    *,
    model: str,
    output: Path,
    fixture: Path = DEFAULT_FIXTURE,
    base_url: str = DEFAULT_BASE_URL,
    schedule_seed: int = 20261012,
) -> dict[str, Any]:
    base_url = validate_local_base_url(base_url)
    create_output_dir(output)
    cases = load_fixture_v3(fixture)
    schedule = build_schedule_v3(cases, schedule_seed)
    server_version, model_digest, metadata_errors = server_metadata(base_url, model)
    script = Path(__file__).resolve()
    manifest = {
        "schema_version": SCHEMA_VERSION,
        "primary_model": PRIMARY_MODEL,
        "model": model,
        "model_digest": model_digest,
        "server_version": server_version,
        "fixture": str(fixture),
        "fixture_sha256": sha256_file(fixture),
        "script_sha256": sha256_file(script),
        "schedule_seed": schedule_seed,
        "seeds": list(SEEDS),
        "condition": CONDITION,
        "analysis": "two-sided exact sign test at alpha 0.05 over 16 paired base-task clusters",
        "schedule": schedule,
        "metadata_errors": metadata_errors,
        "started_at": utc_now(),
    }
    write_json(output / "manifest.json", manifest)
    events = EventLog(output / "events.jsonl")
    results = run_schedule_v2(cases, schedule, base_url=base_url, model=model, events=events)
    summary = summarize_v3(results, cases, model)
    write_json(output / "results.json", {"schema_version": SCHEMA_VERSION, "results": results})
    write_json(output / "summary.json", summary)
    manifest.update({"ended_at": utc_now(), "completed_records": len(results)})
    write_json(output / "manifest.json", manifest)
    return summary


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default=PRIMARY_MODEL)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--fixture", default=DEFAULT_FIXTURE, type=Path)
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL)
    parser.add_argument("--schedule-seed", default=20261012, type=int)
    args = parser.parse_args(argv)
    try:
        summary = run_probe_v3(
            model=args.model,
            output=args.out,
            fixture=args.fixture,
            base_url=args.base_url,
            schedule_seed=args.schedule_seed,
        )
    except (OutputExistsError, ValueError, OSError, json.JSONDecodeError) as exc:
        print(f"local review probe v3 failed before completion: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
