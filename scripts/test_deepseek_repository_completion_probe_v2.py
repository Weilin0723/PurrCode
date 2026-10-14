import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import deepseek_repository_completion_probe as core
from scripts import deepseek_repository_completion_probe_v2 as probe
from scripts.test_deepseek_repository_completion_probe import fake_candidates


class DeepSeekRepositoryCompletionProbeV2Tests(unittest.TestCase):
    def test_prepare_changes_only_the_cap_and_records_repair_provenance(self) -> None:
        candidates = fake_candidates()
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "run"
            with patch.object(core.base, "build_candidates", return_value=candidates):
                report = probe.prepare(output)
            freeze = core.read_object(output / "freeze.json", "freeze")
            manifest = core.read_object(output / "manifest.json", "manifest")
            requests = core.read_jsonl(output / "requests.jsonl", "bundle")
            self.assertTrue(report["preflight_ok"])
            self.assertEqual(report["max_output_tokens"], 256)
            self.assertEqual(freeze["study_id"], probe.STUDY_ID)
            self.assertEqual(freeze["request_parameters"]["max_output_tokens"], 256)
            self.assertEqual(manifest["analysis_role"], "outcome_aware_operational_repair")
            self.assertEqual(len(requests), 144)
            self.assertEqual({row["payload"]["max_output_tokens"] for row in requests}, {256})
            self.assertEqual(freeze["model_calls"], 0)
            self.assertIn(str(probe.PLAN_PATH), freeze["dependency_hashes"])
            self.assertNotIn(
                "docs/research/repository-completion-deepseek-v1-plan.md",
                freeze["dependency_hashes"],
            )


if __name__ == "__main__":
    unittest.main()
