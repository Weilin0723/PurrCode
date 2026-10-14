import copy
import hashlib
import json
import unittest
from pathlib import Path

from ase_disjoint_acquisition import select, oracle_count, public_projection


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
        self.assertEqual(result, select(prior, {index: public_projection(row) for index, row in rows.items()}))
        self.assertEqual(prior, original)
        self.assertEqual(result["selection"]["repository_order"], names[10:])
        self.assertEqual(len(result["tasks"]), 30)
        serialized = json.dumps(result)
        for hidden in ("hidden-f2p", "hidden-p2p", "private-solution", "private-tests"):
            self.assertNotIn(hidden, serialized)
        self.assertEqual([r["dataset_row"] for r in result["tasks"]], list(range(30, 60)))


class PublicProjectionTests(unittest.TestCase):
    def test_projection_preserves_eligibility_and_hashes_without_solution_or_hidden_names(self):
        row = dict(instance_id="example__repo-1", repo="example/repo", base_commit="abc",
                   pull_number=1, difficulty={"lines": 5}, test_cmds=["pytest"],
                   log_parser="pytest", problem_statement="Public issue text",
                   patch="private-reference-solution", test_patch="private-oracle-body",
                   FAIL_TO_PASS=["private-failing-test-name"],
                   PASS_TO_PASS=["private-passing-test-name"])
        projected = public_projection(row)
        encoded = json.dumps(projected)
        for private in (row["patch"], row["test_patch"], *row["FAIL_TO_PASS"], *row["PASS_TO_PASS"]):
            self.assertNotIn(private, encoded)
        self.assertEqual(projected["patch_sha256"], hashlib.sha256(row["patch"].encode()).hexdigest())
        for label in ("FAIL_TO_PASS", "PASS_TO_PASS"):
            self.assertEqual(oracle_count(row, label), oracle_count(projected, label))
        self.assertEqual(projected["problem_statement"], row["problem_statement"])


if __name__ == "__main__":
    unittest.main()
