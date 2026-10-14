import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import swebench_live_oracle_preflight as module


class PublicEnvironmentMetadataTests(unittest.TestCase):
    def test_package_query_has_no_mount_network_or_provider_credential(self):
        results = [SimpleNamespace(returncode=127, stdout="", stderr="missing interpreter"),
                   SimpleNamespace(returncode=0, stdout=json.dumps({"packages": {"torch": "2.5.1+cpu"},
                       "python_version": "3.11.9", "prefix": "/opt/venv", "executable": "/opt/venv/bin/python"})),
                   SimpleNamespace(returncode=0, stdout='["public/image@sha256:verified"]')]
        with patch.dict(module.os.environ, {"DEEPSEEK_API_KEY": "must-not-reach-container"}), \
             patch.object(module.subprocess, "run", side_effect=results) as run:
            report = module.public_environment_metadata("public/image:latest", module.clean_environment(Path("/tmp/hf")))
        self.assertEqual(report["status"], "observed")
        self.assertEqual(report["packages"]["torch"], "2.5.1+cpu")
        self.assertEqual(report["registry_repo_digests"], ["public/image@sha256:verified"])
        for call in run.call_args_list:
            self.assertNotIn("DEEPSEEK_API_KEY", call.kwargs["env"])
            self.assertNotIn("--mount", call.args[0])
            self.assertNotIn("--volume", call.args[0])
            if "run" in call.args[0]:
                self.assertEqual(call.args[0][3:5], ["--network", "none"])

    def test_missing_interpreters_are_unavailable_not_observed(self):
        with patch.object(module.subprocess, "run", return_value=SimpleNamespace(returncode=127, stdout="")):
            report = module.public_environment_metadata("public/image:latest", {})
        self.assertEqual(report["status"], "unavailable")
        self.assertEqual(len(report["attempts"]), 4)


if __name__ == "__main__":
    unittest.main()
