#!/usr/bin/env python3
"""Provision public base-revision dependencies before any provider credential exists."""
import argparse
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import tomllib

from swebench_live_e2e import clean_environment, verify_checkout

PUBLIC_TEST_GROUPS = {"test", "tests", "testing", "dev", "development", "examples"}


def public_ruff_version(checkout):
    configuration = checkout / ".pre-commit-config.yaml"
    if configuration.exists():
        for block in re.split(r"(?m)^\s*-\s*repo:\s*", configuration.read_text())[1:]:
            repository = block.splitlines()[0].strip().strip("\"'")
            if repository in {"https://github.com/astral-sh/ruff-pre-commit",
                              "https://github.com/charliermarsh/ruff-pre-commit"}:
                revision = re.search(r"(?m)^\s*rev:\s*[\"']?v?(\d+\.\d+\.\d+)[\"']?\s*(?:#.*)?$", block)
                if revision:
                    return revision.group(1)
    return "0.11.5"


def public_test_dependencies(metadata):
    dependencies = []
    tables = [metadata.get("dependency-groups", {}),
              metadata.get("tool", {}).get("pdm", {}).get("dev-dependencies", {})]
    for table in tables:
        def expand(name, ancestors):
            if name in ancestors or len(ancestors) >= 32:
                raise ValueError("cyclic or excessive public test dependency groups")
            for item in table[name]:
                if isinstance(item, str) and item and not item.startswith("-"):
                    dependencies.append(item)
                elif isinstance(item, dict) and set(item) == {"include-group"}:
                    expand(item["include-group"], ancestors + [name])
                else:
                    raise ValueError("unsupported public test dependency declaration")
        for name in sorted(set(table) & PUBLIC_TEST_GROUPS):
            expand(name, [])
    return list(dict.fromkeys(dependencies))


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
            root.mkdir(parents=True, exist_ok=True)
            execute([os.sys.executable, "-m", "venv", str(venv)])
            python = str(venv / "bin/python")
            ruff_version = public_ruff_version(checkout)
            constraints = root / "public-tool-constraints.txt"
            constraints.write_text("ruff==" + ruff_version + "\n")
            receipt["ruff_version"] = ruff_version
            execute([python, "-m", "pip", "install", "-c", str(constraints), "pytest==8.3.5", "ruff==" + ruff_version,
                     "hypothesis==6.131.9", "pytest-mock==3.14.0"])
            project = checkout / "pyproject.toml"
            metadata = tomllib.loads(project.read_text()) if project.exists() else {}
            extras = metadata.get("project", {}).get("optional-dependencies", {})
            selected = sorted(set(extras) & PUBLIC_TEST_GROUPS)
            if project.exists() or (checkout / "setup.py").exists():
                package = str(checkout) + ("[" + ",".join(selected) + "]" if selected else "")
                execute([python, "-m", "pip", "install", "-c", str(constraints), package])
            test_dependencies = public_test_dependencies(metadata)
            if test_dependencies:
                execute([python, "-m", "pip", "install", "-c", str(constraints), *test_dependencies])
            receipt["public_test_extras"] = selected
            receipt["public_test_group_dependencies"] = test_dependencies
            receipt["installed_packages"] = subprocess.check_output(
                [python, "-m", "pip", "freeze", "--all"], env=environment, text=True).splitlines()
            collection_environment = dict(environment)
            collection_environment["PYTHONPATH"] = str(checkout / "src") + os.pathsep + str(checkout)
            collection_environment["PYTHONDONTWRITEBYTECODE"] = "1"
            arguments = [python, "-m", "pytest", "--collect-only", "-q"]
            receipt["commands"].append(arguments)
            collection = subprocess.run(arguments, cwd=checkout, env=collection_environment,
                                        capture_output=True, text=True, timeout=180)
            receipt["public_test_collection"] = {"exit_code": collection.returncode,
                "stdout": collection.stdout[-65536:], "stderr": collection.stderr[-65536:]}
            if collection.returncode != 0:
                raise RuntimeError("public base-revision pytest collection failed before model execution")
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
