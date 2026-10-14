import hashlib
import json
import unittest

from ase_disjoint_acquisition import oracle_count, public_projection


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
