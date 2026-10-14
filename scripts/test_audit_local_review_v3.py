#!/usr/bin/env python3
"""Tests for the fail-closed v3 local-review evidence audit."""

from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from typing import Any, Mapping, Sequence

from scripts.audit_local_review_v3 import (
    DEFAULT_DEPENDENCIES,
    audit,
    build_prompt,
    canonical_json,
    prompt_view,
    sha256_file,
)


ROOT = Path(__file__).resolve().parents[1]
SOURCE_FIXTURE = ROOT / "benchmarks" / "research" / "review-traces-v3.json"
CONDITION = "revision_rubric_json"
SEEDS = (17, 29)
MODEL = "llama3.2:3b"


def _json_hash(value: Any) -> str:
    return hashlib.sha256(canonical_json(value)).hexdigest()


def _text_hash(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _write_json(path: Path, value: Any) -> None:
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def _dependency_key(path: str) -> str:
    resolved = Path(path)
    if not resolved.is_absolute():
        resolved = ROOT / resolved
    resolved = resolved.resolve()
    root = ROOT.resolve()
    return str(resolved.relative_to(root)) if resolved.is_relative_to(root) else str(resolved)


def _make_complete_evidence(
    directory: Path,
    *,
    observed_mode: str = "correct",
    model: str = MODEL,
    dependencies: Sequence[str] = DEFAULT_DEPENDENCIES,
) -> tuple[Path, Path, tuple[str, ...]]:
    """Build a small deterministic evidence bundle without making model calls."""

    evidence_dir = directory / "evidence"
    evidence_dir.mkdir(parents=True, exist_ok=True)
    fixture_path = evidence_dir / "fixture.json"
    fixture = json.loads(SOURCE_FIXTURE.read_text(encoding="utf-8"))
    _write_json(fixture_path, fixture)

    cases = fixture["cases"]
    schedule = [
        {
            "case_id": case["id"],
            "condition": CONDITION,
            "seed": seed,
            "schedule_index": index,
        }
        for index, (case, seed) in enumerate(
            (item for case in cases for item in ((case, SEEDS[0]), (case, SEEDS[1])))
        )
    ]

    results: list[dict[str, Any]] = []
    events: list[dict[str, Any]] = []
    for item in schedule:
        case = next(case for case in cases if case["id"] == item["case_id"])
        expected = str(case["expected_verdict"]).upper()
        if observed_mode == "correct":
            observed = expected
        elif observed_mode == "all_blocked":
            observed = "BLOCKED"
        else:
            raise ValueError(f"unknown observed_mode: {observed_mode}")
        prompt = build_prompt(case)
        payload = {
            "model": model,
            "prompt": prompt,
            "format": "json",
            "stream": False,
            "keep_alive": "2m",
            "options": {
                "seed": item["seed"],
                "num_ctx": 2048,
                "num_predict": 16,
                "temperature": 0.0,
            },
        }
        response_payload = {
            "model": model,
            "done": True,
            "done_reason": "stop",
            "response": json.dumps({"verdict": observed}, separators=(",", ":")),
            "prompt_eval_count": 3,
            "eval_count": 1,
        }
        common = {
            "case_id": item["case_id"],
            "condition": CONDITION,
            "schedule_index": item["schedule_index"],
            "seed": item["seed"],
        }
        events.append(
            {
                **common,
                "event": "request",
                "sequence": item["schedule_index"] * 2,
                "model": model,
                "payload": payload,
                "payload_sha256": _json_hash(payload),
                "prompt_data_sha256": _json_hash(prompt_view(case)),
                "prompt_sha256": _text_hash(prompt),
            }
        )
        events.append(
            {
                **common,
                "event": "response",
                "sequence": item["schedule_index"] * 2 + 1,
                "response": response_payload,
            }
        )
        results.append(
            {
                **common,
                "status": "ok",
                "expected": expected,
                "observed": observed,
                "error": None,
                "latency_ms": 1.0,
                "tokens": {"prompt": 3, "completion": 1, "total": 4},
            }
        )

    dependency_hashes = {
        _dependency_key(path): sha256_file(
            Path(path) if Path(path).is_absolute() else ROOT / path
        )
        for path in dependencies
    }
    manifest = {
        "schema_version": 3,
        "primary_model": MODEL,
        "model": model,
        "primary_analysis": model == MODEL,
        "model_digest": "sha256:test-model",
        "server_version": "0.4.2",
        "fixture": str(fixture_path),
        "fixture_sha256": sha256_file(fixture_path),
        "script_sha256": sha256_file(ROOT / "scripts/local_review_probe_v3.py"),
        "schedule_seed": 20261012,
        "seeds": list(SEEDS),
        "condition": CONDITION,
        "analysis": "test fixture",
        "schedule": schedule,
        "metadata_errors": [],
        "completed_records": len(results),
        "started_at": "2026-01-01T00:00:00Z",
        "ended_at": "2026-01-01T00:00:01Z",
        "base_url": "http://127.0.0.1:11434",
        "environment": {
            "os": "test",
            "architecture": "test",
            "cpu_count": 1,
            "memory_bytes": 1,
            "python_version": "test",
            "git_head": "test",
            "ollama_binary_sha256": "sha256:test-ollama",
            "local_only": True,
        },
        "model_info": {
            "parameter_size": "3.2B",
            "quantization_level": "Q4_K_M",
        },
        "dependency_hashes": dependency_hashes,
    }
    pair_ids = sorted({str(case["pair_id"]) for case in cases})
    summary = {
        "schema_version": 3,
        "benchmark_kind": "held-out-synthetic-review-gate-component-v3",
        "live_coding": False,
        "repository_execution": False,
        "primary_model": MODEL,
        "model": model,
        "primary_analysis": model == MODEL,
        "scheduled_calls": len(results),
        "unique_traces": len(cases),
        "independent_base_task_clusters": len(pair_ids),
        "valid_responses": len(results),
        "scheduled_accuracy": 1.0 if observed_mode == "correct" else 0.5,
        "cluster_comparison": {
            "wins": 0,
            "losses": 0,
            "ties": len(pair_ids),
            "test": "two-sided exact sign test",
            "p_value": None,
            "alpha": 0.05,
            "significant": False,
        },
        "clusters": [
            {
                "pair_id": pair_id,
                "gate_score": 1.0,
                "model_score": 1.0 if observed_mode == "correct" else 0.5,
                "comparison": "tie" if observed_mode == "correct" else "win",
            }
            for pair_id in pair_ids
        ],
        "limits": [],
    }
    _write_json(evidence_dir / "manifest.json", manifest)
    _write_json(evidence_dir / "results.json", {"schema_version": 3, "results": results})
    _write_json(evidence_dir / "summary.json", summary)
    (evidence_dir / "events.jsonl").write_text(
        "".join(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n" for event in events),
        encoding="utf-8",
    )
    return evidence_dir, fixture_path, tuple(dependencies)


class AuditLocalReviewV3Tests(unittest.TestCase):
    def test_complete_evidence_passes_and_reports_metrics(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            evidence, fixture, dependencies = _make_complete_evidence(Path(temporary))
            report = audit(evidence, fixture, ROOT, dependencies)

        self.assertTrue(report["valid"])
        self.assertTrue(report["integrity_pass"])
        self.assertTrue(report["provenance_complete"])
        self.assertEqual(report["metrics"]["balanced_accuracy"]["rate"], 1.0)
        self.assertEqual(report["metrics"]["ready_recall"]["rate"], 1.0)
        self.assertEqual(report["metrics"]["blocked_recall"]["rate"], 1.0)
        self.assertEqual(report["metrics"]["false_ready"]["rate"], 0.0)
        self.assertEqual(report["metrics"]["wrong_block"]["rate"], 0.0)
        self.assertEqual(report["cluster_effect"]["risk_difference"], 0.0)
        self.assertEqual(report["cluster_effect"]["bootstrap"]["seed"], 20261012)
        self.assertEqual(report["cluster_effect"]["bootstrap"]["replicates"], 10_000)
        self.assertTrue(report["summary_recomputation"]["matches_summary"])

    def test_secondary_model_keeps_significance_flag_false(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            evidence, fixture, dependencies = _make_complete_evidence(
                Path(temporary), observed_mode="all_blocked", model="llama3.2:1b"
            )
            manifest_path = evidence / "manifest.json"
            summary_path = evidence / "summary.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            manifest["primary_analysis"] = False
            summary["primary_analysis"] = False
            summary["cluster_comparison"].update(
                {
                    "wins": 16,
                    "losses": 0,
                    "ties": 0,
                    "p_value": 0.000030517578125,
                    "significant": False,
                }
            )
            _write_json(manifest_path, manifest)
            _write_json(summary_path, summary)
            report = audit(evidence, fixture, ROOT, dependencies)

        self.assertTrue(report["integrity_pass"])

    def test_secondary_model_cannot_mark_primary_significant(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            evidence, fixture, dependencies = _make_complete_evidence(
                Path(temporary), observed_mode="all_blocked", model="llama3.2:1b"
            )
            manifest_path = evidence / "manifest.json"
            summary_path = evidence / "summary.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            manifest["primary_analysis"] = True
            summary["primary_analysis"] = True
            summary["cluster_comparison"].update(
                {
                    "wins": 16,
                    "losses": 0,
                    "ties": 0,
                    "p_value": 0.000030517578125,
                    "significant": True,
                }
            )
            _write_json(manifest_path, manifest)
            _write_json(summary_path, summary)
            report = audit(evidence, fixture, ROOT, dependencies)

        self.assertFalse(report["valid"])
        self.assertTrue(
            any("primary_analysis" in error for error in report["errors"])
        )

    def test_missing_provenance_is_explicit_and_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            evidence, fixture, dependencies = _make_complete_evidence(Path(temporary))
            manifest_path = evidence / "manifest.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest.pop("environment")
            manifest.pop("dependency_hashes")
            _write_json(manifest_path, manifest)
            report = audit(evidence, fixture, ROOT, dependencies)

        self.assertFalse(report["valid"])
        self.assertFalse(report["provenance_complete"])
        self.assertIn("manifest.environment.os", report["provenance"]["missing"])
        self.assertIn("manifest.dependency_hashes", report["provenance"]["missing"])
        self.assertNotIn("environment", report["provenance"]["present"])

    def test_duplicate_schedule_and_missing_response_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            evidence, fixture, dependencies = _make_complete_evidence(Path(temporary))
            manifest_path = evidence / "manifest.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["schedule"][-1]["schedule_index"] = 0
            _write_json(manifest_path, manifest)
            events_path = evidence / "events.jsonl"
            lines = events_path.read_text(encoding="utf-8").splitlines()
            events_path.write_text("\n".join(lines[:-1]) + "\n", encoding="utf-8")
            report = audit(evidence, fixture, ROOT, dependencies)

        self.assertFalse(report["valid"])
        self.assertTrue(any("duplicate schedule_index" in error for error in report["errors"]))
        self.assertTrue(any("request/response pair incomplete" in error for error in report["errors"]))

    def test_summary_mismatch_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            evidence, fixture, dependencies = _make_complete_evidence(Path(temporary))
            summary_path = evidence / "summary.json"
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            summary["scheduled_accuracy"] = 0.5
            _write_json(summary_path, summary)
            report = audit(evidence, fixture, ROOT, dependencies)

        self.assertFalse(report["valid"])
        self.assertTrue(
            any("summary.scheduled_accuracy" in error for error in report["errors"])
        )

    def test_dependency_hash_mismatch_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            dependency = Path(temporary) / "dependency.py"
            dependency.write_text("version = 1\n", encoding="utf-8")
            dependency_arg = str(dependency)
            evidence, fixture, dependencies = _make_complete_evidence(
                Path(temporary), dependencies=(dependency_arg,)
            )
            dependency.write_text("version = 2\n", encoding="utf-8")
            report = audit(evidence, fixture, ROOT, dependencies)

        self.assertFalse(report["valid"])
        self.assertTrue(any("dependency hash mismatch" in error for error in report["errors"]))


if __name__ == "__main__":
    unittest.main()
