import copy
import json
import unittest
from pathlib import Path

from ase_disjoint_acquisition import select


class DisjointAcquisitionTests(unittest.TestCase):
    def test_order_exclusion_and_hidden_field_confinement(self):
        root = Path(__file__).resolve().parents[1]
        prior = json.loads((root / "benchmarks/research/swebench-live-v1/manifest.json").read_text())
        rows = {}
        names = prior["selection"]["repository_order"] + [f"test/repo-{i}" for i in range(10)]
        for group, repo in enumerate(names):
            for issue in range(3):
                index = group * 3 + issue
                rows[index] = dict(instance_id=f"test-{index}", repo=repo, base_commit="a" * 40,
                                   pull_number=issue, difficulty={"lines": 1}, test_cmds=["pytest"],
                                   log_parser="synthetic", FAIL_TO_PASS=["hidden-f2p"],
                                   PASS_TO_PASS=["hidden-p2p"], problem_statement="Public issue",
                                   patch="private-solution", test_patch="private-tests")
        original = copy.deepcopy(prior)
        result = select(prior, rows)
        self.assertEqual(prior, original)
        self.assertEqual(result["selection"]["repository_order"], names[10:])
        self.assertEqual(len(result["tasks"]), 30)
        serialized = json.dumps(result)
        for hidden in ("hidden-f2p", "hidden-p2p", "private-solution", "private-tests"):
            self.assertNotIn(hidden, serialized)
        self.assertEqual([r["dataset_row"] for r in result["tasks"]], list(range(30, 60)))


if __name__ == "__main__":
    unittest.main()
