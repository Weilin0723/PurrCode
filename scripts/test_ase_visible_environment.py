import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import ase_visible_environment as module


class VisibleEnvironmentTests(unittest.TestCase):
    def test_public_test_groups_are_resolved_and_cyclic_or_option_inputs_fail(self):
        metadata = {"dependency-groups": {"test": ["pytest", {"include-group": "fixtures"}],
                                          "fixtures": ["pytest-mock", "pytest"]}}
        self.assertEqual(module.public_test_dependencies(metadata), ["pytest", "pytest-mock"])
        with self.assertRaises(ValueError):
            module.public_test_dependencies({"dependency-groups": {"test": [{"include-group": "test"}]}})
        with self.assertRaises(ValueError):
            module.public_test_dependencies({"dependency-groups": {"test": ["--index-url=untrusted"]}})

    def test_only_public_test_extras_are_installed_without_provider_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            checkout = root / "checkout"
            checkout.mkdir()
            (checkout / "pyproject.toml").write_text(
                '[project.optional-dependencies]\ntest = ["pytest"]\nprivate = ["secret-package"]\n')
            manifest = {"tasks": [{"instance_id": "task", "base_commit": "abc"}]}
            with patch.object(module, "verify_checkout", return_value=checkout), \
                 patch.dict(module.os.environ, {"DEEPSEEK_API_KEY": "must-not-reach-installer"}), \
                 patch.object(module.subprocess, "run") as run, \
                 patch.object(module.subprocess, "check_output", return_value="pytest==8.3.5\n"):
                module.provision(manifest, "task", root / "cache", root / "environment")
            for call in run.call_args_list:
                self.assertNotIn("DEEPSEEK_API_KEY", call.kwargs["env"])
                self.assertIsInstance(call.args[0], list)
            self.assertEqual(run.call_args_list[-1].args[0][-1], str(checkout) + "[test]")
            receipt = json.loads((root / "environment/visible-environment.json").read_text())
            self.assertEqual(receipt["status"], "passed")
            self.assertNotIn("must-not-reach-installer", json.dumps(receipt))


if __name__ == "__main__":
    unittest.main()
