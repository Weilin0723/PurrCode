#!/usr/bin/env python3
"""Build the double-anonymous repository-completion supplementary archive."""

from __future__ import annotations

import gzip
import hashlib
import json
import re
import shutil
import tarfile
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "docs/research/submission/anonymous-repository-completion-study.tar.gz"
TEXT_SUFFIXES = {".json", ".jsonl", ".md", ".py", ".rs", ".toml", ".lock", ".txt"}
TEXT_NAMES = {".gitignore", "Cargo.lock"}
SOURCES = (
    "benchmarks/alignment-fixture",
    "benchmarks/research/small-model-pilot",
    "docs/research/paper-draft.md",
    "docs/research/repository-completion-v1-plan.md",
    "docs/research/repository-completion-v1-results.md",
    "docs/research/repository-completion-deepseek-v1-plan.md",
    "docs/research/repository-completion-deepseek-v2-plan.md",
    "docs/research/repository-completion-deepseek-results.md",
    "docs/research/evidence/2026-10-12/repository-completion-v1-study-freeze.json",
    "docs/research/evidence/2026-10-12/repository-completion-v1-independent-analysis.json",
    "docs/research/evidence/2026-10-12/repository-completion-v1-llama3.2-3b",
    "docs/research/evidence/2026-10-12/repository-completion-v1-llama3.2-1b",
    "docs/research/evidence/2026-10-13/deepseek-v4-pro-qualification.json",
    "docs/research/evidence/2026-10-13/repository-completion-deepseek-v1",
    "docs/research/evidence/2026-10-13/repository-completion-deepseek-v2",
    "crates/evaluation-runtime/src/bin/repository_completion_remote.rs",
    "benchmarks/research/swebench-live-v1",
    "docs/research/repository-completion-swebench-live-v1-plan.md",
    "docs/research/swebench-live-v1-model-freeze.md",
    "docs/research/fse-main-track-gap-closure.md",
    "docs/research/swebench-live-smoke-2026-10-13.md",
    "docs/research/evidence/2026-10-13/swebench-live-v1",
    "scripts/swebench_live_e2e.py",
    "scripts/run_swebench_live_oracle.py",
    "scripts/swebench_live_oracle_preflight.py",
    "scripts/freeze_fse_confirmatory_study.py",
    "scripts/fse_confirmatory_analysis.py",
    "scripts/test_swebench_live_e2e.py",
    "scripts/test_freeze_fse_confirmatory_study.py",
    "scripts/test_fse_confirmatory_analysis.py",
    ".github/workflows/research-swebench-live.yml",
    "scripts/repository_completion_probe.py",
    "scripts/deepseek_repository_completion_probe.py",
    "scripts/deepseek_repository_completion_probe_v2.py",
    "scripts/audit_deepseek_post_redaction.py",
    "scripts/local_review_probe.py",
    "scripts/research_metrics.py",
    "scripts/small_model_pilot_preflight.py",
    "scripts/test_repository_completion_probe.py",
    "scripts/test_deepseek_repository_completion_probe.py",
    "scripts/test_deepseek_repository_completion_probe_v2.py",
    "scripts/test_research_metrics.py",
    "scripts/test_small_model_pilot_preflight.py",
)


def sanitize(text: str) -> str:
    text = text.replace(str(ROOT), "${REPOSITORY_ROOT}")
    text = re.sub(
        r"/var/folders/[^\s\"']+/T/repository-completion-",
        "${TMPDIR}/repository-completion-",
        text,
    )
    text = text.replace("PurrCode", "AnonymousPrototype")
    text = text.replace("purrcode", "anonymous_prototype")
    text = text.replace("PawGate", "PolicyEvaluator")
    text = text.replace("NineLives", "DurableSessionStore")
    text = text.replace("Claw", "ExecutionAdapter")
    return text


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def copy_source(source: Path, destination: Path) -> None:
    if source.is_dir():
        shutil.copytree(
            source,
            destination,
            ignore=shutil.ignore_patterns("target", "__pycache__", ".DS_Store"),
        )
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)


def sanitize_tree(root: Path) -> None:
    for path in sorted(root.rglob("*")):
        if path.is_file() and (path.suffix in TEXT_SUFFIXES or path.name in TEXT_NAMES):
            path.write_text(sanitize(path.read_text(encoding="utf-8")), encoding="utf-8")


def refresh_fixture_hashes(root: Path) -> None:
    dataset_path = root / "benchmarks/research/small-model-pilot/dataset.json"
    fixture_root = root / "benchmarks/alignment-fixture"
    dataset = json.loads(dataset_path.read_text(encoding="utf-8"))
    dataset["fixture_sha256"] = {
        relative: sha256(fixture_root / relative) for relative in dataset["fixture_files"]
    }
    dataset_path.write_text(
        json.dumps(dataset, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def scan(root: Path) -> None:
    forbidden = re.compile(
        r"(?i)(/Users/|jackzhang|Documents/GitHub|\bPurrCode\b|\bPawGate\b|"
        r"\bNineLives\b|\bClaw\b|Authorization:\s*Bearer\s+\S+|\bsk-[A-Za-z0-9_-]{12,})"
    )
    findings = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or (
            path.suffix not in TEXT_SUFFIXES and path.name not in TEXT_NAMES
        ):
            continue
        match = forbidden.search(path.read_text(encoding="utf-8"))
        if match:
            findings.append(f"{path.relative_to(root)}: {match.group(0)!r}")
    if findings:
        raise RuntimeError("anonymization scan failed:\n" + "\n".join(findings))


def tar_filter(info: tarfile.TarInfo) -> tarfile.TarInfo:
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    return info


def main() -> None:
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="anonymous-research-artifact-") as temp:
        package = Path(temp) / "anonymous-repository-completion-study"
        package.mkdir()
        for relative in SOURCES:
            source = ROOT / relative
            if not source.exists():
                raise FileNotFoundError(source)
            copy_source(source, package / relative)

        sanitize_tree(package)
        refresh_fixture_hashes(package)
        readme = """# Anonymous repository-completion study artifact

This supplementary package accompanies the double-anonymous submission. It contains the frozen
protocol, executable six-task fixture and hidden oracles, the complete 288-call local records, both
DeepSeek follow-up runs, audits, independent analyses, and tests for the research harness.

Run the offline harness tests from the package root with:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -S -m unittest \\
  scripts.test_repository_completion_probe \\
  scripts.test_deepseek_repository_completion_probe \\
  scripts.test_deepseek_repository_completion_probe_v2 \\
  scripts.test_research_metrics scripts.test_small_model_pilot_preflight
```

Paths, prototype/package names, and temporary-directory prefixes in the copied text have been
replaced to preserve double anonymity. This creates a disclosure-safe derivative: cryptographic
hashes retained in the freeze records identify the immutable pre-sanitization originals and are
not expected to match the renamed derivative byte-for-byte. Counts, labels, model outputs,
schedule indices, statistical results, and model digests are unchanged. No model weights or
provider credentials are included.
"""
        (package / "README.md").write_text(readme, encoding="utf-8")

        files = []
        for path in sorted(package.rglob("*")):
            if path.is_file():
                files.append(
                    {
                        "path": path.relative_to(package).as_posix(),
                        "sha256": sha256(path),
                        "bytes": path.stat().st_size,
                    }
                )
        (package / "ARTIFACT-MANIFEST.json").write_text(
            json.dumps({"schema_version": 1, "files": files}, indent=2) + "\n",
            encoding="utf-8",
        )
        scan(package)

        with OUTPUT.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w") as archive:
                    archive.add(package, arcname=package.name, filter=tar_filter)

    print(f"wrote {OUTPUT}")
    print(f"sha256 {sha256(OUTPUT)}")


if __name__ == "__main__":
    main()
