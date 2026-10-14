#!/usr/bin/env python3
"""Focused, credential-free checks for the SWE-bench-Live acquisition preflight."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import acquisition_preflight as preflight
import authoring_inputs


HERE = Path(__file__).resolve().parent


class AcquisitionPreflightTest(unittest.TestCase):
    def test_lock_matches_frozen_manifest(self) -> None:
        manifest = preflight.load_json(HERE / "manifest.json")
        lock = preflight.load_json(HERE / "environment_lock.json")
        preflight.validate_manifest(manifest)
        preflight.validate_environment_lock(lock, manifest)
        plan = manifest["run_plan"]
        self.assertEqual(plan["request_variants"], ["static", "correction"])
        self.assertEqual(plan["repeat_labels"], [0])
        self.assertEqual(plan["runs_per_model"], 120)
        self.assertFalse(plan["requirement_revision"]["production_path_filter"])
        revision = preflight.requirement_revision_preflight(manifest)
        self.assertEqual(revision["p0_ready_count"], 30)
        self.assertEqual(revision["p1_materialized_count"], 30)
        self.assertEqual(lock["p1_authoring"], preflight.P1_AUTHORING_LOCK)
        self.assertEqual(revision["p1_authoring_status"], "verified")
        self.assertTrue(revision["p1_hashes_verified"])

    def test_authoring_record_allowlist_excludes_solution_fields(self) -> None:
        record = {
            "instance_id": "repo__name-1",
            "repo": "org/name",
            "base_commit": "a" * 40,
            "p0_sha256": "b" * 64,
            "problem_statement": "A task-specific requirement.",
            "base_checkout": {"path": "/tmp/checkout", "head": "a" * 40, "clean": True},
        }
        self.assertEqual(set(record), authoring_inputs.P0_RECORD_FIELDS)
        authoring_inputs._assert_no_forbidden_keys(record)
        self.assertFalse(any(field in record for field in ("patch", "test_patch", "hints_text")))

    def test_p1_provenance_shape_is_fail_closed(self) -> None:
        provenance = {
            "schema_version": 1,
            "kind": authoring_inputs.P1_PROVENANCE_KIND,
            "status": "blocked_fail_closed",
            "reason_code": "no_verified_task_specific_authoring_source",
            "input_kinds": ["p0_authoring_input", "clean_base_checkout"],
            "p1_text_materialized": False,
            "shared_p1_hash_required": True,
            "task_count": 30,
            "tasks": [],
        }
        authoring_inputs._assert_no_forbidden_keys(provenance)
        self.assertFalse(provenance["p1_text_materialized"])
        self.assertEqual(provenance["status"], "blocked_fail_closed")

    def test_confined_tracked_symlink_is_allowed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swebench-link-test-") as temporary:
            root = Path(temporary)
            (root / "target.txt").write_text("ok", encoding="utf-8")
            (root / "link.txt").symlink_to("target.txt")
            index = "120000 deadbeef 0\tlink.txt\0"
            result = preflight.validate_confined_symlinks(root, index)
            self.assertEqual(result["count"], 1)

    def test_escaping_tracked_symlink_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swebench-link-test-") as temporary:
            root = Path(temporary)
            outside = root.parent / f"{root.name}-outside.txt"
            outside.write_text("not in checkout", encoding="utf-8")
            try:
                (root / "link.txt").symlink_to(outside)
                index = "120000 deadbeef 0\tlink.txt\0"
                with self.assertRaises(preflight.AcquisitionError):
                    preflight.validate_confined_symlinks(root, index)
            finally:
                outside.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
