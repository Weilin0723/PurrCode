#!/usr/bin/env python3
"""Prepare and analyze the frozen DeepSeek v2 operational-repair study."""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any

try:
    from scripts import deepseek_repository_completion_probe as core
except ModuleNotFoundError:  # Direct execution places scripts/ on sys.path.
    import deepseek_repository_completion_probe as core


STUDY_ID = "repository-completion-deepseek-v2"
MAX_OUTPUT_TOKENS = 256
PLAN_PATH = Path("docs/research/repository-completion-deepseek-v2-plan.md")


def prepare(output: Path) -> dict[str, Any]:
    """Create a v2 freeze while retaining the tested v1 analysis implementation."""

    core.MAX_OUTPUT_TOKENS = MAX_OUTPUT_TOKENS
    report = core.prepare(output)
    output = output.resolve()
    repo_root = Path(__file__).resolve().parents[1]
    plan = repo_root / PLAN_PATH
    wrapper = Path(__file__).resolve()
    analysis_core = Path(core.__file__).resolve()
    freeze_path = output / "freeze.json"
    manifest_path = output / "manifest.json"
    freeze = core.read_object(freeze_path, "freeze")
    manifest = core.read_object(manifest_path, "manifest")

    old_plan = str(freeze["preregistration_plan"])
    dependencies = dict(freeze["dependency_hashes"])
    dependencies.pop(old_plan, None)
    dependencies[str(PLAN_PATH)] = core.file_hash(plan)
    dependencies[str(wrapper.relative_to(repo_root))] = core.file_hash(wrapper)
    dependencies[str(analysis_core.relative_to(repo_root))] = core.file_hash(analysis_core)

    freeze.update(
        {
            "study_id": STUDY_ID,
            "freeze_kind": "deepseek-repository-completion-operational-repair-pre-call-freeze",
            "script": str(wrapper.relative_to(repo_root)),
            "script_sha256": core.file_hash(wrapper),
            "analysis_core": str(analysis_core.relative_to(repo_root)),
            "analysis_core_sha256": core.file_hash(analysis_core),
            "dependency_hashes": dependencies,
            "preregistration_plan": str(PLAN_PATH),
            "preregistration_plan_sha256": core.file_hash(plan),
            "predecessor_study": "repository-completion-deepseek-v1",
            "repair_reason": "v1 JSON truncation attrition exceeded 10 percent at 128 tokens",
        }
    )
    core.write_json(freeze_path, freeze)

    manifest.update(
        {
            "study_id": STUDY_ID,
            "artifact_kind": "deepseek-repository-completion-operational-repair",
            "analysis_role": "outcome_aware_operational_repair",
            "script": freeze["script"],
            "script_sha256": freeze["script_sha256"],
            "analysis_core": freeze["analysis_core"],
            "analysis_core_sha256": freeze["analysis_core_sha256"],
            "dependency_hashes": dependencies,
            "preregistration_plan": freeze["preregistration_plan"],
            "preregistration_plan_sha256": freeze["preregistration_plan_sha256"],
            "predecessor_study": freeze["predecessor_study"],
            "repair_reason": freeze["repair_reason"],
            "freeze": {"path": "freeze.json", "sha256": core.file_hash(freeze_path)},
        }
    )
    core.write_json(manifest_path, manifest)
    report.update(
        {
            "study_id": STUDY_ID,
            "freeze_sha256": manifest["freeze"]["sha256"],
            "max_output_tokens": MAX_OUTPUT_TOKENS,
        }
    )
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("prepare", "analyze", "audit"):
        command = subparsers.add_parser(name)
        command.add_argument("--out", type=Path, required=True)
        if name == "audit":
            command.add_argument("--repo-root", type=Path)
    args = parser.parse_args()
    core.MAX_OUTPUT_TOKENS = MAX_OUTPUT_TOKENS
    if args.command == "prepare":
        result = prepare(args.out)
        exit_code = 0 if result["preflight_ok"] else 1
    elif args.command == "analyze":
        result = core.analyze(args.out)
        exit_code = 0 if result["status"] == "complete" else 1
    else:
        result = core.audit(args.out, args.repo_root)
        exit_code = 0 if result["valid"] else 1
    print(core.json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
