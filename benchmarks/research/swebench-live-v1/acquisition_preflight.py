#!/usr/bin/env python3
"""Plan or preflight pinned SWE-bench-Live repository acquisition.

This tool never contacts a model.  ``--plan`` is side-effect free, ``--verify-dataset``
only reads the public dataset rows endpoint, and ``--preflight`` only inspects
already-acquired detached Git checkouts.  ``--environment-preflight`` additionally
hashes pinned public harness assets and probes Docker with a credential-free
configuration.  Network acquisition requires the explicit
``--acquire --allow-network-write`` pair and refuses to reuse a non-empty target
directory.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import ssl
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter
from pathlib import Path
from typing import Any, Mapping, Sequence


ROOT = Path(__file__).resolve().parent
DEFAULT_MANIFEST = ROOT / "manifest.json"
DEFAULT_CACHE = ROOT / "acquired"
DEFAULT_LOCK = ROOT / "environment_lock.json"
OFFICIAL_HARNESS_COMMIT = "a145aa87c62361532dacaa243398978164b234b7"
OFFICIAL_HARNESS_REPOSITORY = "SWE-bench-Live/SWE-bench-Live"
OFFICIAL_RAW_BASE = (
    "https://raw.githubusercontent.com/SWE-bench-Live/SWE-bench-Live/"
    f"{OFFICIAL_HARNESS_COMMIT}/"
)
OFFICIAL_ASSET_PATHS = (
    "pyproject.toml",
    "README.md",
    "docs/guides/evaluation.md",
    "docs/guides/docker_setup.md",
    "swebench/harness/run_evaluation.py",
    "swebench/harness/test_spec/test_spec.py",
    "swebench/harness/docker_build.py",
    "swebench/harness/docker_utils.py",
    "swebench/harness/grading.py",
    "swebench/harness/constants/python.py",
    "swebench/harness/dockerfiles/python.py",
)
FORBIDDEN_MODEL_FIELDS = {
    "patch",
    "test_patch",
    "hints_text",
    "all_hints_text",
    "FAIL_TO_PASS",
    "PASS_TO_PASS",
    "gold_patch",
    "reference_solution",
}
SAFE_DOCKER_PATH = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin"
P1_AUTHORING_LOCK = {
    "status": "verified",
    "reason_code": "controlled_span_from_original_statement",
    "task_count": 30,
    "p1_materialized_count": 30,
    "p1_text_materialized": True,
    "shared_p1_hash_required": True,
    "p1_hashes_verified": True,
    "input_kinds": ["original_problem_statement", "clean_base_checkout"],
    "asset_path": "requirement_revision.json",
    "asset_sha256": "de94df6c4b04aa99fb01a5a7c61077a155c07079e0c7c163244b6065f13fb0cf",
}


class AcquisitionError(RuntimeError):
    pass


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise AcquisitionError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise AcquisitionError(f"{path}: expected an object")
    return value


def clean_environment() -> dict[str, str]:
    blocked = (
        "API_KEY",
        "APIKEY",
        "AUTH_TOKEN",
        "CREDENTIAL",
        "PASSWORD",
        "PRIVATE_KEY",
        "SECRET",
        "TOKEN",
        "PROXY",
    )
    safe = {
        key: value
        for key, value in os.environ.items()
        if not any(fragment in key.upper() for fragment in blocked)
        and not key.upper().startswith(("GIT_CONFIG_", "GIT_ASKPASS", "SSH_ASKPASS"))
    }
    safe.update(
        {
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_TERMINAL_PROMPT": "0",
        }
    )
    return safe


def run(argv: Sequence[str], *, cwd: Path | None = None, timeout: int = 120) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            list(argv),
            cwd=cwd,
            env=clean_environment(),
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise AcquisitionError(f"command timed out: {' '.join(argv)}") from error
    if result.returncode != 0:
        detail = (result.stdout + result.stderr).strip()[-4000:]
        raise AcquisitionError(f"command failed ({result.returncode}): {' '.join(argv)}\n{detail}")
    return result


def safe_child(root: Path, name: str) -> Path:
    path = (root / name).resolve()
    root = root.resolve()
    if path != root and root not in path.parents:
        raise AcquisitionError(f"target escapes cache directory: {name}")
    return path


def tasks(manifest: Mapping[str, Any]) -> list[Mapping[str, Any]]:
    raw = manifest.get("tasks")
    if not isinstance(raw, list):
        raise AcquisitionError("manifest.tasks must be a list")
    return [item for item in raw if isinstance(item, Mapping)]


def validate_manifest(manifest: Mapping[str, Any]) -> None:
    if manifest.get("schema_version") != 1:
        raise AcquisitionError("unsupported manifest schema_version")
    if manifest.get("status") not in {"acquisition_pending", "preflight_pass"}:
        raise AcquisitionError("manifest is not in an acquisition/preflight state")
    if manifest.get("outcomes") != "unrun":
        raise AcquisitionError("outcomes must remain unrun")
    source = manifest.get("source")
    if not isinstance(source, Mapping):
        raise AcquisitionError("manifest.source is missing")
    if source.get("dataset") != "SWE-bench-Live/SWE-bench-Live" or source.get("split") != "verified":
        raise AcquisitionError("manifest must use the pinned SWE-bench-Live verified split")
    revision = source.get("revision")
    if not isinstance(revision, str) or len(revision) != 40 or any(char not in "0123456789abcdef" for char in revision):
        raise AcquisitionError("source.revision must be a 40-character lowercase commit")
    raw_tasks = tasks(manifest)
    if len(raw_tasks) != 30:
        raise AcquisitionError("manifest must contain exactly thirty tasks")
    ids = [str(item.get("instance_id")) for item in raw_tasks]
    repos = [str(item.get("repo")) for item in raw_tasks]
    rows = [item.get("dataset_row") for item in raw_tasks]
    repo_issue_keys = [(str(item.get("repo")), str(item.get("pull_number"))) for item in raw_tasks]
    repo_counts = Counter(repos)
    if len(set(ids)) != 30 or len(set(rows)) != 30 or len(set(repo_issue_keys)) != 30:
        raise AcquisitionError("task instance_id, pull number, and dataset_row values must be unique")
    if len(repo_counts) != 10 or set(repo_counts.values()) != {3}:
        raise AcquisitionError("manifest must contain exactly ten repositories with three tasks each")
    selection = manifest.get("selection")
    expected_selection = {
        "task_count": 30,
        "distinct_repository_count": 10,
        "tasks_per_repository": 3,
        "difficulty_lines_max": 50,
        "source_rows_scanned": 500,
        "eligible_row_count": 370,
        "eligible_repository_count": 87,
    }
    if not isinstance(selection, Mapping) or any(selection.get(key) != value for key, value in expected_selection.items()):
        raise AcquisitionError("selection is not the frozen thirty-task shape")
    for item in raw_tasks:
        for field in ("instance_id", "repo", "repo_url", "base_commit", "commit_url", "patch_sha256", "test_patch_sha256", "problem_statement_sha256"):
            if not isinstance(item.get(field), str) or not item[field]:
                raise AcquisitionError(f"task {item.get('instance_id')}: missing {field}")
        if item.get("candidate_status") != "pending" or item.get("oracle_status") != "pending":
            raise AcquisitionError(f"task {item['instance_id']}: candidate/oracle status must remain pending")
        forbidden = {"patch", "test_patch", "hints_text", "all_hints_text", "FAIL_TO_PASS", "PASS_TO_PASS"}
        if forbidden.intersection(item):
            raise AcquisitionError(f"task {item['instance_id']}: solution-bearing field leaked into manifest")
    plan = manifest.get("run_plan")
    expected_plan = {
        "repositories": 10,
        "tasks": 30,
        "arms": ["B", "C"],
        "request_variants": ["static", "correction"],
        "repeat_labels": [0],
        "runs_per_model": 120,
        "arm_definitions": {
            "B": "alignment disabled",
            "C": "alignment enabled with automatic evidence refresh",
        },
        "requirement_phases": ["P0", "P1"],
        "requirement_revision": {
            "status": "verified",
            "per_task": True,
            "P0": "controlled abridged problem_statement with one withheld original constraint span in the correction variant",
            "P1": "the exact original constraint span selected from the complete problem_statement",
            "static_delivery": "complete original problem_statement delivered from call 0",
            "correction_delivery": "abridged P0 at call 0; the identical P1 span after the first phase settles with a qualifying successful mutation",
            "injection_trigger": "first qualifying ExecutionFinished; identical P1 delivered after the first phase settles",
            "production_path_filter": False,
            "acknowledgement_event": "requirement_revision_ack",
            "acknowledgement_required": True,
            "correction_text_stored": True,
            "authoring_asset": "requirement_revision.json",
            "span_selection": "one unique original behavior-related sentence or concise span; reject non-unique or non-specific spans",
            "span_offsets_unit": "unicode_codepoints",
            "final_requirement": "reconstructing abridged P0 with P1 at the recorded offset equals the complete original statement",
            "shared_final_requirement_hash": True,
        },
        "status": "blocked_until_environment_oracle_audit",
    }
    if plan != expected_plan:
        raise AcquisitionError("run_plan is not the frozen 120-run 2x2 E2E shape")


def manifest_hash(manifest: Mapping[str, Any]) -> str:
    encoded = json.dumps(manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def public_https_context() -> ssl.SSLContext:
    """Use certificate verification even when the bundled Python lacks a CA path."""
    defaults = ssl.get_default_verify_paths()
    for candidate in (defaults.cafile, "/etc/ssl/cert.pem"):
        if candidate and Path(candidate).is_file():
            return ssl.create_default_context(cafile=candidate)
    return ssl.create_default_context()


def public_proxy_map() -> dict[str, str]:
    """Keep public proxy routing while dropping any embedded proxy credentials."""
    proxies: dict[str, str] = {}
    for scheme in ("http", "https"):
        value = next(
            (os.environ.get(name) for name in (f"{scheme.upper()}_PROXY", f"{scheme.lower()}_proxy") if os.environ.get(name)),
            None,
        )
        if not value:
            continue
        try:
            parsed = urllib.parse.urlsplit(value)
            if parsed.scheme and parsed.hostname:
                netloc = parsed.hostname
                if parsed.port is not None:
                    netloc = f"{netloc}:{parsed.port}"
                proxies[scheme] = urllib.parse.urlunsplit((parsed.scheme, netloc, "", "", ""))
        except ValueError:
            continue
    return proxies


def fetch_public_bytes(url: str, *, accept: str = "*/*") -> bytes:
    """Read a public URL without inheriting proxy credentials or auth headers."""
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler(public_proxy_map()),
        urllib.request.HTTPSHandler(context=public_https_context()),
    )
    request = urllib.request.Request(
        url,
        headers={"Accept": accept, "User-Agent": "purrcode-swebench-preflight/1"},
    )
    try:
        with opener.open(request, timeout=60) as response:
            return response.read()
    except (OSError, urllib.error.URLError) as error:
        raise AcquisitionError(f"cannot read public asset: {error}") from error


def target_name(task: Mapping[str, Any]) -> str:
    return str(task["instance_id"]).replace("/", "__")


def acquisition_plan(manifest: Mapping[str, Any], cache: Path) -> list[list[str]]:
    commands: list[list[str]] = []
    for task in tasks(manifest):
        target = safe_child(cache, target_name(task))
        commands.extend(
            [
                ["git", "clone", "--filter=blob:none", "--no-checkout", str(task["repo_url"]), str(target)],
                ["git", "-C", str(target), "fetch", "--depth", "1", "origin", str(task["base_commit"])],
                ["git", "-C", str(target), "checkout", "--detach", str(task["base_commit"])],
            ]
        )
    return commands


def acquire(manifest: Mapping[str, Any], cache: Path) -> None:
    cache.mkdir(parents=True, exist_ok=True)
    for task in tasks(manifest):
        target = safe_child(cache, target_name(task))
        if target.exists():
            if any(target.iterdir()):
                raise AcquisitionError(f"refusing to reuse non-empty target: {target}")
            target.rmdir()
        run(["git", "clone", "--filter=blob:none", "--no-checkout", str(task["repo_url"]), str(target)])
        run(["git", "-C", str(target), "fetch", "--depth", "1", "origin", str(task["base_commit"])])
        run(["git", "-C", str(target), "checkout", "--detach", str(task["base_commit"])])


def tracked_symlink_paths(index: str) -> set[str]:
    paths: set[str] = set()
    for entry in index.split("\0"):
        if not entry:
            continue
        metadata, separator, relative = entry.partition("\t")
        if not separator:
            raise AcquisitionError("malformed git index entry")
        if metadata.split(" ", 1)[0] == "120000":
            paths.add(relative)
    return paths


def validate_confined_symlinks(root: Path, index: str) -> dict[str, Any]:
    root = root.resolve()
    tracked = tracked_symlink_paths(index)
    for relative in sorted(tracked):
        relative_path = Path(relative)
        if relative_path.is_absolute() or any(part in {"", ".", ".."} for part in relative_path.parts):
            raise AcquisitionError(f"unsafe tracked symlink path: {relative}")
        path = root / relative_path
        if not path.is_symlink():
            raise AcquisitionError(f"tracked symlink is not a symlink: {relative}")
        try:
            resolved = path.resolve(strict=True)
        except OSError as error:
            raise AcquisitionError(f"broken tracked symlink {relative}: {error}") from error
        if resolved != root and root not in resolved.parents:
            raise AcquisitionError(f"tracked symlink escapes checkout: {relative} -> {os.readlink(path)}")
    return {
        "tracked": sorted(tracked),
        "count": len(tracked),
        "policy": "tracked_relative_targets_confined_to_checkout",
    }


def checkout_preflight(manifest: Mapping[str, Any], cache: Path) -> dict[str, Any]:
    cache = cache.resolve()
    checked: list[str] = []
    symlink_count = 0
    for task in tasks(manifest):
        target = safe_child(cache, target_name(task))
        if not target.is_dir():
            raise AcquisitionError(f"missing acquired checkout: {target}")
        head = run(["git", "-C", str(target), "rev-parse", "HEAD"]).stdout.strip()
        if head != task["base_commit"]:
            raise AcquisitionError(f"{task['instance_id']}: HEAD {head} != pinned base commit")
        status = run(["git", "-C", str(target), "status", "--porcelain", "--untracked-files=all"]).stdout
        if status.strip():
            raise AcquisitionError(f"{task['instance_id']}: checkout is dirty")
        index = run(["git", "-C", str(target), "ls-files", "-s", "-z"]).stdout
        symlinks = validate_confined_symlinks(target, index)
        symlink_count += int(symlinks["count"])
        checked.append(str(task["instance_id"]))
    return {
        "checked_out": checked,
        "checkout_count": len(checked),
        "confined_tracked_symlink_count": symlink_count,
        "symlink_policy": "tracked_relative_targets_confined_to_checkout",
    }


def fetch_public_rows(source: Mapping[str, Any], offset: int, length: int) -> dict[int, Mapping[str, Any]]:
    rows: dict[int, Any] = {}
    for page_offset in range(offset, offset + length, 100):
        page_length = min(100, offset + length - page_offset)
        query = urllib.parse.urlencode(
            {
                "dataset": source["dataset"],
                "config": source["config"],
                "split": source["split"],
                "offset": page_offset,
                "length": page_length,
                "revision": source["revision"],
            }
        )
        url = f"{source['rows_endpoint']}?{query}"
        try:
            payload = json.loads(fetch_public_bytes(url, accept="application/json").decode("utf-8"))
        except (AcquisitionError, json.JSONDecodeError) as error:
            raise AcquisitionError(f"cannot verify public dataset rows: {error}") from error
        page_rows = payload.get("rows", [])
        if not isinstance(page_rows, list):
            raise AcquisitionError("public dataset response rows must be a list")
        for entry in page_rows:
            if not isinstance(entry, Mapping) or not isinstance(entry.get("row"), Mapping):
                raise AcquisitionError("public dataset response contains a malformed row")
            rows[int(entry["row_idx"])] = entry["row"]
    if len(rows) != length:
        raise AcquisitionError(f"public dataset response returned {len(rows)} rows; expected {length}")
    return rows


def verify_dataset_rows(manifest: Mapping[str, Any]) -> dict[str, Any]:
    source = manifest["source"]
    selection = manifest["selection"]
    raw_tasks = tasks(manifest)
    all_rows = fetch_public_rows(source, 0, int(selection["source_rows_scanned"]))
    eligible: list[tuple[int, Mapping[str, Any]]] = []
    for row_idx in sorted(all_rows):
        row = all_rows[row_idx]
        difficulty = row.get("difficulty")
        if (
            isinstance(difficulty, Mapping)
            and isinstance(difficulty.get("lines"), int)
            and difficulty["lines"] <= selection["difficulty_lines_max"]
            and row.get("FAIL_TO_PASS")
            and row.get("PASS_TO_PASS")
        ):
            eligible.append((row_idx, row))
    grouped: dict[str, list[tuple[int, Mapping[str, Any]]]] = {}
    for row_idx, row in eligible:
        grouped.setdefault(str(row["repo"]), []).append((row_idx, row))
    repository_order = sorted(
        (repo for repo, group in grouped.items() if len(group) >= selection["tasks_per_repository"]),
        key=lambda repo: min(row_idx for row_idx, _ in grouped[repo]),
    )[: selection["distinct_repository_count"]]
    if len(eligible) != selection["eligible_row_count"]:
        raise AcquisitionError("eligible row count does not match the frozen selection")
    if len(grouped) != selection["eligible_repository_count"]:
        raise AcquisitionError("eligible repository count does not match the frozen selection")
    if repository_order != selection["repository_order"]:
        raise AcquisitionError("repository order does not match the frozen selection")
    selected_ids = {
        row["instance_id"]
        for repo in repository_order
        for _, row in sorted(grouped[repo], key=lambda item: item[0])[: selection["tasks_per_repository"]]
    }
    manifest_ids = {item["instance_id"] for item in raw_tasks}
    if selected_ids != manifest_ids:
        raise AcquisitionError("manifest tasks do not match the deterministic selection")
    verified: list[str] = []
    for task in raw_tasks:
        row = all_rows.get(int(task["dataset_row"]))
        if not isinstance(row, Mapping):
            raise AcquisitionError(f"dataset row missing: {task['dataset_row']}")
        for field in ("instance_id", "repo", "pull_number", "base_commit", "difficulty", "test_cmds", "log_parser"):
            if row.get(field) != task[field]:
                raise AcquisitionError(f"{task['instance_id']}: dataset field mismatch for {field}")
        if len(row.get("FAIL_TO_PASS") or []) != task["fail_to_pass_count"]:
            raise AcquisitionError(f"{task['instance_id']}: FAIL_TO_PASS count mismatch")
        if len(row.get("PASS_TO_PASS") or []) != task["pass_to_pass_count"]:
            raise AcquisitionError(f"{task['instance_id']}: PASS_TO_PASS count mismatch")
        for field, manifest_field in (
            ("problem_statement", "problem_statement_sha256"),
            ("patch", "patch_sha256"),
            ("test_patch", "test_patch_sha256"),
        ):
            digest = hashlib.sha256(str(row[field]).encode("utf-8")).hexdigest()
            if digest != task[manifest_field]:
                raise AcquisitionError(f"{task['instance_id']}: {field} hash mismatch")
        verified.append(str(task["instance_id"]))
    return {
        "source_row_count": len(all_rows),
        "eligible_row_count": len(eligible),
        "eligible_repository_count": len(grouped),
        "selected_repository_order": repository_order,
        "verified_rows": verified,
        "row_count": len(verified),
    }


def validate_environment_lock(lock: Mapping[str, Any], manifest: Mapping[str, Any]) -> None:
    if lock.get("schema_version") != 1:
        raise AcquisitionError("unsupported environment lock schema_version")
    if lock.get("status") not in {"preflight_partial", "preflight_pass"}:
        raise AcquisitionError("environment lock is not a preflight lock")
    if lock.get("manifest_sha256") != manifest_hash(manifest):
        raise AcquisitionError("environment lock does not match manifest digest")
    dataset = lock.get("dataset")
    expected_dataset = {
        "dataset": "SWE-bench-Live/SWE-bench-Live",
        "split": "verified",
        "revision": manifest["source"]["revision"],
    }
    if dataset != expected_dataset:
        raise AcquisitionError("environment lock dataset does not match manifest")
    checkout = lock.get("acquired_checkout")
    if not isinstance(checkout, Mapping) or checkout.get("checkout_count") != 30:
        raise AcquisitionError("environment lock must describe thirty acquired checkouts")
    official = lock.get("official_harness")
    if not isinstance(official, Mapping):
        raise AcquisitionError("environment lock official_harness is missing")
    if official.get("repository") != OFFICIAL_HARNESS_REPOSITORY:
        raise AcquisitionError("environment lock has an unexpected official harness repository")
    if official.get("commit") != OFFICIAL_HARNESS_COMMIT:
        raise AcquisitionError("environment lock has an unexpected official harness commit")
    if official.get("raw_base_url") != OFFICIAL_RAW_BASE:
        raise AcquisitionError("environment lock raw asset base URL is not pinned")
    if official.get("python_minimum") != ">=3.10" or official.get("docker_required") is not True:
        raise AcquisitionError("environment lock must require the official Python and Docker runtime")
    if official.get("image_namespace") != "starryzhang" or official.get("image_digest_status") not in {
        "unverified_until_docker_smoke",
        "single_instance_smoke_only",
    }:
        raise AcquisitionError("environment lock image digest status is invalid")
    if official.get("image_digest_status") == "single_instance_smoke_only":
        smoke_image = official.get("selected_smoke_image")
        if not isinstance(smoke_image, Mapping) or smoke_image.get("instance_id") != "bridgecrewio__checkov-6630":
            raise AcquisitionError("environment lock is missing the verified smoke image")
        if smoke_image.get("digest") != "sha256:bd59eee7174a803d311a588bab4682e826a46774608d1a6587762d45bfd4dcad":
            raise AcquisitionError("environment lock smoke image digest is not pinned")
    files = official.get("files")
    if not isinstance(files, list):
        raise AcquisitionError("environment lock official_harness.files must be a list")
    by_path: dict[str, Mapping[str, Any]] = {}
    for item in files:
        if not isinstance(item, Mapping) or not isinstance(item.get("path"), str):
            raise AcquisitionError("environment lock contains a malformed official asset")
        asset_path = str(item["path"])
        if asset_path in by_path or asset_path not in OFFICIAL_ASSET_PATHS:
            raise AcquisitionError(f"unexpected or duplicate official asset: {asset_path}")
        digest = item.get("sha256")
        if not isinstance(digest, str) or len(digest) != 64 or any(char not in "0123456789abcdef" for char in digest):
            raise AcquisitionError(f"official asset {asset_path}: invalid SHA-256 digest")
        by_path[asset_path] = item
    if set(by_path) != set(OFFICIAL_ASSET_PATHS):
        missing = sorted(set(OFFICIAL_ASSET_PATHS) - set(by_path))
        raise AcquisitionError(f"environment lock is missing official assets: {missing}")
    visibility = lock.get("model_visibility")
    if not isinstance(visibility, Mapping):
        raise AcquisitionError("environment lock model_visibility is missing")
    forbidden = visibility.get("forbidden_fields")
    if not isinstance(forbidden, list) or not FORBIDDEN_MODEL_FIELDS.issubset(set(forbidden)):
        raise AcquisitionError("environment lock does not forbid solution-bearing fields")
    requirement_revision = lock.get("requirement_revision")
    if requirement_revision != manifest.get("run_plan", {}).get("requirement_revision"):
        raise AcquisitionError("environment lock requirement revision does not match manifest")
    if lock.get("p1_authoring") != P1_AUTHORING_LOCK:
        raise AcquisitionError("environment lock must keep P1 authoring fail-closed")
    oracle = lock.get("oracle")
    if not isinstance(oracle, Mapping) or oracle.get("status") not in {"unverified", "verified"}:
        raise AcquisitionError("environment lock oracle status is invalid")
    smoke = oracle.get("gold_smoke")
    if not isinstance(smoke, Mapping) or smoke.get("status") != "passed":
        raise AcquisitionError("environment lock must record the real gold smoke result")
    if (
        smoke.get("instance_id") != "bridgecrewio__checkov-6630"
        or smoke.get("target_test_exit_code") != 0
        or smoke.get("target_test_count") != 1
        or smoke.get("evidence_sha256")
        != "57f8d1e2e92cc26c5cc90312a692f63d19c3fd8c4a532c21b3c48de915eea2da"
    ):
        raise AcquisitionError("environment lock gold smoke evidence is invalid")


def requirement_revision_preflight(manifest: Mapping[str, Any]) -> dict[str, Any]:
    """Audit P0 sources without materializing a correction from hidden solution data."""
    revision = manifest["run_plan"]["requirement_revision"]
    p0_missing: list[str] = []
    p1_materialized: list[str] = []
    for task in tasks(manifest):
        instance_id = str(task["instance_id"])
        if not task.get("base_commit") or not task.get("problem_statement_sha256"):
            p0_missing.append(instance_id)
        if any(key.lower() in {"p1", "p1_text", "correction", "correction_text"} for key in task):
            p1_materialized.append(instance_id)
    if p0_missing:
        raise AcquisitionError(f"P0 source missing for tasks: {p0_missing[:8]}")
    if p1_materialized:
        raise AcquisitionError(f"P1 correction leaked into manifest for tasks: {p1_materialized[:8]}")
    return {
        "task_count": len(tasks(manifest)),
        "p0_source": "problem_statement_sha256 plus base_commit",
        "p0_ready_count": len(tasks(manifest)),
        "p1_status": revision["status"],
        "p1_materialized_count": len(tasks(manifest)) if revision["correction_text_stored"] else 0,
        "p1_authoring_status": revision["status"],
        "p1_hashes_verified": revision["shared_final_requirement_hash"],
        "static_delivery": revision["static_delivery"],
        "correction_delivery": revision["correction_delivery"],
        "injection_trigger": revision["injection_trigger"],
        "production_path_filter": revision["production_path_filter"],
        "acknowledgement_event": revision["acknowledgement_event"],
    }


def requirement_revision_asset_preflight(
    manifest: Mapping[str, Any], lock: Mapping[str, Any]
) -> dict[str, Any]:
    """Verify the stored controlled-span asset before allowing any model run."""
    try:
        from requirement_revision import RequirementRevisionError, verify_asset

        report = verify_asset(manifest, ROOT / "requirement_revision.json")
    except (AcquisitionError, RequirementRevisionError, OSError, KeyError, TypeError, ValueError) as error:
        raise AcquisitionError(f"requirement revision asset is not verified: {error}") from error
    if report["asset_sha256"] != lock["p1_authoring"]["asset_sha256"]:
        raise AcquisitionError("requirement revision asset digest does not match environment lock")
    return report


def verify_official_assets(lock: Mapping[str, Any]) -> dict[str, Any]:
    official = lock["official_harness"]
    verified: list[str] = []
    for item in official["files"]:
        asset_path = str(item["path"])
        url = f"{official['raw_base_url']}{urllib.parse.quote(asset_path, safe='/')}"
        digest = hashlib.sha256(fetch_public_bytes(url)).hexdigest()
        if digest != item["sha256"]:
            raise AcquisitionError(f"official asset digest mismatch: {asset_path}")
        verified.append(asset_path)
    return {
        "repository": official["repository"],
        "commit": official["commit"],
        "asset_count": len(verified),
        "verified_assets": verified,
    }


def docker_probe() -> dict[str, Any]:
    """Probe only a local default Docker daemon with a credential-free config."""
    docker_binary = shutil.which("docker", path=SAFE_DOCKER_PATH)
    if docker_binary is None:
        return {"status": "unavailable", "reason": "docker_cli_missing"}
    hosts: list[str | None] = [None]
    for socket in (Path("/var/run/docker.sock"), Path.home() / ".docker" / "run" / "docker.sock"):
        if socket.exists():
            hosts.append(f"unix://{socket}")
    with tempfile.TemporaryDirectory(prefix="purrcode-docker-config-") as config_dir:
        for host in hosts:
            environment = {
                "PATH": SAFE_DOCKER_PATH,
                "DOCKER_CONFIG": config_dir,
                "DOCKER_CLI_HINTS": "false",
                "LANG": "C",
                "LC_ALL": "C",
            }
            if host is not None:
                environment["DOCKER_HOST"] = host
            result = subprocess.run(
                [docker_binary, "version", "--format", "{{.Server.Version}}"],
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            server_version = result.stdout.strip().splitlines()
            if result.returncode == 0 and server_version and server_version[0]:
                return {"status": "available", "server_version": server_version[0]}
    return {"status": "unavailable", "reason": "docker_daemon_unavailable"}


def environment_preflight(
    manifest: Mapping[str, Any], cache: Path, lock: Mapping[str, Any]
) -> dict[str, Any]:
    expected_cache = lock["acquired_checkout"]["cache_basename"]
    if cache.resolve().name != expected_cache:
        raise AcquisitionError(f"cache basename does not match lock: {cache.resolve().name}")
    requirement_report = requirement_revision_preflight(manifest)
    requirement_asset_report = requirement_revision_asset_preflight(manifest, lock)
    checkout_report = checkout_preflight(manifest, cache)
    assets_report = verify_official_assets(lock)
    docker_report = docker_probe()
    blockers = []
    if lock["requirement_revision"]["status"] != "verified":
        blockers.append("contamination_safe_p0_p1_unverified")
    if lock["p1_authoring"]["status"] != "verified":
        blockers.append("p1_authoring_not_verified")
    if lock["oracle"]["status"] != "verified":
        blockers.append("hidden_behavioral_oracle_unverified")
    if docker_report["status"] != "available":
        blockers.append(str(docker_report["reason"]))
    return {
        "status": "preflight_pass" if not blockers else "preflight_partial",
        "checkout": checkout_report,
        "official_assets": assets_report,
        "docker": docker_report,
        "requirement_revision": {**requirement_report, "asset": requirement_asset_report},
        "p1_authoring": lock["p1_authoring"],
        "oracle": {
            "status": lock["oracle"]["status"],
            "gold_smoke": lock["oracle"]["gold_smoke"],
        },
        "blockers": blockers,
        "model_calls": 0,
        "outcomes": "unrun",
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--cache-dir", type=Path, default=DEFAULT_CACHE)
    parser.add_argument("--plan", action="store_true", help="print explicit acquisition argv vectors without executing them")
    parser.add_argument("--acquire", action="store_true", help="acquire pinned public repositories")
    parser.add_argument("--allow-network-write", action="store_true", help="required with --acquire")
    parser.add_argument("--preflight", action="store_true", help="inspect already-acquired detached checkouts")
    parser.add_argument("--verify-dataset", action="store_true", help="read and verify pinned public dataset rows")
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK, help="environment lock manifest")
    parser.add_argument(
        "--verify-official-assets",
        action="store_true",
        help="verify hashes of the pinned public SWE-bench evaluation assets",
    )
    parser.add_argument(
        "--environment-preflight",
        action="store_true",
        help="audit checkouts, official assets, Docker reachability, and oracle status",
    )
    args = parser.parse_args(argv)
    try:
        manifest = load_json(args.manifest.resolve())
        validate_manifest(manifest)
        lock: dict[str, Any] | None = None
        if args.verify_official_assets or args.environment_preflight:
            lock = load_json(args.lock.resolve())
            validate_environment_lock(lock, manifest)
        if args.acquire and not args.allow_network_write:
            raise AcquisitionError("--acquire requires explicit --allow-network-write")
        report: dict[str, Any] = {
            "manifest": str(args.manifest.resolve()),
            "manifest_sha256": manifest_hash(manifest),
            "status": "acquisition_pending",
            "model_calls": 0,
            "outcomes": "unrun",
        }
        if args.plan:
            report["plan"] = acquisition_plan(manifest, args.cache_dir.resolve())
        if args.acquire:
            acquire(manifest, args.cache_dir.resolve())
            report["acquired"] = True
        if args.verify_dataset:
            report["dataset"] = verify_dataset_rows(manifest)
        if args.preflight and not args.environment_preflight:
            report["checkout"] = checkout_preflight(manifest, args.cache_dir.resolve())
            report["status"] = "checkout_preflight_pass"
        if args.verify_official_assets:
            if lock is None:
                raise AcquisitionError("official asset verification requires an environment lock")
            report["official_assets"] = verify_official_assets(lock)
            report["status"] = "official_assets_verified"
        if args.environment_preflight:
            if lock is None:
                raise AcquisitionError("environment preflight requires an environment lock")
            report.update(environment_preflight(manifest, args.cache_dir.resolve(), lock))
        if not any(
            (
                args.plan,
                args.acquire,
                args.preflight,
                args.verify_dataset,
                args.verify_official_assets,
                args.environment_preflight,
            )
        ):
            raise AcquisitionError(
                "choose at least one of --plan, --acquire, --preflight, --verify-dataset, "
                "--verify-official-assets, or --environment-preflight"
            )
    except (AcquisitionError, OSError, KeyError, TypeError, ValueError) as error:
        print(json.dumps({"status": "preflight_failed", "error": str(error)}, sort_keys=True))
        return 1
    print(json.dumps(report, indent=2, sort_keys=True))
    return 2 if args.environment_preflight and report.get("status") != "preflight_pass" else 0


if __name__ == "__main__":
    raise SystemExit(main())
