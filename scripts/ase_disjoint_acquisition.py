#!/usr/bin/env python3
"""Reproduce the pre-model, repository-disjoint ASE qualification candidates.

Only public statements and opaque hashes leave the dataset reader. No model
is contacted, and solution/test strings are never written or printed.
"""

import copy
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "benchmarks/research/swebench-live-v1"))
import acquisition_preflight as acquisition


def oracle_count(row, name):
    return row.get(name.lower() + "_count", len(row.get(name, [])))


def public_projection(row):
    public = {key: row[key] for key in ("instance_id", "repo", "base_commit", "pull_number", "difficulty", "test_cmds", "log_parser", "problem_statement")}
    for field in ("problem_statement", "patch", "test_patch"):
        public[field + "_sha256"] = hashlib.sha256(row[field].encode()).hexdigest()
    public["fail_to_pass_count"] = len(row["FAIL_TO_PASS"])
    public["pass_to_pass_count"] = len(row["PASS_TO_PASS"])
    return public


def select(prior, rows, rejected_repositories=(), observed_repositories=(), pytest_only=False):
    groups = {}
    for index, row in sorted(rows.items()):
        difficulty = row.get("difficulty", {})
        commands = row.get("test_cmds") or []
        if isinstance(commands, str):
            commands = [commands]
        pytest_eligible = not pytest_only or any("pytest" in str(command).lower() for command in commands)
        if (pytest_eligible and isinstance(difficulty, dict) and isinstance(difficulty.get("lines"), int)
                and difficulty["lines"] <= 50 and oracle_count(row, "FAIL_TO_PASS") > 0 and oracle_count(row, "PASS_TO_PASS") > 0):
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
        filtered_eligible_row_count=sum(len(group) for group in groups.values()),
        filtered_eligible_repository_count=len(groups),
        eligibility_count_scope="eligible_* counts describe the pinned base eligibility; filtered_eligible_* additionally apply the public pytest-command filter",
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
                    fail_to_pass_count=oracle_count(row, "FAIL_TO_PASS"), pass_to_pass_count=oracle_count(row, "PASS_TO_PASS"),
                    candidate_status="pending", oracle_status="pending")
        for field in ("problem_statement", "patch", "test_patch"):
            task[field + "_sha256"] = (hashlib.sha256(row[field].encode()).hexdigest()
                                        if field in row else row[field + "_sha256"])
        manifest["tasks"].append(task)
    acquisition.validate_manifest(manifest)
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("rejected_repositories", nargs="*")
    parser.add_argument("--observed-manifest", type=Path)
    parser.add_argument("--pytest-only", action="store_true")
    parser.add_argument("--fetch-page-size", type=int, choices=range(1, 101), default=100)
    parser.add_argument("--fetch-workers", type=int, choices=range(1, 5), default=1)
    parser.add_argument("--public-cache-root", type=Path)
    args = parser.parse_args()
    prior = acquisition.load_json(ROOT / "benchmarks/research/swebench-live-v1/manifest.json")
    rows = {}
    def fetch_page(offset):
        cache = (args.public_cache_root / f"{offset}-{args.fetch_page_size}.json"
                 if args.public_cache_root else None)
        if cache and cache.exists():
            page = json.loads(cache.read_text())
            if page["source"] != prior["source"]:
                raise ValueError("public projection source differs from pinned source")
            return {int(index): row for index, row in page["rows"].items()}
        fetched = acquisition.fetch_public_rows(
            prior["source"], offset, min(args.fetch_page_size, 500 - offset))
        projected = {index: public_projection(row) for index, row in fetched.items()}
        if cache:
            cache.parent.mkdir(parents=True, exist_ok=True)
            with cache.open("x") as handle:
                json.dump({"source": prior["source"], "rows": projected}, handle, sort_keys=True)
        return projected
    with ThreadPoolExecutor(max_workers=args.fetch_workers) as executor:
        for page in executor.map(fetch_page, range(0, 500, args.fetch_page_size)):
            rows.update(page)
    observed = (acquisition.load_json(args.observed_manifest)["selection"]["repository_order"]
                if args.observed_manifest else [])
    manifest = select(prior, rows, args.rejected_repositories, observed, args.pytest_only)
    target = args.output
    with target.open("x") as handle:
        json.dump(manifest, handle, indent=2)
        handle.write("\n")
