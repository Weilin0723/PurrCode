"""Verify immutable oracle inputs without downloading hidden study material."""

import argparse
import json
import sys
import tempfile
import types
import unittest
from datetime import datetime
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_swebench_live_oracle as oracle


class OracleInputsTests(unittest.TestCase):
    def test_revision_is_pinned_and_missing_instances_fail_closed(self):
        rows = [{"instance_id": "selected", "patch": "synthetic-gold",
                 "created_at": datetime(2026, 10, 14)},
                {"instance_id": "other", "patch": "synthetic-other"}]
        loader = Mock(return_value=rows)
        args = argparse.Namespace(dataset_name="fixture", split="verified",
                                  dataset_revision="immutable-revision", instance_ids=["selected"])
        with tempfile.TemporaryDirectory() as temporary, patch.dict(
            sys.modules, {"datasets": types.SimpleNamespace(load_dataset=loader)}
        ):
            target = Path(temporary) / "operator.json"
            oracle.materialize_pinned_dataset(args, target)
            loader.assert_called_once_with("fixture", split="verified", revision="immutable-revision")
            expected = dict(rows[0], created_at="2026-10-14T00:00:00")
            self.assertEqual(json.loads(target.read_text()), [expected])
            args.instance_ids = ["absent"]
            with self.assertRaisesRegex(ValueError, "omits selected"):
                oracle.materialize_pinned_dataset(args, target)


if __name__ == "__main__":
    unittest.main()
