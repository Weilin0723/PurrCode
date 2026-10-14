import copy
import unittest

from scripts.local_review_probe import InvalidResponse
from scripts.local_review_probe_v2 import (
    CONDITIONS,
    build_prompt_v2,
    parse_verdict_v2,
    summarize_v2,
    validate_prompt_data,
)


def prompt_data():
    return {
        "request": {
            "revision": 2,
            "text": "Revision 2: return 404 and preserve the error key.",
            "requirements": [
                {"id": "r1", "kind": "behavior", "text": "Return 404."},
                {"id": "r2", "kind": "preservation", "text": "Keep error key."},
            ],
        },
        "artifact": {
            "revision": 2,
            "digest": "tree-b",
            "facts": [{"requirement_id": "r1", "detail": "status is 404"}],
            "preservation": [{"requirement_id": "r2", "detail": "error key remains"}],
        },
        "evidence": {
            "revision": 2,
            "artifact_digest": "tree-b",
            "observations": [
                {"requirement_id": "r1", "status": "PASS", "detail": "status == 404"},
                {"requirement_id": "r2", "status": "PASS", "detail": "error key exists"},
            ],
        },
    }


class LocalReviewProbeV2Tests(unittest.TestCase):
    def test_prompt_excludes_scorer_metadata(self):
        case = {
            "id": "secret-case-id",
            "pair_id": "secret-pair",
            "expected_verdict": "ready",
            "label_rationale": "secret rationale",
            "prompt_data": validate_prompt_data(prompt_data(), "case.prompt_data"),
        }
        for condition in CONDITIONS:
            text = build_prompt_v2(condition, case)
            self.assertNotIn("secret-case-id", text)
            self.assertNotIn("secret-pair", text)
            self.assertNotIn("expected_verdict", text)
            self.assertNotIn("secret rationale", text)

    def test_prompt_data_rejects_leaks_unknown_observations_and_duplicate_requirements(self):
        leaked = prompt_data()
        leaked["expected_verdict"] = "ready"
        with self.assertRaises(ValueError):
            validate_prompt_data(leaked, "data")

        unknown = prompt_data()
        unknown["evidence"]["observations"][0]["requirement_id"] = "missing"
        with self.assertRaises(ValueError):
            validate_prompt_data(unknown, "data")

        duplicate = prompt_data()
        duplicate["request"]["requirements"][1]["id"] = "r1"
        with self.assertRaises(ValueError):
            validate_prompt_data(duplicate, "data")

    def test_strict_json_parser_accepts_only_the_application_schema(self):
        self.assertEqual(
            parse_verdict_v2({"done": True, "response": '{"verdict":"READY"}'}),
            "READY",
        )
        invalid = (
            {},
            {"done": False, "response": '{"verdict":"READY"}'},
            {"done": True, "response": '```json\n{"verdict":"READY"}\n```'},
            {"done": True, "response": '{"verdict":"ready"}'},
            {"done": True, "response": '{"verdict":"READY","extra":1}'},
            {"done": True, "response": '{"verdict":"READY","verdict":"BLOCKED"}'},
            {"done": True, "response": 'null'},
        )
        for response in invalid:
            with self.subTest(response=response):
                with self.assertRaises(InvalidResponse):
                    parse_verdict_v2(response)

    def test_invalid_responses_stay_in_scheduled_denominators(self):
        cases = [
            {"id": "ready", "expected_verdict": "ready"},
            {"id": "blocked", "expected_verdict": "blocked"},
        ]
        results = []
        for condition in CONDITIONS:
            results.extend(
                [
                    {
                        "condition": condition,
                        "case_id": "ready",
                        "status": "invalid_response",
                        "expected": "READY",
                        "observed": None,
                        "tokens": {"total": 4},
                        "latency_ms": 10.0,
                    },
                    {
                        "condition": condition,
                        "case_id": "blocked",
                        "status": "ok",
                        "expected": "BLOCKED",
                        "observed": "BLOCKED",
                        "tokens": {"total": 4},
                        "latency_ms": 10.0,
                    },
                ]
            )
        summary = summarize_v2(results, cases)
        row = summary["by_condition"]["revision_rubric_json"]
        self.assertEqual(row["format_coverage"], 0.5)
        self.assertEqual(row["scheduled_accuracy"], 0.5)
        self.assertEqual(row["ready_recall_scheduled"], 0.0)
        self.assertEqual(row["blocked_recall_scheduled"], 1.0)
        self.assertFalse(row["meets_engineering_bar"])

    def test_missing_observation_stale_revision_and_digest_remain_prompt_visible(self):
        case = {"prompt_data": prompt_data()}
        missing = copy.deepcopy(case)
        missing["prompt_data"]["evidence"]["observations"].pop()
        stale_revision = copy.deepcopy(case)
        stale_revision["prompt_data"]["evidence"]["revision"] = 1
        stale_digest = copy.deepcopy(case)
        stale_digest["prompt_data"]["evidence"]["artifact_digest"] = "tree-a"
        for value, marker in (
            (missing, '"requirement_id":"r1"'),
            (stale_revision, '"revision":1'),
            (stale_digest, '"artifact_digest":"tree-a"'),
        ):
            text = build_prompt_v2("revision_rubric_json", value)
            self.assertIn(marker, text)


if __name__ == "__main__":
    unittest.main()
