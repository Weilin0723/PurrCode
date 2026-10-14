#!/usr/bin/env python3
"""Reproduce the pre-model, repository-disjoint ASE qualification candidates.

Only public statements and opaque hashes leave the dataset reader. No model
is contacted, and solution/test strings are never written or printed.
"""

import copy
import argparse
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "benchmarks/research/swebench-live-v1"))
import acquisition_preflight as acquisition


def select(prior, rows, rejected_repositories=(), observed_repositories=(), pytest_only=False):
    groups = {}
    for index, row in sorted(rows.items()):
        difficulty = row.get("difficulty", {})
        commands = row.get("test_cmds") or []
        if isinstance(commands, str):
            commands = [commands]
        pytest_eligible = not pytest_only or any("pytest" in str(command).lower() for command in commands)
        if (pytest_eligible and isinstance(difficulty, dict) and isinstance(difficulty.get("lines"), int)
                and difficulty["lines"] <= 50 and row.get("FAIL_TO_PASS") and row.get("PASS_TO_PASS")):
            groups.setdefault(row["repo"], []).append((index, row))
    excluded = set(prior["selection"]["repository_order"])
    excluded.update(rejected_repositories)
    excluded.update(observed_repositories)
    repositories = [repo for repo, group in groups.items() if len(group) >= 3 and repo not in excluded][:10]
    if len(repositories) != 10:
        raise ValueError("not enough disjoint eligible repositories")
    manifest = copy.deepcopy(prior)
    manifest["selection"].update(
        algorithm="Apply pinned line-count and nonempty F2P/P2P eligibility, additionally require a public pytest test command when pytest_only is true; exclude prior, observed, and oracle-failed repository lists; retain the next ten groups in first-eligible-row order and each group's first three rows. Oracle qualification precedes the scientific freeze.",
        eligible_row_count=sum(len(group) for group in groups.values()),
        eligible_repository_count=len(groups),
        repository_order=repositories,
        excluded_prior_repository_order=prior["selection"]["repository_order"],
        excluded_oracle_failed_repositories=list(rejected_repositories),
        excluded_observed_repository_order=list(observed_repositories),
        pytest_only=pytest_only,
    )
    manifest["study_role"] = "repository-disjoint pre-model oracle qualification candidates; no efficacy outcomes"
    manifest["tasks"] = []
    selected = sorted([entry for repo in repositories for entry in groups[repo][:3]])
    for index, (row_index, row) in enumerate(selected):
        task = {key: row[key] for key in ("instance_id", "repo", "base_commit", "pull_number", "difficulty", "test_cmds", "log_parser")}
        task.update(selection_index=index, dataset_row=row_index,
                    repo_url=f"https://github.com/{row['repo']}.git",
                    commit_url=f"https://github.com/{row['repo']}/commit/{row['base_commit']}",
                    fail_to_pass_count=len(row["FAIL_TO_PASS"]), pass_to_pass_count=len(row["PASS_TO_PASS"]),
                    candidate_status="pending", oracle_status="pending")
        for field in ("problem_statement", "patch", "test_patch"):
            task[field + "_sha256"] = hashlib.sha256(row[field].encode()).hexdigest()
        manifest["tasks"].append(task)
    acquisition.validate_manifest(manifest)
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("rejected_repositories", nargs="*")
    parser.add_argument("--observed-manifest", type=Path)
    parser.add_argument("--pytest-only", action="store_true")
    args = parser.parse_args()
    prior = acquisition.load_json(ROOT / "benchmarks/research/swebench-live-v1/manifest.json")
    rows = acquisition.fetch_public_rows(prior["source"], 0, 500)
    observed = (acquisition.load_json(args.observed_manifest)["selection"]["repository_order"]
                if args.observed_manifest else [])
    manifest = select(prior, rows, args.rejected_repositories, observed, args.pytest_only)
    target = args.output
    with target.open("x") as handle:
        json.dump(manifest, handle, indent=2)
        handle.write("\n")
