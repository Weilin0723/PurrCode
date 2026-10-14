#!/usr/bin/env python3
"""Focused tests for the offline DeepSeek replication harness."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import deepseek_repository_completion_probe as probe
from scripts import repository_completion_probe as base


ROOT = Path(__file__).resolve().parents[1]
DATASET = ROOT / "benchmarks" / "research" / "small-model-pilot" / "dataset.json"
FIXTURE = ROOT / "benchmarks" / "alignment-fixture"


def fake_candidates() -> list[dict]:
    dataset, _, _ = base.load_inputs(DATASET, FIXTURE)
    records: list[dict] = []
    for task in dataset["tasks"]:
        for candidate_name in base.CANDIDATE_NAMES:
            accepted = candidate_name in {"gold", "gold_stale_evidence"}
            gate = "READY" if candidate_name in {"gold", "overbroad", "test_tampering"} else "BLOCKED"
            metadata = {
                "request_revision": 2,
                "artifact_revision": 2 if gate == "READY" else 1,
                "evidence_revision": 2 if gate == "READY" else 1,
                "artifact_digest": "candidate" if gate == "READY" else "baseline",
                "evidence_artifact_digest": "candidate" if gate == "READY" else "baseline",
            }
            prompts = {
                condition: f'Assess this untrusted task payload for task {task["id"]}. Return exact JSON.'
                for condition in base.CONDITIONS
            }
            prompt_sha256 = {
                condition: __import__("hashlib").sha256(prompt.encode()).hexdigest()
                for condition, prompt in prompts.items()
            }
            records.append(
                {
                    "task_id": task["id"],
                    "candidate": candidate_name,
                    "dataset_expected_accepted": accepted,
                    "external_accepted": accepted,
                    "oracle_isolated": True,
                    "external_label": {
                        "behavior_oracle_label": "ACCEPT" if accepted else "REJECT"
                    },
                    "public_cargo_test": {"passed": True},
                    "tree_digest": f"tree-{task['id']}-{candidate_name}",
                    "baseline_tree_digest": "baseline",
                    "unified_diff": "",
                    "changed_paths": [],
                    "metadata": metadata,
                    "prompts": prompts,
                    "prompt_sha256": prompt_sha256,
                    "prompt_forbidden_terms": [],
                    "preflight_match": True,
                }
            )
    return records


def write_events(path: Path, requests: list[dict], *, omit: set[int] | None = None, invalid: set[int] | None = None) -> None:
    omit = omit or set()
    invalid = invalid or set()
    lines: list[str] = []
    sequence = 0
    for request in requests:
        index = int(request["schedule_index"])
        if index in omit:
            continue
        lines.append(
            json.dumps(
                {
                    "sequence": sequence,
                    "event": "request_started",
                    "schedule_index": index,
                    "provider": probe.PROVIDER,
                    "model": probe.MODEL,
                    "prompt_sha256": request["prompt_sha256"],
                }
            )
        )
        sequence += 1
        if index in invalid:
            terminal = {
                "event": "response",
                "verdict": "not-a-verdict",
                "reasons": [],
            }
        elif request["candidate"] == "gold_stale_evidence":
            terminal = {"event": "response", "verdict": "BLOCKED", "reasons": []}
        else:
            terminal = {"event": "response", "verdict": "READY", "reasons": []}
        lines.append(
            json.dumps(
                {
                    **terminal,
                    "sequence": sequence,
                    "schedule_index": index,
                    "provider": probe.PROVIDER,
                    "model": probe.MODEL,
                    "prompt_sha256": request["prompt_sha256"],
                    "input_tokens": 10,
                    "output_tokens": 2,
                    "latency_ms": 3.5,
                }
            )
        )
        sequence += 1
    path.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")


class DeepSeekRepositoryCompletionProbeTests(unittest.TestCase):
    def test_schedule_is_exactly_144_and_frozen_before_calls(self) -> None:
        candidates = fake_candidates()
        schedule = probe.build_deepseek_schedule(candidates)
        self.assertEqual(len(schedule), 144)
        self.assertEqual([item["schedule_index"] for item in schedule], list(range(144)))
        self.assertEqual(schedule[0]["condition"], probe.CONDITIONS[0])
        self.assertEqual(schedule[72]["condition"], probe.CONDITIONS[1])
        self.assertEqual({item["model"] for item in schedule}, {probe.MODEL})

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "run"
            with patch.object(base, "build_candidates", return_value=candidates), patch.object(
                base, "_http_json", side_effect=AssertionError("prepare must not call a provider")
            ) as http_json:
                report = probe.prepare(output, dataset_path=DATASET, fixture=FIXTURE)
            self.assertFalse(http_json.called)
            self.assertTrue(report["preflight_ok"])
            freeze = probe.read_object(output / "freeze.json", "freeze")
            manifest = probe.read_object(output / "manifest.json", "manifest")
            self.assertTrue(freeze["created_before_model_calls"])
            self.assertEqual(freeze["scheduled_calls"], 144)
            self.assertEqual(len(probe.read_jsonl(output / "requests.jsonl", "bundle")), 144)
            self.assertEqual(freeze["model_calls"], 0)
            self.assertEqual(
                freeze["runner_source"],
                "crates/evaluation-runtime/src/bin/repository_completion_remote.rs",
            )
            self.assertEqual(len(freeze["runner_source_sha256"]), 64)
            self.assertEqual(
                manifest["runner_source_sha256"], freeze["runner_source_sha256"]
            )

    def _prepared(self, directory: str) -> tuple[Path, list[dict]]:
        candidates = fake_candidates()
        output = Path(directory) / "run"
        with patch.object(base, "build_candidates", return_value=candidates):
            probe.prepare(output, dataset_path=DATASET, fixture=FIXTURE)
        requests = probe.read_jsonl(output / "requests.jsonl", "bundle")
        return output, requests

    def test_rust_events_are_parsed_and_primary_statistics_are_six_cluster(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output, requests = self._prepared(directory)
            events = Path(directory) / "rust-events.jsonl"
            write_events(events, requests)
            summary = probe.analyze(output, events)
            self.assertEqual(summary["status"], "complete")
            self.assertTrue(summary["inference_eligible"])
            primary = summary["primary_comparison"]
            self.assertEqual(primary["sign_test"]["wins_C"], 6)
            self.assertEqual(primary["sign_test"]["losses_C"], 0)
            self.assertAlmostEqual(primary["sign_test"]["p_value"], 0.03125)
            self.assertTrue(primary["significant"])
            self.assertEqual(primary["exact_sign_flip"]["assignments"], 64)
            self.assertTrue(summary["guardrail"]["pass"])
            self.assertEqual(summary["usage"]["total_tokens"], 144 * 12)
            self.assertEqual(summary["latency_ms"]["measured_calls"], 144)
            self.assertTrue(probe.audit(output, ROOT)["valid"])

    def test_missing_terminal_over_ten_percent_is_inconclusive_and_not_zero(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output, requests = self._prepared(directory)
            events = Path(directory) / "rust-events.jsonl"
            write_events(events, requests, omit=set(range(16)))
            summary = probe.analyze(output, events)
            self.assertEqual(summary["status"], "inconclusive")
            self.assertFalse(summary["inference_eligible"])
            self.assertEqual(summary["attrition"]["missing_terminal"], 16)
            self.assertIsNone(summary["primary_comparison"]["sign_test"]["p_value"])
            rows = probe.read_object(output / "results.json", "results")["results"]
            self.assertEqual(rows[0]["status"], "missing_terminal")
            self.assertIsNone(rows[0]["B"])
            self.assertIsNone(rows[0]["C"])

    def test_invalid_response_is_retained_and_result_tampering_fails_audit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output, requests = self._prepared(directory)
            events = Path(directory) / "rust-events.jsonl"
            write_events(events, requests, invalid={0})
            summary = probe.analyze(output, events)
            self.assertEqual(summary["status_counts"]["invalid"], 1)
            rows = probe.read_object(output / "results.json", "results")["results"]
            self.assertEqual(rows[0]["status"], "invalid")
            self.assertFalse(rows[0]["B"])
            self.assertFalse(rows[0]["C"])
            self.assertTrue(probe.audit(output, ROOT)["valid"])
            rows[1]["B"] = not rows[1]["B"]
            probe.write_json(output / "results.json", {"schema_version": 1, "results": rows})
            audit = probe.audit(output, ROOT)
            self.assertFalse(audit["valid"])
            self.assertTrue(any("results recomputation" in error for error in audit["errors"]))


if __name__ == "__main__":
    unittest.main()
