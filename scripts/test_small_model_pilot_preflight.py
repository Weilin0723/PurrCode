import tempfile
import unittest
from types import SimpleNamespace
from pathlib import Path
from unittest.mock import patch

from scripts import small_model_pilot_preflight as preflight
from scripts.small_model_pilot_preflight import (
    DEFAULT_DATASET,
    apply_operations,
    read_json,
    run_preflight,
    test_integrity_passes,
    validate_dataset,
    validate_fixture,
)


class SmallModelPilotPreflightTests(unittest.TestCase):
    def test_dataset_is_frozen_b_c_six_task_design(self):
        data = read_json(DEFAULT_DATASET)
        validate_dataset(data, DEFAULT_DATASET)
        self.assertEqual(data["arms"], ["B", "C"])
        self.assertEqual(len(data["tasks"]), 6)
        self.assertEqual(len(data["candidate_types"]), 6)
        self.assertEqual(data["run_plan"]["scheduled_model_calls_per_model"], 144)
        self.assertEqual(data["run_plan"]["scheduled_model_calls"], 288)
        self.assertEqual(data["run_plan"]["unique_d_evaluations"], 36)
        self.assertEqual(data["models"]["primary"], "llama3.2:3b")
        self.assertTrue(all(data["status"] == "design_only" for _ in data["tasks"]))

    def test_fixture_files_are_present(self):
        data = read_json(DEFAULT_DATASET)
        fixture = validate_fixture(data, DEFAULT_DATASET.parents[3])
        self.assertTrue((fixture / "Cargo.toml").is_file())
        self.assertTrue((fixture / "src" / "pagination.rs").is_file())

    def test_test_integrity_rejects_removed_baseline_line(self):
        data = read_json(DEFAULT_DATASET)
        fixture = validate_fixture(data, DEFAULT_DATASET.parents[3])
        task = next(item for item in data["tasks"] if item["id"] == "pagination-tail")
        with tempfile.TemporaryDirectory(prefix="small-model-pilot-test-") as temporary:
            workspace = Path(temporary) / "fixture"
            import shutil

            shutil.copytree(fixture, workspace)
            apply_operations(workspace, task["mutations"]["test_tampering"]["operations"])
            self.assertFalse(test_integrity_passes(fixture, workspace, task["test_integrity"]))

    def test_oracle_infrastructure_failure_is_unknown_not_reject(self):
        data = read_json(DEFAULT_DATASET)
        task = data["tasks"][0]
        with tempfile.TemporaryDirectory(prefix="small-model-pilot-oracle-") as temporary:
            with patch.object(
                preflight.subprocess,
                "run",
                return_value=SimpleNamespace(returncode=1, stdout='{"passed":false}\n', stderr="runner failed"),
            ):
                result = preflight.run_oracle(
                    DEFAULT_DATASET.parents[3],
                    Path(temporary),
                    task["oracle"],
                )
        self.assertEqual(result["label"], "UNKNOWN")
        self.assertIsNone(result["passed"])
        self.assertFalse(result["explicit"])

    def test_all_mutation_expectations_pass_external_oracles(self):
        report = run_preflight(DEFAULT_DATASET)
        self.assertTrue(report["all_expected"])
        self.assertEqual(report["accepted_gold"], 6)
        self.assertEqual(report["candidate_snapshots"], 36)
        self.assertEqual(report["scheduled_model_calls"], 0)
        self.assertEqual(report["oracle_unknown"], 0)


if __name__ == "__main__":
    unittest.main()
