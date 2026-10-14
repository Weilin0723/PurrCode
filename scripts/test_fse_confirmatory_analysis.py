import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("fse_confirmatory_analysis.py")
SPEC = importlib.util.spec_from_file_location("fse_confirmatory_analysis", MODULE_PATH)
assert SPEC and SPEC.loader
analysis = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(analysis)

DATASET_MANIFEST = {"schema_version": 1, "dataset": "test", "revision": "test-revision"}
ARTIFACT_PATHS = {
    "analysis_script": MODULE_PATH,
    "runner": MODULE_PATH,
    "dependency_lock": MODULE_PATH,
    "qualification": MODULE_PATH,
    "model_freeze": MODULE_PATH,
}


def artifact_hashes():
    return {name: analysis.file_hash(path) for name, path in ARTIFACT_PATHS.items()}


def requirement_revision(tasks):
    p0 = {task: "1" * 64 for task in tasks}
    p1 = {task: "2" * 64 for task in tasks}
    trigger = dict(analysis.TRIGGER_POLICY)
    return {
        "p0_sha256": p0,
        "p1_sha256": p1,
        "trigger_policy": trigger,
        "p0_hashes_sha256": analysis.canonical_hash(p0),
        "p1_hashes_sha256": analysis.canonical_hash(p1),
        "trigger_policy_sha256": analysis.canonical_hash(trigger),
    }


def plan():
    value = {
        "schema_version": 1,
        "study_id": "test-study",
        "primary_model": "small",
        "requested_model": "small",
        "primary_variant": "correction",
        "arms": ["B", "C"],
        "request_variants": ["static", "correction"],
        "dataset_revision": "test-revision",
        "dataset_manifest_sha256": analysis.canonical_hash(DATASET_MANIFEST),
        "schedule_sha256": "0" * 64,
        "runs_per_model": 120,
        "scheduled_runs_per_model": 120,
        "session_token_budget": analysis.SESSION_TOKEN_BUDGET,
        "qualification_sha256": analysis.file_hash(MODULE_PATH),
        "model_freeze_sha256": analysis.file_hash(MODULE_PATH),
        "primary_estimand": analysis.PRIMARY_ESTIMAND,
        "interaction_estimand": analysis.INTERACTION_ESTIMAND,
        "attrition_definition": "terminal_failure_or_oracle_unknown",
        "repositories": {
            f"r{index}": [f"r{index}-task-{task}" for task in range(3)]
            for index in range(1, 11)
        },
        "repeats": [0],
        "bootstrap_samples": 10_000,
        "bootstrap_seed": 7,
        "alpha": 0.05,
        "guardrail_margin": 0.05,
        "attrition_limit": 0.10,
        "minimum_baseline_correct_delivery_risk": 0.10,
        "minimum_method_correct_delivery_risk": 0.10,
    }
    value["requirement_revision"] = requirement_revision(
        [task for tasks in value["repositories"].values() for task in tasks]
    )
    value["schedule_sha256"] = analysis.canonical_hash(analysis.schedule_payload(value))
    return value


def freeze(value=None):
    value = value or plan()
    return {
        "schema_version": 1,
        "study_id": value["study_id"],
        "analysis_plan_sha256": analysis.canonical_hash(value),
        "frozen_at": "2026-10-13T00:00:00Z",
        "primary_model": value["primary_model"],
        "requested_model": value["requested_model"],
        "dataset_revision": "test-revision",
        "dataset_manifest_sha256": value["dataset_manifest_sha256"],
        "schedule_sha256": value["schedule_sha256"],
        "scheduled_runs_per_model": value["scheduled_runs_per_model"],
        "session_token_budget": value["session_token_budget"],
        "qualification_sha256": value["qualification_sha256"],
        "model_freeze_sha256": value["model_freeze_sha256"],
        "p0_hashes_sha256": value["requirement_revision"]["p0_hashes_sha256"],
        "p1_hashes_sha256": value["requirement_revision"]["p1_hashes_sha256"],
        "trigger_policy_sha256": value["requirement_revision"]["trigger_policy_sha256"],
        "artifact_sha256": artifact_hashes(),
    }


def rows(*, c_correct_loss: bool = False, c_error: bool = False):
    result = []
    plan_value = plan()
    plan_sha256 = analysis.canonical_hash(plan_value)
    freeze_value = freeze(plan_value)
    freeze_sha256 = analysis.canonical_hash(freeze_value)
    for repository, tasks in plan_value["repositories"].items():
        for task in tasks:
            for variant in ("static", "correction"):
                for arm in ("B", "C"):
                    repeat = 0
                    failed = arm == "C" and c_error
                    triggered = variant == "correction" and not failed
                    result.append(
                        {
                            "run_id": f"{task}-{variant}-{arm}-{repeat}",
                            "study_id": plan_value["study_id"],
                            "plan_sha256": plan_sha256,
                            "freeze_sha256": freeze_sha256,
                            "dataset_manifest_sha256": plan_value["dataset_manifest_sha256"],
                            "schedule_sha256": plan_value["schedule_sha256"],
                            "repository": repository,
                            "base_task_id": task,
                            "variant": variant,
                            "arm": arm,
                            "model": "small",
                            "repeat": repeat,
                            "terminal_status": "error" if failed else "valid",
                            "oracle_status": (
                                "incorrect"
                                if arm == "B" and task.endswith("task-0")
                                else "correct"
                            ),
                            "claimed_complete": None
                            if failed
                            else (arm == "B" or not c_correct_loss),
                            "mid_run_correction_requested": variant == "correction",
                            "mid_run_correction_triggered": triggered,
                            "mid_run_correction_acknowledged": triggered,
                            "mid_run_correction_trigger_event_sha256": "3" * 64
                            if triggered
                            else None,
                            "mid_run_correction_message_event_sha256": "4" * 64
                            if triggered
                            else None,
                        }
                    )
    return result


class ConfirmatoryAnalysisTests(unittest.TestCase):
    def analyze(self, row_values=None, plan_value=None, freeze_value=None, artifact_paths=None):
        plan_value = plan_value or plan()
        freeze_value = freeze_value or freeze(plan_value)
        return analysis.analyze(
            row_values or rows(),
            plan_value,
            freeze_value,
            DATASET_MANIFEST,
            artifact_paths or ARTIFACT_PATHS,
            expected_freeze_sha256=analysis.canonical_hash(freeze_value),
        )

    def test_joint_support_requires_benefit_and_guardrail(self):
        report = self.analyze()
        self.assertEqual(report["repositories"], 10)
        self.assertEqual(report["scheduled_rows"], 120)
        self.assertAlmostEqual(report["endpoints"]["false_ready"]["difference"], -1 / 3)
        self.assertEqual(
            report["endpoints"]["false_ready"]["repository_sign_test_p_two_sided"],
            0.001953125,
        )
        self.assertEqual(
            report["endpoints"]["false_ready"]["interaction"][
                "repository_sign_test_p_two_sided"
            ],
            None,
        )
        self.assertTrue(report["guardrail"]["passed"])
        self.assertTrue(report["decision"]["joint_method_support"])

    def test_coverage_loss_fails_joint_decision(self):
        report = self.analyze(rows(c_correct_loss=True))
        self.assertTrue(report["decision"]["false_ready_supported"])
        self.assertFalse(report["guardrail"]["passed"])
        self.assertFalse(report["decision"]["joint_method_support"])

    def test_attrition_suppresses_support(self):
        report = self.analyze(rows(c_error=True))
        self.assertFalse(report["operationally_eligible"])
        self.assertIsNone(report["decision"]["joint_method_support"])
        self.assertEqual(report["decision"]["status"], "operationally_inconclusive")
        self.assertEqual(report["attrition"]["correction:C"]["missing_claim"], 0)
        self.assertEqual(report["attrition"]["correction:C"]["claim_not_applicable"], 30)
        self.assertEqual(report["attrition"]["correction:C"]["trigger_failures"], 30)

    def test_rejects_missing_arm(self):
        incomplete = [
            row
            for row in rows()
            if not (row["base_task_id"] == "r1-task-0" and row["arm"] == "C")
        ]
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(incomplete)

    def test_rejects_unmatched_repeats(self):
        incomplete = [
            row
            for row in rows()
            if not (
                row["base_task_id"] == "r1-task-0"
                and row["variant"] == "correction"
                and row["arm"] == "C"
            )
        ]
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(incomplete)

    def test_rejects_task_missing_from_both_arms(self):
        incomplete = [row for row in rows() if row["base_task_id"] != "r1-task-0"]
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(incomplete)

    def test_rejects_repository_mismatch(self):
        mismatched = rows()
        mismatched[0]["repository"] = "wrong"
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(mismatched)

    def test_sign_test_handles_ties_and_mixed_signs(self):
        self.assertEqual(analysis.exact_two_sided_sign_p([-1] * 6), 0.03125)
        self.assertEqual(analysis.exact_two_sided_sign_p([1] * 6), 0.03125)
        self.assertEqual(analysis.exact_two_sided_sign_p([-1, -1, -1, 1, 1, 1]), 1.0)
        self.assertEqual(analysis.exact_two_sided_sign_p([0, -1, -1, 1]), 1.0)

    def test_schedule_is_balanced_120_cells(self):
        self.assertEqual(len(analysis.schedule_payload(plan())), 120)
        self.assertEqual(
            {entry["variant"] for entry in analysis.schedule_payload(plan())},
            {"static", "correction"},
        )

    def test_untriggered_correction_stays_in_itt_and_counts_attrition(self):
        values = rows()
        for row in [
            row
            for row in values
            if row["variant"] == "correction" and row["arm"] == "C"
        ][:7]:
            row["mid_run_correction_triggered"] = False
            row["mid_run_correction_acknowledged"] = False
            row["mid_run_correction_trigger_event_sha256"] = None
            row["mid_run_correction_message_event_sha256"] = None
        report = self.analyze(values)
        self.assertEqual(report["scheduled_rows"], 120)
        self.assertEqual(report["attrition"]["correction:C"]["trigger_failures"], 7)
        self.assertIsNone(report["decision"]["joint_method_support"])
        self.assertEqual(report["decision"]["status"], "operationally_inconclusive")

    def test_requires_lifecycle_fields(self):
        values = rows()
        values[0].pop("mid_run_correction_requested")
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(values)

    def test_confirmatory_plan_rejects_too_few_bootstrap_draws(self):
        invalid = plan()
        invalid["bootstrap_samples"] = 1
        with self.assertRaises(analysis.AnalysisError):
            analysis.load_plan_object(invalid)

    def test_rejects_modified_plan_after_freeze(self):
        modified = plan()
        modified["guardrail_margin"] = 0.06
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(rows(), modified, freeze())

    def test_absolute_utility_floor_blocks_always_blocking_method(self):
        blocked = rows()
        for row in blocked:
            row["claimed_complete"] = row["arm"] == "B" and row["oracle_status"] == "incorrect"
        report = self.analyze(blocked)
        self.assertTrue(report["decision"]["false_ready_supported"])
        self.assertFalse(report["guardrail"]["passed"])
        self.assertFalse(report["decision"]["joint_method_support"])

    def test_oracle_unknown_is_conservative_and_auditable(self):
        uncertain = rows()
        uncertain[0]["oracle_status"] = "unknown"
        report = self.analyze(uncertain)
        self.assertEqual(report["attrition"]["static:B"]["oracle_unknown"], 1)
        self.assertGreater(report["endpoints"]["false_ready"]["B"], 0)

    def test_rejects_six_task_plan(self):
        too_small = plan()
        too_small["repositories"] = {
            f"r{index}": [f"r{index}-task"] for index in range(1, 7)
        }
        with self.assertRaises(analysis.AnalysisError):
            analysis.load_plan_object(too_small)

    def test_rejects_more_than_ten_repositories(self):
        invalid = plan()
        invalid["repositories"]["r11"] = invalid["repositories"]["r10"]
        del invalid["repositories"]["r10"]
        invalid["repositories"]["r11"].append("r11-task-extra")
        with self.assertRaises(analysis.AnalysisError):
            analysis.load_plan_object(invalid)

    def test_rejects_non_three_task_repository(self):
        invalid = plan()
        invalid["repositories"]["r1"].pop()
        invalid["repositories"]["r2"].append("replacement")
        with self.assertRaises(analysis.AnalysisError):
            analysis.load_plan_object(invalid)

    def test_analyze_rejects_duplicate_cell(self):
        duplicated = rows()
        duplicate = dict(duplicated[0])
        duplicate["run_id"] = "different-run-id"
        duplicated.append(duplicate)
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(duplicated)

    def test_rejects_inconsistent_terminal_and_claim(self):
        malformed = rows()
        malformed[0]["terminal_status"] = "error"
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(malformed)

    def test_rejects_unpublished_freeze_digest(self):
        with self.assertRaises(analysis.AnalysisError):
            analysis.analyze(
                rows(),
                plan(),
                freeze(),
                DATASET_MANIFEST,
                ARTIFACT_PATHS,
                expected_freeze_sha256="f" * 64,
            )

    def test_rejects_result_not_bound_to_dataset_manifest(self):
        unbound = rows()
        unbound[0]["dataset_manifest_sha256"] = "3" * 64
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(unbound)

    def test_rejects_opaque_schedule_digest(self):
        invalid = plan()
        invalid["schedule_sha256"] = "6" * 64
        frozen = freeze(invalid)
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(rows(), invalid, frozen)

    def test_rejects_tampered_requirement_revision_freeze_binding(self):
        frozen = freeze()
        frozen["p1_hashes_sha256"] = "9" * 64
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(rows(), plan(), frozen)

    def test_rejects_modified_dataset_manifest(self):
        with self.assertRaises(analysis.AnalysisError):
            analysis.analyze(
                rows(),
                plan(),
                freeze(),
                {**DATASET_MANIFEST, "revision": "modified"},
                ARTIFACT_PATHS,
                expected_freeze_sha256=analysis.canonical_hash(freeze()),
            )

    def test_rejects_unfrozen_runner(self):
        modified_artifacts = {**ARTIFACT_PATHS, "runner": Path(__file__).resolve()}
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(artifact_paths=modified_artifacts)

    def test_rejects_tampered_qualification_artifact(self):
        modified_artifacts = {**ARTIFACT_PATHS, "qualification": Path(__file__).resolve()}
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(artifact_paths=modified_artifacts)

    def test_rejects_tampered_model_freeze_artifact(self):
        modified_artifacts = {**ARTIFACT_PATHS, "model_freeze": Path(__file__).resolve()}
        with self.assertRaises(analysis.AnalysisError):
            self.analyze(artifact_paths=modified_artifacts)

    def test_rejects_tiny_utility_floor(self):
        invalid = plan()
        invalid["minimum_method_correct_delivery_risk"] = 1e-6
        with self.assertRaises(analysis.AnalysisError):
            analysis.load_plan_object(invalid)


if __name__ == "__main__":
    unittest.main()
