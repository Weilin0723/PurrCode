import copy
import unittest

from scripts.local_review_probe_v3 import (
    DEFAULT_FIXTURE,
    deterministic_verdict,
    exact_two_sided_sign_test,
    load_fixture_v3,
    summarize_v3,
)


def case(case_id, pair_id, expected):
    revision = 2
    digest = f"tree-{pair_id}"
    status = "PASS" if expected == "ready" else "FAIL"
    return {
        "id": case_id,
        "pair_id": pair_id,
        "expected_verdict": expected,
        "prompt_data": {
            "request": {
                "revision": revision,
                "text": "request",
                "requirements": [{"id": "r1", "kind": "behavior", "text": "works"}],
            },
            "artifact": {"revision": revision, "digest": digest, "facts": [], "preservation": []},
            "evidence": {
                "revision": revision,
                "artifact_digest": digest,
                "observations": [{"requirement_id": "r1", "status": status, "detail": "check"}],
            },
        },
    }


class LocalReviewProbeV3Tests(unittest.TestCase):
    def test_exact_sign_test(self):
        self.assertEqual(exact_two_sided_sign_test(6, 0), 0.03125)
        self.assertEqual(exact_two_sided_sign_test(4, 0), 0.125)
        self.assertIsNone(exact_two_sided_sign_test(0, 0))

    def test_repeated_calls_collapse_to_pair(self):
        cases = [case("ready", "pair", "ready"), case("blocked", "pair", "blocked")]
        rows = []
        for item in cases:
            for seed in (17, 29):
                rows.append(
                    {
                        "case_id": item["id"],
                        "condition": "revision_rubric_json",
                        "seed": seed,
                        "status": "ok",
                        "expected": item["expected_verdict"].upper(),
                        "observed": "READY",
                    }
                )
        summary = summarize_v3(rows, cases, "llama3.2:3b")
        self.assertEqual(summary["independent_base_task_clusters"], 1)
        self.assertEqual(summary["cluster_comparison"]["wins"], 1)
        self.assertEqual(summary["cluster_comparison"]["losses"], 0)

    def test_artifact_revision_mismatch_blocks(self):
        data = case("ready", "pair", "ready")["prompt_data"]
        stale = copy.deepcopy(data)
        stale["artifact"]["revision"] -= 1
        self.assertEqual(deterministic_verdict(data)[0], "READY")
        self.assertEqual(deterministic_verdict(stale)[0], "BLOCKED")

    def test_frozen_fixture_labels_match_gate_and_pair_shape(self):
        cases = load_fixture_v3(DEFAULT_FIXTURE)
        pairs = {item["pair_id"] for item in cases}
        self.assertEqual(len(cases), 32)
        self.assertEqual(len(pairs), 16)
        self.assertTrue(
            all(
                deterministic_verdict(item["prompt_data"])[0]
                == item["expected_verdict"].upper()
                for item in cases
            )
        )


if __name__ == "__main__":
    unittest.main()
