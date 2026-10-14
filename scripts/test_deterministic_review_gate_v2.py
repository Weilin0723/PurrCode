import unittest

from scripts.deterministic_review_gate_v2 import verdict


class DeterministicGateTests(unittest.TestCase):
    def test_current_complete_evidence_is_ready_and_stale_digest_blocks(self):
        prompt = {
            "request": {"revision": 2, "requirements": [{"id": "r1"}]},
            "artifact": {"digest": "tree-b"},
            "evidence": {
                "revision": 2,
                "artifact_digest": "tree-b",
                "observations": [{"requirement_id": "r1", "status": "PASS"}],
            },
        }
        self.assertEqual(verdict(prompt), ("READY", []))
        prompt["evidence"]["artifact_digest"] = "tree-a"
        self.assertEqual(verdict(prompt)[0], "BLOCKED")


if __name__ == "__main__":
    unittest.main()
