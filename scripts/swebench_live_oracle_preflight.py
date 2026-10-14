#!/usr/bin/env python3
"""Verify the 30-task SWE-bench-Live hidden oracle with gold patches.

This is an auditor-side operation. It may fetch datasets and Docker images, but
it passes only through the pinned official harness and never starts PurrCode or
contacts a model provider.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected an object")
    return value


def now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def clean_environment(hf_home: Path) -> dict[str, str]:
    return {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "HOME": str(Path.home()),
        "LANG": os.environ.get("LANG", "en_US.UTF-8"),
        "LC_ALL": os.environ.get("LC_ALL", "en_US.UTF-8"),
        "HF_HOME": str(hf_home),
    }


def image_digest(instance_id: str) -> tuple[str | None, str | None]:
    image = f"starryzhang/sweb.eval.x86_64.{instance_id.lower()}:latest"
    result = subprocess.run(
        ["docker", "image", "inspect", "--format", "{{.Id}}", image],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return image, None
    return image, result.stdout.strip() or None


def run_gold(task: Mapping[str, Any], *, out: Path, args: argparse.Namespace) -> dict[str, Any]:
    index = int(task["selection_index"])
    instance_id = str(task["instance_id"])
    run_id = f"gold-{index:02d}-{hashlib.sha256(instance_id.encode()).hexdigest()[:12]}"
    work = out / "oracle-work"
    work.mkdir(parents=True, exist_ok=True)
    stdout_path = out / "logs" / f"{run_id}.stdout"
    stderr_path = out / "logs" / f"{run_id}.stderr"
    stdout_path.parent.mkdir(parents=True, exist_ok=True)
    command = [
        str(args.oracle_python),
        str(args.oracle_runner),
        "--instance-ids",
        instance_id,
        "--predictions-path",
        "gold",
        "--max-workers",
        "1",
        "--timeout",
        str(args.timeout),
        "--cache-level",
        "instance",
        "--run-id",
        run_id,
        "--report-dir",
        str(out / "reports"),
    ]
    completed = subprocess.run(
        command,
        cwd=work,
        env=clean_environment(args.hf_home),
        capture_output=True,
        text=True,
        timeout=args.timeout + 300,
        check=False,
    )
    stdout_path.write_text(completed.stdout, encoding="utf-8")
    stderr_path.write_text(completed.stderr, encoding="utf-8")
    report_path = work / "logs" / "run_evaluation" / run_id / "gold" / instance_id / "report.json"
    report: Mapping[str, Any] | None = None
    report_error: str | None = None
    if report_path.is_file():
        try:
            raw = json.loads(report_path.read_text(encoding="utf-8"))
            if isinstance(raw, Mapping):
                entry = raw.get(instance_id)
                if isinstance(entry, Mapping):
                    report = entry
                else:
                    report_error = "report omitted instance entry"
            else:
                report_error = "report was not an object"
        except (OSError, json.JSONDecodeError) as error:
            report_error = str(error)
    else:
        report_error = "report.json was not written"
    image, digest = image_digest(instance_id)
    resolved = report.get("resolved") if isinstance(report, Mapping) else None
    passed = completed.returncode == 0 and resolved is True
    return {
        "selection_index": index,
        "instance_id": instance_id,
        "repo": str(task["repo"]),
        "base_commit": str(task["base_commit"]),
        "run_id": run_id,
        "status": "passed" if passed else "failed",
        "resolved": resolved,
        "patch_successfully_applied": report.get("patch_successfully_applied") if isinstance(report, Mapping) else None,
        "report_error": report_error,
        "oracle_exit_code": completed.returncode,
        "report_sha256": hashlib.sha256(report_path.read_bytes()).hexdigest() if report_path.is_file() else None,
        "image": image,
        "image_digest": digest,
        "model_calls": 0,
        "created_at": now(),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--oracle-python", type=Path, required=True)
    parser.add_argument("--oracle-runner", type=Path, required=True)
    parser.add_argument("--hf-home", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--max-tasks", type=int)
    parser.add_argument("--instance-ids", nargs="*")
    parser.add_argument("--resume", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    manifest = load_json(args.manifest)
    raw_tasks = manifest.get("tasks")
    if not isinstance(raw_tasks, list) or len(raw_tasks) != 30:
        raise ValueError("manifest must contain 30 tasks")
    tasks = [
        {**task, "selection_index": index}
        for index, task in enumerate(raw_tasks)
        if isinstance(task, Mapping)
    ]
    args.out.mkdir(parents=True, exist_ok=True)
    results_path = args.out / "gold-oracle-results.jsonl"
    existing: dict[str, dict[str, Any]] = {}
    if results_path.exists():
        for line in results_path.read_text(encoding="utf-8").splitlines():
            if line.strip():
                row = json.loads(line)
                existing[str(row["instance_id"])] = row
    processed = 0
    selected_ids = set(args.instance_ids or [])
    for task in tasks:
        instance_id = str(task["instance_id"])
        if selected_ids and instance_id not in selected_ids:
            continue
        if args.resume and instance_id in existing and existing[instance_id].get("status") == "passed":
            continue
        if args.max_tasks is not None and processed >= args.max_tasks:
            break
        row = run_gold(task, out=args.out, args=args)
        existing[instance_id] = row
        with results_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
            handle.flush()
            os.fsync(handle.fileno())
        processed += 1
        print(f"[{row['selection_index']:02d}] {instance_id}: {row['status']}", flush=True)
    selected_existing = {
        key: value
        for key, value in existing.items()
        if not selected_ids or key in selected_ids
    }
    summary = {
        "schema_version": 1,
        "kind": "swebench-live-hidden-oracle-gold-preflight",
        "status": "passed" if all(row.get("status") == "passed" for row in selected_existing.values()) and len(selected_existing) == (len(selected_ids) if selected_ids else 30) else "partial_or_failed",
        "task_count": len(selected_ids) if selected_ids else 30,
        "checked_count": len(selected_existing),
        "passed_count": sum(row.get("status") == "passed" for row in selected_existing.values()),
        "failed_count": sum(row.get("status") == "failed" for row in selected_existing.values()),
        "model_calls": 0,
        "outcomes": "unrun",
        "created_at": now(),
    }
    write_json(args.out / "gold-oracle-summary.json", summary)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
