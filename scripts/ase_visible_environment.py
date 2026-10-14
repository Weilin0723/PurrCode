#!/usr/bin/env python3
"""Provision public base-revision dependencies before any provider credential exists."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

from swebench_live_e2e import clean_environment, verify_checkout


def provision(manifest, instance_id, cache, root):
    row = next(row for row in manifest["tasks"] if row["instance_id"] == instance_id)
    task = {**row, "checkout": instance_id, "base_task_id": instance_id}
    checkout = verify_checkout(task, cache)
    environment = clean_environment()
    venv = root / "agent-venv"
    if venv.exists():
        raise RuntimeError("visible environment already exists; refusing to replace it")
    receipt = {"instance_id": instance_id, "base_commit": row["base_commit"],
               "status": "failed", "commands": []}
    with tempfile.TemporaryDirectory(prefix="ase-install-home-") as home:
        environment["HOME"] = home
        def execute(arguments):
            receipt["commands"].append(arguments)
            subprocess.run(arguments, cwd=checkout, env=environment, check=True)
        try:
            execute([os.sys.executable, "-m", "venv", str(venv)])
            python = str(venv / "bin/python")
            execute([python, "-m", "pip", "install", "pytest==8.3.5", "ruff==0.11.5",
                     "hypothesis==6.131.9", "pytest-mock==3.14.0"])
            project = checkout / "pyproject.toml"
            metadata = tomllib.loads(project.read_text()) if project.exists() else {}
            extras = metadata.get("project", {}).get("optional-dependencies", {})
            selected = sorted(set(extras) & {"test", "tests", "testing"})
            if project.exists() or (checkout / "setup.py").exists():
                package = str(checkout) + ("[" + ",".join(selected) + "]" if selected else "")
                execute([python, "-m", "pip", "install", package])
            receipt["public_test_extras"] = selected
            receipt["installed_packages"] = subprocess.check_output(
                [python, "-m", "pip", "freeze", "--all"], env=environment, text=True).splitlines()
            receipt["status"] = "passed"
        finally:
            root.mkdir(parents=True, exist_ok=True)
            (root / "visible-environment.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--instance-id", required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    provision(json.loads(args.manifest.read_text()), args.instance_id,
              args.cache.resolve(), args.root.resolve())
