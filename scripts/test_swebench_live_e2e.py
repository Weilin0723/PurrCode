#!/usr/bin/env python3
"""Focused offline tests for the SWE-bench-Live E2E runner."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fse_confirmatory_analysis as analysis
import swebench_live_e2e as e2e


class SwebenchLiveE2ETests(unittest.TestCase):
    def test_unknown_usage_is_reported_as_unmeasured(self) -> None:
        metrics = e2e.event_metrics([
            dict(event="model_request_started", data={"provider": "synthetic"}),
            dict(event="model_request_finished", data={"input_tokens": None, "output_tokens": None})])
        self.assertEqual(metrics["model_calls"], 1)
        self.assertEqual(metrics["unknown_usage_calls"], 1)

    def test_blocked_review_does_not_receive_correction(self) -> None:
        view = dict(status_code="awaiting_review", lease_active=False)
        task = dict(objective="synthetic", model="synthetic", max_tokens=10,
                    arm="C", variant="correction", mid_run_correction="correction")
        with patch.object(e2e, "http_json", return_value={"id": "s"}) as http, \
             patch.object(e2e, "get_session", return_value=view), \
             patch.object(e2e, "get_events", return_value=[]), \
             patch.object(e2e, "wait_until_stopped", return_value=view), \
             patch.object(e2e, "first_successful_mutation", return_value=({}, 0)):
            measurement, _, _ = e2e.run_session(
                base="unused", token="synthetic", task=task,
                repository=Path("/synthetic"), maximum_seconds=1)
        self.assertFalse(measurement["mid_run_correction_triggered"])
        self.assertEqual(http.call_count, 1)

    def test_duplicate_result_rows_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            result = Path(temporary) / "results.jsonl"
            result.write_text('{"run_id":"same"}\n{"run_id":"same"}\n')
            with self.assertRaisesRegex(e2e.E2EError, "duplicate run ID"):
                e2e.read_results(result)

    def test_failed_session_and_unknown_oracle_are_not_valid_observations(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            task = dict(run_id="r", task_id="r", repository="org/repo", base_task_id="t",
                        variant="static", arm="B", repeat=0, model="nim/model", maximum_seconds=1)
            measurement = dict(session_id="s", session_status="failed", elapsed_ms=1,
                               timed_out=False, mid_run_correction_requested=False,
                               mid_run_correction_triggered=False, mid_run_correction_acknowledged=False,
                               mid_run_correction_trigger_event_sha256=None,
                               mid_run_correction_message_event_sha256=None)
            with patch.object(e2e, "verify_checkout", return_value=root), \
                 patch.object(e2e, "run_session", return_value=(measurement, [], {})), \
                 patch.object(e2e, "state_worktree", return_value=root), \
                 patch.object(e2e, "extract_patch", return_value=b""), \
                 patch.object(e2e, "run_oracle", return_value=dict(status="unknown", oracle_run_id="o")):
                row = e2e.run_one(task, cache_dir=root, base="unused", token="synthetic",
                                  output=root, plan=dict(study_id="test", dataset_manifest_sha256="d",
                                                         schedule_sha256="s"), freeze={}, bundle_sha256="b",
                                  oracle_python=root, oracle_runner=root, hf_home=root, oracle_timeout=1)
            self.assertEqual(row["terminal_status"], "error")
            self.assertIsNone(row["claimed_complete"])
            self.assertIsNone(row["externally_correct"])

    def _fixture(self, root: Path) -> tuple[Path, Path, Path]:
        cache = root / "cache"
        cache.mkdir()
        repositories = [f"org/repo-{index}" for index in range(10)]
        tasks = []
        revisions = []
        for repo_index, repository in enumerate(repositories):
            for task_index in range(3):
                instance_id = f"{repository.replace('/', '__')}-{task_index}"
                checkout = cache / instance_id
                checkout.mkdir()
                tasks.append(
                    {
                        "instance_id": instance_id,
                        "repo": repository,
                        "repo_url": f"https://example.invalid/{repository}.git",
                        "base_commit": "a" * 40,
                    }
                )
                full = f"full requirement {instance_id}"
                p0 = f"abridged {instance_id}"
                p1 = f"withheld {instance_id}"
                revisions.append(
                    {
                        "instance_id": instance_id,
                        "repo": repository,
                        "repo_url": f"https://example.invalid/{repository}.git",
                        "base_commit": "a" * 40,
                        "full_statement": full,
                        "full_statement_sha256": hashlib.sha256(full.encode()).hexdigest(),
                        "abridged_p0": p0,
                        "abridged_p0_sha256": hashlib.sha256(p0.encode()).hexdigest(),
                        "p1_text": p1,
                        "p1_text_sha256": hashlib.sha256(p1.encode()).hexdigest(),
                    }
                )
        manifest = {
            "source": {"revision": "revision"},
            "run_plan": {
                "repositories": 10,
                "tasks": 30,
                "arms": ["B", "C"],
                "request_variants": ["static", "correction"],
                "repeat_labels": [0],
                "runs_per_model": 120,
            },
            "tasks": tasks,
        }
        revision = {"schema_version": 2, "task_count": 30, "tasks": revisions}
        manifest_path = root / "manifest.json"
        revision_path = root / "revision.json"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        revision_path.write_text(json.dumps(revision), encoding="utf-8")
        return cache, manifest_path, revision_path

    def test_prepare_bundle_uses_frozen_schedule(self) -> None:
        with tempfile.TemporaryDirectory(prefix="purrcode-e2e-test-") as temporary:
            root = Path(temporary)
            cache, manifest_path, revision_path = self._fixture(root)
            output = root / "out"
            report = e2e.prepare_bundle(
                manifest_path=manifest_path,
                revision_path=revision_path,
                cache_dir=cache,
                out=output,
                model="deepseek/deepseek-v4-pro",
            )
            self.assertEqual(report["task_count"], 120)
            bundle = json.loads((output / "run-bundle.json").read_text(encoding="utf-8"))
            plan = {
                "primary_model": "deepseek/deepseek-v4-pro",
                "repositories": {},
                "request_variants": list(analysis.REQUEST_VARIANTS),
                "repeats": list(analysis.REPEAT_LABELS),
            }
            for task in json.loads(manifest_path.read_text(encoding="utf-8"))["tasks"]:
                plan["repositories"].setdefault(task["repo"], []).append(task["instance_id"])
            expected = analysis.schedule_payload(plan)
            actual = [
                {key: task[key] for key in ("base_task_id", "variant", "arm", "model", "repeat", "repository")}
                for task in bundle["tasks"]
            ]
            self.assertEqual(actual, expected)
            self.assertTrue(all(item["objective"] for item in bundle["tasks"]))
            self.assertEqual(sum(item["mid_run_correction"] is not None for item in bundle["tasks"]), 60)

    def test_extract_patch_includes_untracked_files(self) -> None:
        with tempfile.TemporaryDirectory(prefix="purrcode-patch-test-") as temporary:
            root = Path(temporary)
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.email", "test@example.invalid"], cwd=root, check=True)
            (root / "tracked.txt").write_text("before\n", encoding="utf-8")
            subprocess.run(["git", "add", "tracked.txt"], cwd=root, check=True)
            subprocess.run(["git", "commit", "-q", "-m", "base"], cwd=root, check=True)
            (root / "tracked.txt").write_text("after\n", encoding="utf-8")
            (root / "new.txt").write_text("new\n", encoding="utf-8")
            patch = e2e.extract_patch(root).decode("utf-8")
            self.assertIn("tracked.txt", patch)
            self.assertIn("new.txt", patch)


    def test_primary_read_is_not_a_mutation_trigger(self) -> None:
        action_id = "00000000-0000-0000-0000-000000000001"
        read_evidence = {
            "event": "tool_evidence_recorded",
            "data": {
                "evidence": {
                    "action_id": action_id,
                    "tool_id": "native:read_file",
                    "effective_filesystem_scope": {"kind": "worktree_read"},
                }
            },
        }
        read_finished = {
            "event": "execution_finished",
            "data": {"action_id": action_id, "exit_code": 0, "affected_paths": ["src/lib.rs"]},
        }
        self.assertIsNone(e2e.first_successful_mutation([read_evidence, read_finished]))
        write_evidence = {
            "event": "tool_evidence_recorded",
            "data": {
                "evidence": {
                    "action_id": action_id,
                    "tool_id": "native:write_file",
                    "effective_filesystem_scope": {"kind": "worktree", "write_globs": ["**"]},
                }
            },
        }
        self.assertIsNotNone(e2e.first_successful_mutation([write_evidence, read_finished]))


    def test_operator_purrcode_state_does_not_dirty_source_checkout(self) -> None:
        with tempfile.TemporaryDirectory(prefix="purrcode-checkout-test-") as temporary:
            root = Path(temporary)
            checkout = root / "checkout"
            checkout.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=checkout, check=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=checkout, check=True)
            subprocess.run(["git", "config", "user.email", "test@example.invalid"], cwd=checkout, check=True)
            (checkout / "tracked.txt").write_text("tracked\n", encoding="utf-8")
            subprocess.run(["git", "add", "tracked.txt"], cwd=checkout, check=True)
            subprocess.run(["git", "commit", "-q", "-m", "base"], cwd=checkout, check=True)
            head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, text=True).strip()
            (checkout / ".purrcode").mkdir()
            (checkout / ".purrcode" / "context.db").write_text("local state", encoding="utf-8")
            task = {"base_task_id": "task", "checkout": "checkout", "base_commit": head}
            self.assertEqual(e2e.verify_checkout(task, root), checkout.resolve())

    def test_normalized_correction_digest_ignores_trailing_space(self) -> None:
        self.assertEqual(
            e2e.normalized_correction_digest("keep it  "),
            e2e.normalized_correction_digest("keep it"),
        )


if __name__ == "__main__":
    unittest.main()
