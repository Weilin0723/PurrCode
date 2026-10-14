#!/usr/bin/env python3
"""Prepare and run the repository-disjoint SWE-bench-Live E2E study.

The runner keeps the model-visible worktree separate from the auditor-only
dataset row. PurrCode receives only the pinned objective (and, for correction
runs, the frozen P1 span). The final worktree is converted to a patch and
graded by the pinned official SWE-bench-Live harness, not by the agent's own
verdict.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import fse_confirmatory_analysis as analysis  # noqa: E402

TERMINAL_STATUSES = {
    "completed",
    "failed",
    "cancelled",
    "paused",
    "awaiting_review",
    "awaiting_approval",
    "ready",
}
STOPPED_WORKER_STATUSES = {
    "completed",
    "interrupted",
    "failed",
    "failed:",
    "skipped dependency:",
}
DEFAULT_MAXIMUM_SECONDS = 20 * 60
DEFAULT_ORACLE_TIMEOUT = 30 * 60
MAXIMUM_PATCH_BYTES = 8 * 1024 * 1024
POLL_SECONDS = 0.5


class E2EError(RuntimeError):
    pass


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise E2EError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise E2EError(f"{path}: expected a JSON object")
    return value


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def canonical_hash(value: Any) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def file_hash(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise E2EError(f"cannot hash {path}: {error}") from error


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def safe_slug(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-") or "run"


def run(argv: Sequence[str], *, cwd: Path, env: Mapping[str, str] | None = None, timeout: int | None = None) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            list(argv),
            cwd=cwd,
            env=None if env is None else dict(env),
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise E2EError(f"command timed out: {' '.join(argv)}") from error


def git_output(repository: Path, arguments: Sequence[str], *, env: Mapping[str, str] | None = None) -> str:
    result = run(["git", "-C", str(repository), *arguments], cwd=repository, env=env)
    if result.returncode != 0:
        raise E2EError(f"git {' '.join(arguments)} failed: {result.stderr.strip()}")
    return result.stdout


def ensure_checkout(task: Mapping[str, Any], cache_dir: Path) -> Path:
    checkout = (cache_dir / str(task["checkout"])).resolve()
    if checkout.exists() and not checkout.is_dir():
        raise E2EError(f"checkout path is not a directory: {checkout}")
    if not checkout.is_dir():
        checkout.parent.mkdir(parents=True, exist_ok=True)
        env = clean_environment()
        for command in (
            ["git", "clone", "--filter=blob:none", "--no-checkout", str(task["repo_url"]), str(checkout)],
            ["git", "-C", str(checkout), "fetch", "--depth", "1", "origin", str(task["base_commit"])],
            ["git", "-C", str(checkout), "checkout", "--detach", str(task["base_commit"])],
        ):
            result = run(command, cwd=checkout.parent, env=env)
            if result.returncode != 0:
                raise E2EError(f"checkout setup failed: {result.stderr.strip()}")
    return checkout


def verify_checkout(task: Mapping[str, Any], cache_dir: Path) -> Path:
    checkout = ensure_checkout(task, cache_dir)
    head = git_output(checkout, ["rev-parse", "HEAD"]).strip()
    if head != str(task["base_commit"]):
        raise E2EError(f"{task['base_task_id']}: checkout HEAD {head} != {task['base_commit']}")
    status = git_output(
        checkout,
        [
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            ".",
            ":(exclude).purrcode/**",
        ],
    )
    if status.strip():
        raise E2EError(f"{task['base_task_id']}: source checkout is dirty")
    return checkout


def clean_environment(*, home: Path | None = None, hf_home: Path | None = None) -> dict[str, str]:
    blocked = ("API_KEY", "APIKEY", "AUTH_TOKEN", "CREDENTIAL", "PASSWORD", "PRIVATE_KEY", "SECRET", "TOKEN")
    values = {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "HOME": str(home or Path.home()),
        "LANG": os.environ.get("LANG", "en_US.UTF-8"),
        "LC_ALL": os.environ.get("LC_ALL", "en_US.UTF-8"),
    }
    if hf_home is not None:
        values["HF_HOME"] = str(hf_home)
    return values


def http_json(method: str, url: str, token: str, *, body: Any = None, timeout: float = 30.0) -> Any:
    data = None if body is None else canonical_bytes(body)
    request = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
            "Content-Type": "application/json",
            "User-Agent": "purrcode-swebench-live-e2e/1",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            payload = response.read()
            if not payload:
                return None
            return json.loads(payload.decode("utf-8"))
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")[:1000]
        raise E2EError(f"HTTP {error.code} from {url}: {detail}") from error
    except (OSError, json.JSONDecodeError) as error:
        raise E2EError(f"HTTP request failed for {url}: {error}") from error


def session_status(view: Mapping[str, Any]) -> str:
    return str(view.get("status_code") or view.get("status") or "").lower()


def session_stopped(view: Mapping[str, Any]) -> bool:
    return view.get("lease_active") is False and session_status(view) in TERMINAL_STATUSES


def worker_stopped(status: str) -> bool:
    lowered = status.lower()
    return lowered in STOPPED_WORKER_STATUSES or lowered.startswith(("failed:", "skipped dependency:"))


def supervisor_stopped(view: Mapping[str, Any]) -> bool:
    workers = view.get("workers")
    return isinstance(workers, list) and all(
        isinstance(worker, Mapping) and worker_stopped(str(worker.get("status", ""))) for worker in workers
    )


def get_session(base: str, token: str, session_id: str) -> dict[str, Any]:
    value = http_json("GET", f"{base}/v1/sessions/{session_id}", token)
    if not isinstance(value, dict):
        raise E2EError("daemon returned a non-object session view")
    return value


def get_events(base: str, token: str, session_id: str) -> list[dict[str, Any]]:
    value = http_json("GET", f"{base}/v1/sessions/{session_id}/events", token)
    if not isinstance(value, list) or any(not isinstance(item, dict) for item in value):
        raise E2EError("daemon returned invalid session events")
    return value


def first_successful_mutation(events: Sequence[Mapping[str, Any]]) -> tuple[Mapping[str, Any], int] | None:
    scopes: dict[str, Mapping[str, Any]] = {}
    for event in events:
        if event.get("event") != "tool_evidence_recorded":
            continue
        data = event.get("data")
        evidence = data.get("evidence") if isinstance(data, Mapping) else None
        if not isinstance(evidence, Mapping):
            continue
        action_id = evidence.get("action_id")
        if isinstance(action_id, str):
            scopes[action_id] = evidence
    for index, event in enumerate(events):
        if event.get("event") != "execution_finished":
            continue
        data = event.get("data")
        if not isinstance(data, Mapping):
            continue
        if data.get("exit_code") != 0 or not data.get("affected_paths"):
            continue
        evidence = scopes.get(str(data.get("action_id")), {})
        tool_id = str(evidence.get("tool_id", ""))
        scope = evidence.get("effective_filesystem_scope")
        if tool_id in {"native:write_file", "native:delete_file"}:
            return event, index
        if isinstance(scope, Mapping) and scope.get("kind") == "worktree":
            return event, index
    return None


def normalized_correction_digest(value: str) -> str:
    return hashlib.sha256(value.rstrip(" \t").encode("utf-8")).hexdigest()


def wait_for_ack(base: str, token: str, session_id: str, start_index: int, digest: str, timeout: float = 10.0) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        events = get_events(base, token, session_id)
        for event in events[start_index:]:
            if event.get("event") != "conversation_message_added":
                continue
            data = event.get("data")
            message = data.get("message") if isinstance(data, Mapping) else None
            if not isinstance(message, Mapping) or message.get("role") != "user":
                continue
            if normalized_correction_digest(str(message.get("content", ""))) == digest:
                return canonical_hash(event)
        time.sleep(POLL_SECONDS)
    raise E2EError("daemon accepted the correction but no durable user-message acknowledgement appeared")


def find_correction_ack(events: Sequence[Mapping[str, Any]], start_index: int, digest: str) -> str | None:
    for event in events[start_index:]:
        if event.get("event") != "conversation_message_added":
            continue
        data = event.get("data")
        message = data.get("message") if isinstance(data, Mapping) else None
        if not isinstance(message, Mapping) or message.get("role") != "user":
            continue
        if normalized_correction_digest(str(message.get("content", ""))) == digest:
            return canonical_hash(event)
    return None


def request_cancel(base: str, token: str, session_id: str, reason: str) -> None:
    try:
        http_json("POST", f"{base}/v1/sessions/{session_id}/cancel", token, body={"reason": reason})
    except E2EError as error:
        if "HTTP 409" not in str(error):
            raise


def wait_until_stopped(base: str, token: str, session_id: str, timeout: float = 30.0) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last: dict[str, Any] = {}
    while time.monotonic() < deadline:
        last = get_session(base, token, session_id)
        supervisor = http_json("GET", f"{base}/v1/supervisor/{session_id}", token)
        if session_stopped(last) and isinstance(supervisor, Mapping) and supervisor_stopped(supervisor):
            return last
        time.sleep(POLL_SECONDS)
    raise E2EError("could not confirm that the session and supervisor workers stopped")


def run_session(
    *,
    base: str,
    token: str,
    task: Mapping[str, Any],
    repository: Path,
    maximum_seconds: int,
) -> tuple[dict[str, Any], list[dict[str, Any]], dict[str, Any]]:
    created = http_json(
        "POST",
        f"{base}/v1/sessions",
        token,
        body={
            "objective": task["objective"],
            "repository": str(repository),
            "task_mode": "build",
            "permission_mode": "auto",
            "model": task["model"],
            "max_tokens": int(task["max_tokens"]),
            "alignment": "enabled" if task["arm"] == "C" else "disabled",
            "research_alignment_opt_in": task["arm"] == "B",
        },
    )
    if not isinstance(created, Mapping) or not isinstance(created.get("id"), str):
        raise E2EError("daemon did not return a session id")
    session_id = created["id"]
    started = time.monotonic()
    correction_requested = task["variant"] == "correction"
    correction_triggered = False
    correction_acknowledged = False
    trigger_hash: str | None = None
    message_hash: str | None = None
    timed_out = False

    def await_phase(deadline: float) -> tuple[dict[str, Any], list[dict[str, Any]], bool]:
        while True:
            view = get_session(base, token, session_id)
            events = get_events(base, token, session_id)
            if session_stopped(view):
                return view, events, False
            if time.monotonic() >= deadline:
                request_cancel(
                    base,
                    token,
                    session_id,
                    "SWE-bench-Live E2E task deadline exceeded",
                )
                return view, events, True
            time.sleep(POLL_SECONDS)

    deadline = started + maximum_seconds
    phase_view, phase_events, timed_out = await_phase(deadline)
    # A new user correction can reopen an outcome review, but it does not
    # approve that outcome or an awaiting native-action authorization.
    phase_view = wait_until_stopped(base, token, session_id)
    if (correction_requested and not timed_out
            and session_status(phase_view) in {"completed", "paused", "awaiting_review"}):
        trigger = first_successful_mutation(phase_events)
        if trigger is not None:
            event, event_index = trigger
            correction = str(task.get("mid_run_correction") or "")
            if not correction.strip():
                raise E2EError("correction task has no P1 text")
            digest = normalized_correction_digest(correction)
            trigger_hash = canonical_hash(event)
            start_index = len(phase_events)
            try:
                http_json(
                    "POST",
                    f"{base}/v1/sessions/{session_id}/messages",
                    token,
                    body={"content": correction},
                    timeout=30,
                )
            except E2EError as error:
                # The daemon can durably append the message and still return a
                # conflict while the previous supervisor is draining. The
                # durable message hash is the acknowledgement.
                if "HTTP 409" not in str(error):
                    raise
            refreshed = get_events(base, token, session_id)
            message_hash = find_correction_ack(refreshed, start_index, digest)
            if message_hash is None:
                message_hash = wait_for_ack(
                    base, token, session_id, start_index, digest
                )
            correction_triggered = True
            correction_acknowledged = True
            phase_view, phase_events, timed_out = await_phase(deadline)

    final_view = wait_until_stopped(base, token, session_id)
    events = get_events(base, token, session_id)
    measurement = {
        "session_id": session_id,
        "session_status": session_status(final_view),
        "timed_out": timed_out,
        "elapsed_ms": int((time.monotonic() - started) * 1000),
        "mid_run_correction_requested": correction_requested,
        "mid_run_correction_triggered": correction_triggered,
        "mid_run_correction_acknowledged": correction_acknowledged,
        "mid_run_correction_trigger_event_sha256": trigger_hash,
        "mid_run_correction_message_event_sha256": message_hash,
    }
    return measurement, events, final_view


def state_worktree(view: Mapping[str, Any], events: Sequence[Mapping[str, Any]]) -> Path:
    candidates = [view.get("worktree")]
    state = view.get("state")
    if isinstance(state, Mapping):
        candidates.append(state.get("worktree"))
    for event in events:
        if event.get("event") != "worktree_created":
            continue
        data = event.get("data")
        if isinstance(data, Mapping):
            candidates.append(data.get("path"))
    for value in candidates:
        if isinstance(value, str) and value.strip():
            path = Path(value)
            if path.is_absolute():
                return path
    raise E2EError("session view did not expose an absolute worktree")


def extract_patch(worktree: Path) -> bytes:
    env = clean_environment()
    result = run(
        ["git", "-C", str(worktree), "add", "-N", "-A", "--", ".", ":(exclude).purrcode/**"],
        cwd=worktree,
        env=env,
    )
    if result.returncode != 0:
        raise E2EError(f"could not mark untracked files: {result.stderr.strip()}")
    result = run(
        [
            "git",
            "-C",
            str(worktree),
            "diff",
            "--binary",
            "--no-ext-diff",
            "HEAD",
            "--",
            ".",
            ":(exclude).purrcode/**",
        ],
        cwd=worktree,
        env=env,
    )
    if result.returncode != 0:
        raise E2EError(f"could not extract patch: {result.stderr.strip()}")
    patch = result.stdout.encode("utf-8")
    if len(patch) > MAXIMUM_PATCH_BYTES:
        raise E2EError(f"candidate patch exceeds {MAXIMUM_PATCH_BYTES} bytes")
    return patch


def event_metrics(events: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    started = finished = unknown_usage = 0
    input_tokens = output_tokens = 0
    providers: set[str] = set()
    models: set[str] = set()
    for event in events:
        kind = event.get("event")
        data = event.get("data")
        if not isinstance(data, Mapping):
            continue
        if kind == "model_request_started":
            started += 1
            if isinstance(data.get("provider"), str):
                providers.add(data["provider"])
            if isinstance(data.get("model"), str):
                models.add(data["model"])
        elif kind == "model_request_finished":
            finished += 1
            if data.get("input_tokens") is None or data.get("output_tokens") is None:
                unknown_usage += 1
            input_tokens += int(data.get("input_tokens") or 0)
            output_tokens += int(data.get("output_tokens") or 0)
    return {
        "model_calls": started,
        "incomplete_model_calls": max(0, started - finished),
        "unknown_usage_calls": unknown_usage,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "providers": sorted(providers),
        "models": sorted(models),
    }


def alignment_observed(events: Sequence[Mapping[str, Any]]) -> bool:
    kinds = {str(event.get("event")) for event in events}
    return "expectation_contract_created" in kinds or "expectation_contract_revised" in kinds


def run_oracle(
    *,
    task: Mapping[str, Any],
    patch: bytes,
    output: Path,
    oracle_python: Path,
    oracle_runner: Path,
    hf_home: Path,
    timeout_seconds: int,
) -> dict[str, Any]:
    run_id = f"purrcode-{int(task['schedule_index']):03d}-{uuid.uuid4().hex[:12]}"
    predictions_dir = output / "predictions"
    predictions_dir.mkdir(parents=True, exist_ok=True)
    prediction_path = predictions_dir / f"{run_id}.jsonl"
    model_label = "purrcode"
    prediction_path.write_text(
        json.dumps(
            {
                "instance_id": task["base_task_id"],
                "model_name_or_path": model_label,
                "model_patch": patch.decode("utf-8", errors="replace"),
            },
            ensure_ascii=False,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    oracle_work = output / "oracle-work"
    oracle_work.mkdir(parents=True, exist_ok=True)
    log_dir = output / "oracle-cli"
    log_dir.mkdir(parents=True, exist_ok=True)
    stdout_path = log_dir / f"{run_id}.stdout"
    stderr_path = log_dir / f"{run_id}.stderr"
    env = clean_environment(hf_home=hf_home)
    command = [
        str(oracle_python),
        str(oracle_runner),
        "--instance-ids",
        str(task["base_task_id"]),
        "--predictions-path",
        str(prediction_path),
        "--evaluate-empty",
        "--max-workers",
        "1",
        "--timeout",
        str(timeout_seconds),
        "--cache-level",
        "instance",
        "--run-id",
        run_id,
        "--report-dir",
        str(output / "oracle-reports"),
    ]
    completed = run(command, cwd=oracle_work, env=env, timeout=timeout_seconds + 300)
    stdout_path.write_text(completed.stdout, encoding="utf-8")
    stderr_path.write_text(completed.stderr, encoding="utf-8")
    report_path = oracle_work / "logs" / "run_evaluation" / run_id / model_label / str(task["base_task_id"]) / "report.json"
    if completed.returncode != 0 or not report_path.is_file():
        return {
            "status": "unknown",
            "detail": completed.stderr[-2000:] or completed.stdout[-2000:] or "oracle did not write a report",
            "oracle_run_id": run_id,
            "oracle_report_sha256": None,
        }
    try:
        report = json.loads(report_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return {
            "status": "unknown",
            "detail": f"oracle report is unreadable: {error}",
            "oracle_run_id": run_id,
            "oracle_report_sha256": None,
        }
    entry = report.get(str(task["base_task_id"])) if isinstance(report, Mapping) else None
    if not isinstance(entry, Mapping):
        return {
            "status": "unknown",
            "detail": "oracle report omitted the instance entry",
            "oracle_run_id": run_id,
            "oracle_report_sha256": file_hash(report_path),
        }
    if entry.get("resolved") is True:
        status = "correct"
    elif entry.get("resolved") is False:
        status = "incorrect"
    else:
        status = "unknown"
    return {
        "status": status,
        "detail": {
            "resolved": entry.get("resolved"),
            "patch_successfully_applied": entry.get("patch_successfully_applied"),
        },
        "oracle_run_id": run_id,
        "oracle_report_sha256": file_hash(report_path),
    }


def prepare_bundle(*, manifest_path: Path, revision_path: Path, cache_dir: Path, out: Path, model: str,
                   study_id: str = "repository-completion-swebench-live-v1") -> dict[str, Any]:
    if out.exists() and any(out.iterdir()):
        raise E2EError(f"refusing to reuse non-empty output directory: {out}")
    manifest = load_json(manifest_path)
    revision = load_json(revision_path)
    tasks = manifest.get("tasks")
    revisions = revision.get("tasks")
    if not isinstance(tasks, list) or not isinstance(revisions, list) or len(tasks) != 30 or len(revisions) != 30:
        raise E2EError("manifest and requirement revision must each contain exactly 30 tasks")
    revision_by_id = {str(item["instance_id"]): item for item in revisions if isinstance(item, Mapping)}
    if len(revision_by_id) != 30:
        raise E2EError("requirement revision contains duplicate or malformed tasks")
    plan = {
        "primary_model": model,
        "repositories": {},
        "request_variants": list(analysis.REQUEST_VARIANTS),
        "repeats": list(analysis.REPEAT_LABELS),
    }
    for task in tasks:
        repository = str(task["repo"])
        plan["repositories"].setdefault(repository, []).append(str(task["instance_id"]))
    schedule = analysis.schedule_payload(plan)
    entries: list[dict[str, Any]] = []
    for index, cell in enumerate(schedule):
        task_id = str(cell["base_task_id"])
        task = next((item for item in tasks if str(item["instance_id"]) == task_id), None)
        meta = revision_by_id.get(task_id)
        if not isinstance(task, Mapping) or not isinstance(meta, Mapping):
            raise E2EError(f"missing task or revision metadata for {task_id}")
        full = str(meta["full_statement"])
        p0 = str(meta["abridged_p0"])
        p1 = str(meta["p1_text"])
        if hashlib.sha256(full.encode("utf-8")).hexdigest() != str(meta["full_statement_sha256"]):
            raise E2EError(f"{task_id}: full statement hash mismatch")
        if hashlib.sha256(p0.encode("utf-8")).hexdigest() != str(meta["abridged_p0_sha256"]):
            raise E2EError(f"{task_id}: P0 hash mismatch")
        if hashlib.sha256(p1.encode("utf-8")).hexdigest() != str(meta["p1_text_sha256"]):
            raise E2EError(f"{task_id}: P1 hash mismatch")
        variant = str(cell["variant"])
        arm = str(cell["arm"])
        repeat = int(cell["repeat"])
        run_id = f"{task_id}:{variant}:{arm}:{repeat}"
        entries.append(
            {
                "schedule_index": index,
                "run_id": run_id,
                "task_id": run_id,
                "base_task_id": task_id,
                "repository": str(cell["repository"]),
                "variant": variant,
                "arm": arm,
                "repeat": repeat,
                "model": model,
                "objective": full if variant == "static" else p0,
                "mid_run_correction": None if variant == "static" else p1,
                "checkout": task_id,
                "repo_url": str(task["repo_url"]),
                "base_commit": str(task["base_commit"]),
                "max_tokens": analysis.SESSION_TOKEN_BUDGET,
                "maximum_seconds": DEFAULT_MAXIMUM_SECONDS,
            }
        )
    bundle = {
        "schema_version": 1,
        "study_id": study_id,
        "model": model,
        "dataset_manifest_sha256": canonical_hash(manifest),
        "requirement_revision_sha256": canonical_hash(revision),
        "tasks": entries,
    }
    out.mkdir(parents=True, exist_ok=False)
    bundle_path = out / "run-bundle.json"
    write_json(bundle_path, bundle)
    return {"bundle": str(bundle_path), "bundle_sha256": file_hash(bundle_path), "task_count": len(entries)}


def read_results(path: Path) -> dict[str, dict[str, Any]]:
    rows: dict[str, dict[str, Any]] = {}
    if not path.exists():
        return rows
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            raise E2EError(f"{path}:{line_number}: invalid JSON: {error}") from error
        if not isinstance(row, dict) or not isinstance(row.get("run_id"), str):
            raise E2EError(f"{path}:{line_number}: invalid result row")
        if row["run_id"] in rows:
            raise E2EError(f"{path}:{line_number}: duplicate run ID")
        rows[row["run_id"]] = row
    return rows


def append_result(path: Path, row: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
        handle.flush()
        os.fsync(handle.fileno())


def error_row(
    task: Mapping[str, Any],
    *,
    plan: Mapping[str, Any],
    freeze: Mapping[str, Any],
    bundle_sha256: str,
    detail: str,
) -> dict[str, Any]:
    return {
        "run_id": task["run_id"],
        "task_id": task["task_id"],
        "repository": task["repository"],
        "base_task_id": task["base_task_id"],
        "variant": task["variant"],
        "arm": task["arm"],
        "repeat": task["repeat"],
        "model": task["model"],
        "study_id": plan["study_id"],
        "plan_sha256": canonical_hash(plan),
        "freeze_sha256": canonical_hash(freeze),
        "dataset_manifest_sha256": plan["dataset_manifest_sha256"],
        "schedule_sha256": plan["schedule_sha256"],
        "bundle_sha256": bundle_sha256,
        "terminal_status": "error",
        "oracle_status": "unknown",
        "claimed_complete": None,
        "externally_correct": None,
        "mid_run_correction_requested": task["variant"] == "correction",
        "mid_run_correction_triggered": False,
        "mid_run_correction_acknowledged": False,
        "mid_run_correction_trigger_event_sha256": None,
        "mid_run_correction_message_event_sha256": None,
        "detail": detail[:4000],
        "created_at": utc_now(),
    }


def run_one(
    task: Mapping[str, Any],
    *,
    cache_dir: Path,
    base: str,
    token: str,
    output: Path,
    plan: Mapping[str, Any],
    freeze: Mapping[str, Any],
    bundle_sha256: str,
    oracle_python: Path,
    oracle_runner: Path,
    hf_home: Path,
    oracle_timeout: int,
) -> dict[str, Any]:
    repository = verify_checkout(task, cache_dir)
    measurement, events, view = run_session(
        base=base,
        token=token,
        task=task,
        repository=repository,
        maximum_seconds=int(task["maximum_seconds"]),
    )
    worktree = state_worktree(view, events)
    patch = extract_patch(worktree)
    patch_dir = output / "patches"
    patch_dir.mkdir(parents=True, exist_ok=True)
    patch_path = patch_dir / f"{safe_slug(str(task['run_id']))}.patch"
    patch_path.write_bytes(patch)
    audit_dir = output / "session-audit"
    audit_dir.mkdir(parents=True, exist_ok=True)
    write_json(audit_dir / f"{safe_slug(str(task['run_id']))}.json", {
        "measurement": measurement, "events": events,
        "patch_sha256": hashlib.sha256(patch).hexdigest(),
        "kind": "agent_events_only_no_hidden_oracle_inputs",
    })
    oracle = run_oracle(
        task=task,
        patch=patch,
        output=output,
        oracle_python=oracle_python,
        oracle_runner=oracle_runner,
        hf_home=hf_home,
        timeout_seconds=oracle_timeout,
    )
    metrics = event_metrics(events)
    model_output_observed = any(
        (event.get("event") == "action_proposed" and event.get("data", {}).get("turn_id") is not None)
        or (event.get("event") == "conversation_message_added"
            and event.get("data", {}).get("message", {}).get("role") == "assistant"
            and event.get("data", {}).get("message", {}).get("model") is not None
            and bool(event.get("data", {}).get("message", {}).get("content", "").strip()))
        for event in events
    )
    requested_alignment = task["arm"] == "C"
    observed_alignment = alignment_observed(events)
    terminal_status = "valid"
    if measurement["timed_out"]:
        terminal_status = "timeout"
    elif measurement["session_status"] in {"failed", "cancelled"}:
        terminal_status = "error"
    if terminal_status != "valid":
        claimed_complete = None
        oracle_status = "unknown"
    else:
        claimed_complete = measurement["session_status"] == "completed"
        oracle_status = str(oracle["status"])
    row = {
        "run_id": task["run_id"],
        "task_id": task["task_id"],
        "repository": task["repository"],
        "base_task_id": task["base_task_id"],
        "variant": task["variant"],
        "arm": task["arm"],
        "repeat": task["repeat"],
        "model": task["model"],
        "study_id": plan["study_id"],
        "plan_sha256": canonical_hash(plan),
        "freeze_sha256": canonical_hash(freeze),
        "dataset_manifest_sha256": plan["dataset_manifest_sha256"],
        "schedule_sha256": plan["schedule_sha256"],
        "bundle_sha256": bundle_sha256,
        "terminal_status": terminal_status,
        "model_output_observed": model_output_observed,
        "oracle_status": oracle_status,
        "claimed_complete": claimed_complete,
        "externally_correct": None if oracle_status == "unknown" else oracle_status == "correct",
        "session_id": measurement["session_id"],
        "session_status": measurement["session_status"],
        "elapsed_ms": measurement["elapsed_ms"],
        "alignment_requested": "enabled" if requested_alignment else "disabled",
        "alignment_observed": observed_alignment,
        "mid_run_correction_requested": measurement["mid_run_correction_requested"],
        "mid_run_correction_triggered": measurement["mid_run_correction_triggered"],
        "mid_run_correction_acknowledged": measurement["mid_run_correction_acknowledged"],
        "mid_run_correction_trigger_event_sha256": measurement["mid_run_correction_trigger_event_sha256"],
        "mid_run_correction_message_event_sha256": measurement["mid_run_correction_message_event_sha256"],
        "patch_sha256": hashlib.sha256(patch).hexdigest(),
        "patch_bytes": len(patch),
        "oracle_run_id": oracle["oracle_run_id"],
        "oracle_report_sha256": oracle.get("oracle_report_sha256"),
        "oracle_detail": oracle.get("detail"),
        "session_events_sha256": canonical_hash(events),
        "session_events": len(events),
        **metrics,
        "created_at": utc_now(),
    }
    return row


def command_prepare(args: argparse.Namespace) -> int:
    report = prepare_bundle(
        manifest_path=args.manifest,
        revision_path=args.requirement_revision,
        cache_dir=args.cache_dir,
        out=args.out,
        model=args.model,
        study_id=args.study_id,
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


def command_run(args: argparse.Namespace) -> int:
    lock = load_json(args.runner_lock)
    artifacts = lock.get("artifacts")
    if not isinstance(artifacts, list):
        raise E2EError("runner lock is missing its artifact list")
    for artifact in artifacts:
        if not isinstance(artifact, Mapping):
            raise E2EError("runner lock contains a malformed artifact entry")
        path = (REPO_ROOT / str(artifact["path"])).resolve()
        if file_hash(path) != str(artifact["sha256"]):
            raise E2EError(f"frozen runner artifact changed: {artifact['path']}")
    if file_hash(args.bundle) != str(lock.get("bundle_sha256")):
        raise E2EError("run bundle does not match the frozen runner lock")
    bundle = load_json(args.bundle)
    tasks = bundle.get("tasks")
    if not isinstance(tasks, list) or len(tasks) != analysis.RUNS_PER_MODEL:
        raise E2EError("run bundle must contain exactly 120 scheduled tasks")
    plan = load_json(args.plan)
    freeze = load_json(args.freeze)
    analysis.load_plan_object(plan, source=str(args.plan))
    analysis.load_freeze_object(freeze, source=str(args.freeze))
    bundle_sha256 = file_hash(args.bundle)
    token = args.token_file.read_text(encoding="utf-8").strip()
    if not token or any(character in token for character in "\r\n\t"):
        raise E2EError("daemon token is empty or malformed")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    results_path = output / "results.jsonl"
    existing = read_results(results_path)
    selected_ids = set(args.instance_ids or [])
    selected_arms = set(args.arms)
    selected_variants = set(args.variants)
    limit = args.max_runs
    completed_this_invocation = 0
    for task in tasks:
        if not isinstance(task, Mapping):
            raise E2EError("run bundle contains a non-object task")
        if selected_ids and task["base_task_id"] not in selected_ids:
            continue
        if task["arm"] not in selected_arms or task["variant"] not in selected_variants:
            continue
        if task["run_id"] in existing and not args.rerun:
            continue
        if limit is not None and completed_this_invocation >= limit:
            break
        print(f"[{task['schedule_index']:03d}] {task['run_id']}", flush=True)
        started_dir = output / "started"
        started_dir.mkdir(exist_ok=True)
        started_path = started_dir / f"{safe_slug(str(task['run_id']))}.json"
        if args.rerun:
            raise E2EError("frozen scientific cells cannot be rerun in the same output directory")
        if started_path.exists():
            row = error_row(task, plan=plan, freeze=freeze,
                            bundle_sha256=bundle_sha256,
                            detail="interrupted scheduled cell; retained without replay")
            append_result(results_path, row)
            existing[str(task["run_id"])] = row
            completed_this_invocation += 1
            continue
        with started_path.open("x", encoding="utf-8") as handle:
            json.dump({"run_id": task["run_id"], "started_at": utc_now(),
                       "bundle_sha256": bundle_sha256}, handle)
            handle.flush()
            os.fsync(handle.fileno())
        try:
            row = run_one(
                task,
                cache_dir=args.cache_dir,
                base=args.daemon.rstrip("/"),
                token=token,
                output=output,
                plan=plan,
                freeze=freeze,
                bundle_sha256=bundle_sha256,
                oracle_python=args.oracle_python,
                oracle_runner=args.oracle_runner,
                hf_home=args.hf_home,
                oracle_timeout=args.oracle_timeout,
            )
        except Exception as error:  # noqa: BLE001 - a scheduled ITT row is required
            row = error_row(
                task,
                plan=plan,
                freeze=freeze,
                bundle_sha256=bundle_sha256,
                detail=str(error),
            )
        append_result(results_path, row)
        existing[str(task["run_id"])] = row
        completed_this_invocation += 1
        print(f"  {row['terminal_status']} / {row['oracle_status']}", flush=True)
    print(f"results: {results_path}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    prepare = subparsers.add_parser("prepare")
    prepare.add_argument("--manifest", type=Path, required=True)
    prepare.add_argument("--requirement-revision", type=Path, required=True)
    prepare.add_argument("--cache-dir", type=Path, required=True)
    prepare.add_argument("--out", type=Path, required=True)
    prepare.add_argument("--model", required=True)
    prepare.add_argument("--study-id", default="repository-completion-swebench-live-v1")
    prepare.set_defaults(func=command_prepare)
    run_parser = subparsers.add_parser("run")
    run_parser.add_argument("--bundle", type=Path, required=True)
    run_parser.add_argument("--runner-lock", type=Path, required=True)
    run_parser.add_argument("--cache-dir", type=Path, required=True)
    run_parser.add_argument("--plan", type=Path, required=True)
    run_parser.add_argument("--freeze", type=Path, required=True)
    run_parser.add_argument("--daemon", default="http://127.0.0.1:7477")
    run_parser.add_argument("--token-file", type=Path, required=True)
    run_parser.add_argument("--output", type=Path, required=True)
    run_parser.add_argument("--oracle-python", type=Path, required=True)
    run_parser.add_argument("--oracle-runner", type=Path, required=True)
    run_parser.add_argument("--hf-home", type=Path, required=True)
    run_parser.add_argument("--oracle-timeout", type=int, default=DEFAULT_ORACLE_TIMEOUT)
    run_parser.add_argument("--instance-ids", nargs="*")
    run_parser.add_argument("--arms", nargs="+", choices=("B", "C"), default=list(analysis.PRIMARY_ARMS))
    run_parser.add_argument("--variants", nargs="+", choices=analysis.REQUEST_VARIANTS, default=list(analysis.REQUEST_VARIANTS))
    run_parser.add_argument("--max-runs", type=int)
    run_parser.add_argument("--rerun", action="store_true")
    run_parser.set_defaults(func=command_run)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        return int(args.func(args))
    except (E2EError, analysis.AnalysisError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
