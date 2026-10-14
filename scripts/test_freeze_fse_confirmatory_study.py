import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("freeze_fse_confirmatory_study.py")
QUALIFICATION_PATH = MODULE_PATH
MODEL_FREEZE_PATH = MODULE_PATH
SPEC = importlib.util.spec_from_file_location("freeze_fse_confirmatory_study", MODULE_PATH)
assert SPEC and SPEC.loader
freeze_study = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(freeze_study)


def manifest():
    return {
        "schema_version": 1,
        "source": {"revision": "a" * 40},
        "run_plan": {
            "repositories": 10,
            "tasks": 30,
            "arms": ["B", "C"],
            "request_variants": ["static", "correction"],
            "repeat_labels": [0],
            "runs_per_model": 120,
            "requirement_revision": {
                "trigger_policy": dict(freeze_study.analysis.TRIGGER_POLICY),
            }
        },
        "tasks": [
            {
                "repo": f"owner/repo-{repository}",
                "instance_id": f"task-{repository}-{task}",
                "problem_statement_sha256": "1" * 64,
                "p1_sha256": "2" * 64,
            }
            for repository in range(10)
            for task in range(3)
        ],
    }


class FreezeStudyTests(unittest.TestCase):
    def test_builds_bound_120_cell_study(self):
        plan, frozen, receipt = freeze_study.build(
            manifest(),
            study_id="study-1",
            model="ollama/llama3.2:3b",
            runner=MODULE_PATH,
            dependency_lock=MODULE_PATH,
            qualification=QUALIFICATION_PATH,
            model_freeze=MODEL_FREEZE_PATH,
            frozen_at="2026-10-13T00:00:00Z",
        )
        self.assertEqual(len(freeze_study.analysis.schedule_payload(plan)), 120)
        self.assertEqual(plan["request_variants"], ["static", "correction"])
        self.assertEqual(plan["repeats"], [0])
        self.assertEqual(plan["scheduled_runs_per_model"], 120)
        self.assertEqual(plan["session_token_budget"], 1_000_000)
        self.assertEqual(
            set(frozen["artifact_sha256"]),
            {"analysis_script", "runner", "dependency_lock", "qualification", "model_freeze"},
        )
        self.assertEqual(frozen["qualification_sha256"], plan["qualification_sha256"])
        self.assertEqual(frozen["model_freeze_sha256"], plan["model_freeze_sha256"])
        self.assertEqual(frozen["p1_hashes_sha256"], plan["requirement_revision"]["p1_hashes_sha256"])
        self.assertEqual(frozen["analysis_plan_sha256"], freeze_study.analysis.canonical_hash(plan))
        self.assertEqual(receipt["freeze_sha256"], freeze_study.analysis.canonical_hash(frozen))

    def test_write_once_refuses_existing_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "freeze"
            freeze_study.write_once(target, {"receipt.json": {"ok": True}})
            with self.assertRaises(FileExistsError):
                freeze_study.write_once(target, {"receipt.json": {"ok": True}})

    def test_rejects_wrong_repository_shape(self):
        invalid = manifest()
        invalid["tasks"].pop()
        with self.assertRaises(freeze_study.analysis.AnalysisError):
            freeze_study.build(
                invalid,
                study_id="study-1",
                model="small",
                runner=MODULE_PATH,
                dependency_lock=MODULE_PATH,
                qualification=QUALIFICATION_PATH,
                model_freeze=MODEL_FREEZE_PATH,
                frozen_at="2026-10-13T00:00:00Z",
            )

    def test_rejects_manifest_schedule_mismatch(self):
        invalid = manifest()
        invalid["run_plan"]["repeat_labels"] = [0, 1]
        with self.assertRaises(freeze_study.analysis.AnalysisError):
            freeze_study.build(
                invalid,
                study_id="study-1",
                model="small",
                runner=MODULE_PATH,
                dependency_lock=MODULE_PATH,
                qualification=QUALIFICATION_PATH,
                model_freeze=MODEL_FREEZE_PATH,
                frozen_at="2026-10-13T00:00:00Z",
            )

    def test_fails_closed_without_p1_hashes(self):
        invalid = manifest()
        for task in invalid["tasks"]:
            task.pop("p1_sha256")
        with self.assertRaises(freeze_study.analysis.AnalysisError):
            freeze_study.build(
                invalid,
                study_id="study-1",
                model="small",
                runner=MODULE_PATH,
                dependency_lock=MODULE_PATH,
                qualification=QUALIFICATION_PATH,
                model_freeze=MODEL_FREEZE_PATH,
                frozen_at="2026-10-13T00:00:00Z",
            )

    def test_fails_closed_without_exact_trigger_policy(self):
        invalid = manifest()
        invalid["run_plan"]["requirement_revision"].pop("trigger_policy")
        with self.assertRaises(freeze_study.analysis.AnalysisError):
            freeze_study.build(
                invalid,
                study_id="study-1",
                model="small",
                runner=MODULE_PATH,
                dependency_lock=MODULE_PATH,
                qualification=QUALIFICATION_PATH,
                model_freeze=MODEL_FREEZE_PATH,
                frozen_at="2026-10-13T00:00:00Z",
            )

    def test_derives_manifest_trigger_policy_without_guessing(self):
        value = manifest()
        revision = value["run_plan"]["requirement_revision"]
        revision.pop("trigger_policy")
        revision.update(
            {
                "injection_trigger": (
                    "ExecutionFinished with exit code 0 and any non-empty affected_paths"
                ),
                "production_path_filter": False,
                "acknowledgement_event": "requirement_revision_ack",
                "acknowledgement_required": True,
            }
        )
        plan, frozen, _ = freeze_study.build(
            value,
            study_id="study-1",
            model="small",
            runner=MODULE_PATH,
            dependency_lock=MODULE_PATH,
            qualification=QUALIFICATION_PATH,
            model_freeze=MODEL_FREEZE_PATH,
            frozen_at="2026-10-13T00:00:00Z",
        )
        self.assertEqual(
            frozen["trigger_policy_sha256"],
            plan["requirement_revision"]["trigger_policy_sha256"],
        )


if __name__ == "__main__":
    unittest.main()
