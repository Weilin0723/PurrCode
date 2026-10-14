import tempfile
import unittest
from pathlib import Path

from scripts.local_review_probe import (
    CONDITIONS,
    DEFAULT_FIXTURE,
    OutputExistsError,
    build_prompt,
    build_schedule,
    create_output_dir,
    load_fixture,
    parse_verdict,
    summarize,
)


class LocalReviewProbeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = load_fixture(DEFAULT_FIXTURE)

    def test_prompt_excludes_fixture_labels_and_identifiers(self):
        for case in self.cases:
            for condition in CONDITIONS:
                prompt = build_prompt(condition, case)
                self.assertNotIn(case["id"], prompt)
                self.assertNotIn(case.get("pair_id", ""), prompt)
                self.assertNotIn("expected_verdict", prompt)
                self.assertNotIn("label_rationale", prompt)
                self.assertNotIn("pair_id", prompt)

    def test_invalid_response_is_not_rounded_to_a_verdict(self):
        self.assertEqual(parse_verdict({"done": True, "response": "READY\n"}), "READY")
        self.assertEqual(parse_verdict({"done": True, "response": " BLOCKED "}), "BLOCKED")
        for response in (
            {},
            {"done": False, "response": "READY"},
            {"done": True, "response": "READY because it looks good"},
            {"done": True, "response": "{}"},
            {"done": True, "response": 0},
        ):
            with self.subTest(response=response):
                with self.assertRaises(Exception):
                    parse_verdict(response)

    def test_schedule_has_48_calls_and_repeats_tasks_without_treating_them_as_new_tasks(self):
        schedule = build_schedule(self.cases, schedule_seed=17)
        self.assertEqual(len(schedule), 48)
        self.assertEqual({item["condition"] for item in schedule}, set(CONDITIONS))
        self.assertEqual({item["seed"] for item in schedule}, {17, 29})
        self.assertEqual(len({item["case_id"] for item in schedule}), 12)

        results = [
            {
                "condition": "plain",
                "case_id": self.cases[0]["id"],
                "status": "ok",
                "expected": "READY",
                "observed": "READY",
                "tokens": {"total": 12},
                "latency_ms": 10.0,
            },
            {
                "condition": "plain",
                "case_id": self.cases[0]["id"],
                "status": "transport_error",
                "expected": "READY",
                "observed": None,
                "tokens": {"total": None},
                "latency_ms": 30.0,
            },
        ]
        summary = summarize(results, self.cases, schedule_size=2)
        self.assertEqual(summary["unique_tasks"], 12)
        self.assertEqual(summary["by_condition"]["plain"]["coverage"]["valid_responses"], 1)
        self.assertIn("not independent", summary["independence_note"])

    def test_existing_output_directory_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "already-there"
            path.mkdir()
            with self.assertRaises(OutputExistsError):
                create_output_dir(path)


if __name__ == "__main__":
    unittest.main()
