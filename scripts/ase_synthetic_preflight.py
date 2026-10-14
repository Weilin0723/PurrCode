#!/usr/bin/env python3
"""Verify the real daemon's two-phase correction path on a disposable repository."""

import argparse
import json
import subprocess
import tempfile
from pathlib import Path

import swebench_live_e2e as e2e


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon", required=True)
    parser.add_argument("--token-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--model", default="deepseek/deepseek-flash")
    parser.add_argument("--arm", choices=("B", "C"), default="C")
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("smoke output is write-once")
    repository = Path(tempfile.mkdtemp(prefix="ase-synthetic-preflight-"))
    for argv in (["git", "init", "-q"], ["git", "config", "user.name", "Synthetic Research"],
                 ["git", "config", "user.email", "synthetic@example.invalid"]):
        subprocess.run(argv, cwd=repository, env=e2e.clean_environment(), check=True)
    (repository / "README.md").write_text("Synthetic repository; no study task.\n")
    for argv in (["git", "add", "README.md"], ["git", "commit", "-q", "-m", "Synthetic baseline"]):
        subprocess.run(argv, cwd=repository, env=e2e.clean_environment(), check=True)
    task = dict(arm=args.arm, variant="correction", model=args.model, task_id="synthetic-correction",
                run_id="synthetic-correction", max_tokens=300000,
                objective="Create a new file named marker.txt in the repository root containing exactly marker followed by a newline.",
                mid_run_correction="Additionally create followup.txt containing exactly followup followed by a newline. Preserve marker.txt.")
    measurement, events, view = e2e.run_session(
        base=args.daemon, token=args.token_file.read_text().strip(), task=task,
        repository=repository, maximum_seconds=180,
    )
    worktree = e2e.state_worktree(view, events)
    contents = {name: (worktree / name).read_text() if (worktree / name).exists() else None
                for name in ("marker.txt", "followup.txt")}
    passed = (measurement["session_status"] == "completed"
              and measurement["mid_run_correction_acknowledged"]
              and e2e.alignment_observed(events) == (args.arm == "C")
              and contents == {"marker.txt": "marker\n", "followup.txt": "followup\n"})
    result = dict(kind="synthetic_harness_preflight_not_study_evidence", arm=args.arm, passed=passed,
                  measurement=measurement, metrics=e2e.event_metrics(events), contents=contents,
                  patch=e2e.extract_patch(worktree).decode())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as handle:
        json.dump(result, handle, indent=2)
        handle.write("\n")
    print(json.dumps(result))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
