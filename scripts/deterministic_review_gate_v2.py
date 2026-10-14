#!/usr/bin/env python3
"""Evaluate the frozen v2 review traces with the runtime's deterministic gate."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def verdict(prompt_data: dict) -> tuple[str, list[str]]:
    request = prompt_data["request"]
    artifact = prompt_data["artifact"]
    evidence = prompt_data["evidence"]
    reasons: list[str] = []

    if evidence["revision"] != request["revision"]:
        reasons.append("stale requirement revision")
    if evidence["artifact_digest"] != artifact["digest"]:
        reasons.append("stale artifact digest")

    observations = {}
    for observation in evidence["observations"]:
        observations.setdefault(observation["requirement_id"], []).append(
            observation["status"]
        )
    for requirement in request["requirements"]:
        statuses = observations.get(requirement["id"], [])
        if statuses != ["PASS"]:
            reasons.append(
                f"{requirement['id']} needs exactly one PASS observation; got {statuses}"
            )

    return ("BLOCKED" if reasons else "READY", reasons)


def run(fixture: Path) -> dict:
    fixture_bytes = fixture.read_bytes()
    data = json.loads(fixture_bytes)
    results = []
    for case in data["cases"]:
        observed, reasons = verdict(case["prompt_data"])
        expected = case["expected_verdict"].upper()
        results.append(
            {
                "case_id": case["id"],
                "pair_id": case["pair_id"],
                "expected": expected,
                "observed": observed,
                "correct": observed == expected,
                "reasons": reasons,
            }
        )
    correct = sum(result["correct"] for result in results)
    return {
        "schema_version": 1,
        "benchmark_kind": "deterministic_revision_and_artifact_gate",
        "development_data": True,
        "fixture_sha256": hashlib.sha256(fixture_bytes).hexdigest(),
        "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "cases": len(results),
        "correct": correct,
        "accuracy": correct / len(results),
        "false_ready": sum(
            result["observed"] == "READY" and result["expected"] == "BLOCKED"
            for result in results
        ),
        "wrong_block": sum(
            result["observed"] == "BLOCKED" and result["expected"] == "READY"
            for result in results
        ),
        "results": results,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--fixture",
        type=Path,
        default=Path("benchmarks/research/review-traces-v2.json"),
    )
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = run(args.fixture)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "summary.json").write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps({key: result[key] for key in ("cases", "correct", "accuracy", "false_ready", "wrong_block")}))


if __name__ == "__main__":
    main()
