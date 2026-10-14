#!/usr/bin/env python3
"""Run the pinned SWE-bench-Live oracle with deterministic host accommodations.

This wrapper is intentionally small and auditable. In its default mode it does
not alter the dataset, predictions, test lists, or grading. The explicit
``--targeted`` mode keeps the official test patch, image, parser, and declared
F2P/P2P IDs, while invoking those IDs in bounded chunks instead of unrelated
full-suite tests. Two host-interface details are also fixed before delegating
to the pinned official harness:

1. The dataset is materialized from its immutable Hugging Face revision to
   an operator-only temporary JSON file; the official loader accepts that file.
2. Apple Silicon defaults to ``arm64`` instance images, but the selected
   SWE-bench-Live images are published under the ``x86_64`` architecture. The
   selected instance IDs are therefore marked ``USE_X86`` so Docker runs them
   through the platform emulator.
3. Docker's credential helper may be unavailable in a credential-scrubbed
   research environment. A temporary ``DOCKER_CONFIG`` containing no auth
   entries lets the client pull public images without consulting a keychain.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import sys
import tempfile
from datetime import date, datetime
from pathlib import Path


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset-name", default="SWE-bench-Live/SWE-bench-Live")
    parser.add_argument("--dataset-revision", default="b51a86422e10cfd403beb4773e5a2947953e36ec")
    parser.add_argument("--split", default="verified")
    parser.add_argument("--instance-ids", nargs="+", required=True)
    parser.add_argument("--predictions-path", required=True)
    parser.add_argument("--max-workers", type=int, default=1)
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--cache-level", choices=("none", "base", "env", "instance"), default="env")
    parser.add_argument("--clean", action="store_true")
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--namespace", default="starryzhang")
    parser.add_argument("--instance-image-tag", default="latest")
    parser.add_argument("--report-dir", type=Path, default=Path("."))
    parser.add_argument("--force-rebuild", action="store_true")
    parser.add_argument("--rewrite-reports", action="store_true")
    parser.add_argument("--modal", action="store_true")
    parser.add_argument("--fail-fast", action="store_true")
    parser.add_argument("--targeted", action="store_true")
    return parser.parse_args()


def dataset_json_value(value):
    if isinstance(value, (date, datetime)):
        return value.isoformat()
    raise TypeError(f"unsupported dataset value type: {type(value).__name__}")


def materialize_pinned_dataset(args: argparse.Namespace, target: Path) -> None:
    from datasets import load_dataset

    dataset = load_dataset(args.dataset_name, split=args.split, revision=args.dataset_revision)
    selected = set(args.instance_ids)
    rows = [dict(row) for row in dataset if row["instance_id"] in selected]
    if {row["instance_id"] for row in rows} != selected:
        raise ValueError("pinned oracle dataset omits selected instances")
    target.write_text(json.dumps(rows, default=dataset_json_value), encoding="utf-8")


def main() -> int:
    args = _parse_args()
    try:
        from swebench.harness import run_evaluation
        from swebench.harness.test_spec import create_scripts, test_spec
    except ImportError as error:
        print(f"error: pinned SWE-bench-Live harness is unavailable: {error}", file=sys.stderr)
        return 2

    test_spec.USE_X86.update(args.instance_ids)
    if args.fail_fast or args.targeted:
        original = create_scripts.make_eval_script_list_common

        def adjusted_eval_script(instance, specs, env_name, repo_directory, base_commit, test_patch):
            commands = original(instance, specs, env_name, repo_directory, base_commit, test_patch)
            if args.targeted:
                from swebench.harness.constants import END_TEST_OUTPUT, START_TEST_OUTPUT
                from swebench.harness.test_spec.utils import get_test_cmds

                test_ids = list(instance.get("FAIL_TO_PASS") or []) + list(instance.get("PASS_TO_PASS") or [])
                if test_ids:
                    start = commands.index(f": '{START_TEST_OUTPUT}'") + 1
                    end = commands.index(f": '{END_TEST_OUTPUT}'")
                    base_commands = get_test_cmds(instance)
                    if not base_commands:
                        raise ValueError("targeted oracle has no base test command")
                    chunk_size = 200
                    replacement = []
                    for base_command in base_commands:
                        for offset in range(0, len(test_ids), chunk_size):
                            chunk = test_ids[offset : offset + chunk_size]
                            quoted = " ".join(shlex.quote(str(item)) for item in chunk)
                            replacement.append(f"{base_command} {quoted} --tb=short -x")
                    commands = commands[:start] + replacement + commands[end:]
            if args.fail_fast:
                commands = [
                    f"{command} -x" if "pytest" in command and " -x" not in command else command
                    for command in commands
                ]
            return commands

        create_scripts.make_eval_script_list_common = adjusted_eval_script
    with tempfile.TemporaryDirectory(prefix="purrcode-docker-config-") as docker_config:
        dataset_path = Path(docker_config) / "operator-dataset.json"
        materialize_pinned_dataset(args, dataset_path)
        config_path = Path(docker_config) / "config.json"
        config_path.write_text(json.dumps({"auths": {}}) + "\n", encoding="utf-8")
        os.environ["DOCKER_CONFIG"] = docker_config
        run_evaluation.main(
            dataset_name=str(dataset_path),
            split=args.split,
            instance_ids=args.instance_ids,
            predictions_path=args.predictions_path,
            max_workers=args.max_workers,
            force_rebuild=args.force_rebuild,
            cache_level=args.cache_level,
            clean=args.clean,
            open_file_limit=4096,
            run_id=args.run_id,
            timeout=args.timeout,
            namespace=args.namespace,
            rewrite_reports=args.rewrite_reports,
            modal=args.modal,
            instance_image_tag=args.instance_image_tag,
            report_dir=str(args.report_dir),
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
