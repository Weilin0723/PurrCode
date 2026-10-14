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
    for name, configuration in metadata.get("tool", {}).get("hatch", {}).get("envs", {}).items():
        if name in PUBLIC_TEST_GROUPS or name == "default":
            for key in ("dependencies", "extra-dependencies"):
                for dependency in configuration.get(key, []):
                    if not isinstance(dependency, str) or not dependency or dependency.startswith("-"):
                        raise ValueError("unsupported public Hatch dependency")
                    dependencies.append(dependency)
    return list(dict.fromkeys(dependencies))


def public_lock_constraints(checkout, project_name):
    versions = {}
    normalize = lambda name: re.sub(r"[-_.]+", "-", name).lower()
    for name in ("pdm.lock", "poetry.lock", "uv.lock"):
        path = checkout / name
        if not path.exists():
            continue
        for package in tomllib.loads(path.read_text()).get("package", []):
            package_name, version = package.get("name", ""), package.get("version", "")
            if (not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", package_name)
                    or not re.fullmatch(r"[0-9][A-Za-z0-9.!+_-]*", version)
                    or normalize(package_name) == normalize(project_name)):
                continue
            versions.setdefault(normalize(package_name), set()).add(version)
    return [name + "==" + next(iter(values)) for name, values in sorted(versions.items()) if len(values) == 1]


def poetry_test_groups(metadata):
    poetry = metadata.get("tool", {}).get("poetry", {})
    result = dict(poetry.get("dev-dependencies", {}))
    for name, configuration in poetry.get("group", {}).items():
        if name in PUBLIC_TEST_GROUPS:
            result.update(configuration.get("dependencies", {}))
    result.pop("python", None)
    return result


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
            receipt["initial_ruff_version"] = ruff_version
            execute([python, "-m", "pip", "install", "pytest==8.3.5", "ruff==" + ruff_version,
                     "hypothesis==6.131.9", "pytest-mock==3.14.0"])
            project = checkout / "pyproject.toml"
            metadata = tomllib.loads(project.read_text()) if project.exists() else {}
            project_name = metadata.get("project", {}).get("name", metadata.get("tool", {}).get("poetry", {}).get("name", ""))
            constraints = root / "public-lock-constraints.txt"
            locked = public_lock_constraints(checkout, project_name)
            constraints.write_text("\n".join(locked) + "\n")
            receipt["public_lock_constraints"] = locked
            extras = metadata.get("project", {}).get("optional-dependencies", {})
            selected = sorted(set(extras) & PUBLIC_TEST_GROUPS)
            if project.exists() or (checkout / "setup.py").exists():
                package = str(checkout) + ("[" + ",".join(selected) + "]" if selected else "")
                execute([python, "-m", "pip", "install", "-c", str(constraints), package])
            test_dependencies = public_test_dependencies(metadata)
            poetry_groups = poetry_test_groups(metadata)
            if poetry_groups:
                execute([python, "-m", "pip", "install", "poetry-core==2.1.3"])
                conversion = "import json, sys; from pathlib import Path; from poetry.core.factory import Factory; data=json.load(sys.stdin); print(json.dumps([Factory.create_dependency(name, value, root_dir=Path.cwd()).to_pep_508() for name,value in data.items()]))"
                requirements = subprocess.check_output([python, "-c", conversion], cwd=checkout,
                    input=json.dumps(poetry_groups), env=environment, text=True)
                test_dependencies.extend(json.loads(requirements))
            if test_dependencies:
                execute([python, "-m", "pip", "install", "-c", str(constraints), *test_dependencies])
            receipt["public_test_extras"] = selected
            receipt["public_test_group_dependencies"] = test_dependencies
            receipt["installed_packages"] = subprocess.check_output(
                [python, "-m", "pip", "freeze", "--all"], env=environment, text=True).splitlines()
            receipt["ruff_version"] = next((item.split("==", 1)[1] for item in receipt["installed_packages"] if item.startswith("ruff==")), None)
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
