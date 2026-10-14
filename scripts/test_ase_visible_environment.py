import json
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import ase_visible_environment as module


class VisibleEnvironmentTests(unittest.TestCase):
    def test_failed_collection_is_preserved_and_blocks_qualification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            checkout = root / "checkout"
            checkout.mkdir()
            manifest = {"tasks": [{"instance_id": "task", "base_commit": "abc"}]}
            def result(arguments, **kwargs):
                return SimpleNamespace(returncode=2 if "--collect-only" in arguments else 0,
                                       stdout="missing public dependency", stderr="")
            with patch.object(module, "verify_checkout", return_value=checkout), \
                 patch.object(module.subprocess, "run", side_effect=result), \
                 patch.object(module.subprocess, "check_output", return_value=""):
                with self.assertRaisesRegex(RuntimeError, "collection failed"):
                    module.provision(manifest, "task", root / "cache", root / "environment")
            receipt = json.loads((root / "environment/visible-environment.json").read_text())
            self.assertEqual(receipt["status"], "failed")
            self.assertEqual(receipt["public_test_collection"]["exit_code"], 2)

    def test_public_development_dependencies_and_project_ruff_pin(self):
        self.assertEqual(module.public_test_dependencies({"dependency-groups": {"dev": ["networkx"]}}), ["networkx"])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".pre-commit-config.yaml").write_text(
                'repos:\n- repo: https://github.com/astral-sh/ruff-pre-commit\n  rev: v0.7.2\n  hooks:\n  - id: ruff\n')
            self.assertEqual(module.public_ruff_version(root), "0.7.2")

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
                 patch.object(module.subprocess, "run", return_value=SimpleNamespace(returncode=0, stdout="1 test collected", stderr="")) as run, \
                 patch.object(module.subprocess, "check_output", return_value="pytest==8.3.5\n"):
                module.provision(manifest, "task", root / "cache", root / "environment")
            for call in run.call_args_list:
                self.assertNotIn("DEEPSEEK_API_KEY", call.kwargs["env"])
                self.assertIsInstance(call.args[0], list)
            self.assertTrue(any(call.args[0][-1] == str(checkout) + "[test]" for call in run.call_args_list))
            self.assertEqual(run.call_args_list[-1].args[0][-2:], ["--collect-only", "-q"])
            receipt = json.loads((root / "environment/visible-environment.json").read_text())
            self.assertEqual(receipt["status"], "passed")
            self.assertEqual(receipt["public_test_collection"]["exit_code"], 0)
            self.assertNotIn("must-not-reach-installer", json.dumps(receipt))


if __name__ == "__main__":
    unittest.main()
