//! Run the v1.5 alignment benchmark against a live daemon (§31–§32).
//!
//! ```text
//! purrcode-alignment-benchmark \
//!     --daemon http://127.0.0.1:4477 --token "$PURRCODE_TOKEN" \
//!     --repository benchmarks/alignment-fixture --out target/alignment-report.md
//! ```
//!
//! Twenty tasks, each against a fresh copy of the fixture repository. What is
//! measured comes from two places, and neither of them is the agent:
//!
//! - **The durable log** answers whether the gate ran, what it concluded, and
//!   whether the agent stopped anyway. This is the metric the release lives or
//!   dies by, and reading it from the agent's summary would be asking the
//!   defendant to record the verdict.
//! - **The tree the run left behind** answers whether the requirements were
//!   actually met. It cannot come from the session's own reviewers: they are
//!   the thing under test, and a run where they were wrong looks identical from
//!   inside to one where they were right.
//!
//! The five trap tasks carry a correction the harness sends mid-run, once the
//! agent has committed to a plan. The whole trap is what happens next — the
//! contract must be *revised*, so the work done under the old wording stops
//! counting, rather than the correction being acknowledged and the old plan
//! carried on with.
//!
//! Scoring lives in `alignment::AlignmentReport`, which the unit tests cover.
//! What lives here is the part that cannot be unit-tested: an HTTP conversation
//! with a real daemon and a real model.

use purrcode_evaluation_runtime::alignment::{
    ALIGNMENT_SCHEMA_VERSION, AlignmentOutcome, AlignmentReport, AlignmentRun, AlignmentTask,
    TaskCheck, default_catalog,
};
use purrcode_runtime_core::{SessionEvent, SessionState};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_CONFIRM_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);

struct Options {
    daemon: String,
    token: String,
    repository: PathBuf,
    out: Option<PathBuf>,
    json_out: Option<PathBuf>,
    only: Option<String>,
}

#[derive(serde::Serialize)]
struct MeasurementError {
    task_id: String,
    status: &'static str,
    detail: String,
}

fn usage() -> ! {
    eprintln!(
        "usage: alignment-benchmark --daemon URL --repository PATH \\\n\
         \x20                        (--token TOKEN | --token-file FILE) \
         [--out FILE] [--json-out FILE] [--only TASK_ID]\n\n\
         Runs the twenty-task alignment catalog and reports whether the agent finished\n\
         the task the user asked for — and stopped when it hadn't."
    );
    std::process::exit(2)
}

fn parse_options() -> Options {
    let mut daemon = None;
    let mut token = None;
    let mut token_file = None;
    let mut repository = None;
    let mut out = None;
    let mut json_out = None;
    let mut only = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--daemon" => daemon = Some(value()),
            "--token" => token = Some(value()),
            "--token-file" => token_file = Some(PathBuf::from(value())),
            "--repository" => repository = Some(PathBuf::from(value())),
            "--out" => out = Some(PathBuf::from(value())),
            "--json-out" => json_out = Some(PathBuf::from(value())),
            "--only" => only = Some(value()),
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    if token.is_some() && token_file.is_some() {
        eprintln!("pass only one of --token or --token-file");
        usage();
    }
    let token = token.or_else(|| {
        token_file.map(|path| {
            std::fs::read_to_string(&path)
                .unwrap_or_else(|error| {
                    eprintln!("could not read token file {}: {error}", path.display());
                    std::process::exit(2)
                })
                .trim()
                .to_owned()
        })
    });
    Options {
        daemon: daemon.unwrap_or_else(|| usage()),
        token: token
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| usage()),
        repository: repository.unwrap_or_else(|| usage()),
        out,
        json_out,
        only,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options();
    let client = reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()?;
    let catalog: Vec<AlignmentTask> = default_catalog()
        .into_iter()
        .filter(|task| {
            options
                .only
                .as_ref()
                .is_none_or(|wanted| &task.id == wanted)
        })
        .collect();
    if catalog.is_empty() {
        eprintln!("no task matched --only");
        std::process::exit(2);
    }

    let mut outcomes = Vec::new();
    let mut measurement_errors = Vec::new();
    for task in &catalog {
        eprintln!("▶ {} ({})", task.id, task.kind.label());
        let outcome = match clone_fixture(&options.repository, &task.id) {
            Ok(repository) => match run_task(&client, &options, task, &repository).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("  infrastructure error: {error}");
                    measurement_errors.push(MeasurementError {
                        task_id: task.id.clone(),
                        status: "infrastructure_error",
                        detail: error.to_string(),
                    });
                    failed_outcome(task)
                }
            },
            Err(error) => {
                eprintln!("  fixture error: {error}");
                measurement_errors.push(MeasurementError {
                    task_id: task.id.clone(),
                    status: "infrastructure_error",
                    detail: error.to_string(),
                });
                failed_outcome(task)
            }
        };
        eprintln!(
            "  gate={} · {} / {} requirement check(s) · {}{}",
            outcome
                .run
                .delivery_state
                .as_deref()
                .unwrap_or("never ran — the session stopped with no gate behind it"),
            outcome.satisfied_requirements,
            outcome.expected_requirements,
            if outcome.succeeded() { "pass" } else { "FAIL" },
            if outcome.violated_forbidden {
                " · did something the task forbade"
            } else {
                ""
            }
        );
        outcomes.push(outcome);
        if let Some(path) = options.json_out.as_ref() {
            write_json_atomic(path, &AlignmentReport::new(outcomes.clone()))?;
            write_measurement_errors(path, &measurement_errors)?;
            eprintln!("  wrote partial {}", path.display());
        }
    }

    let report = AlignmentReport::new(outcomes);
    let markdown = report.to_markdown();
    match options.out.as_ref() {
        Some(path) => {
            std::fs::write(path, &markdown)?;
            eprintln!("\nwrote {}", path.display());
        }
        None => println!("{markdown}"),
    }
    if let Some(path) = options.json_out.as_ref() {
        write_json_atomic(path, &report)?;
        write_measurement_errors(path, &measurement_errors)?;
        eprintln!("wrote {}", path.display());
    }
    // A non-zero exit when the bar is not met, so this is usable from CI
    // without a human reading the markdown.
    if report.meets_release_bar() && measurement_errors.is_empty() {
        Ok(())
    } else {
        eprintln!("the release bar was not met");
        if !measurement_errors.is_empty() {
            eprintln!(
                "  · {} task measurement error(s); see the error sidecar",
                measurement_errors.len()
            );
        }
        std::process::exit(1)
    }
}

/// Copy the fixture so each task starts from the same clean state.
///
/// Copied and re-initialised rather than `git clone`d: the fixture lives inside
/// the PurrCode repository and is not a repository of its own, so cloning its
/// directory would either fail or drag the whole workspace along. Each task
/// gets a standalone repository at one commit, which is also what the agent's
/// own worktree isolation expects to find.
fn clone_fixture(source: &Path, suffix: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let destination = unique_directory(&format!("purrcode-alignment-{}", safe_suffix(suffix)))?;
    copy_tree(source, &destination)?;
    let git = |arguments: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        let status = std::process::Command::new("git")
            .args(arguments)
            .env_clear()
            .envs(clean_environment())
            .current_dir(&destination)
            .status()?;
        if !status.success() {
            return Err(format!("git {arguments:?} failed in {}", destination.display()).into());
        }
        Ok(())
    };
    git(&["init", "-q"])?;
    git(&["add", "-A"])?;
    git(&[
        "-c",
        "user.name=PurrCode Benchmark",
        "-c",
        "user.email=benchmark@local.invalid",
        "commit",
        "-q",
        "-m",
        "fixture",
    ])?;
    Ok(destination)
}

/// Allocate a fresh directory without ever removing a path left by an earlier
/// run. The create operation itself is the collision check, so two benchmark
/// processes starting in the same nanosecond still cannot share a fixture.
fn unique_directory(prefix: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = std::env::temp_dir();
    let pid = std::process::id();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..100_u32 {
        let candidate = root.join(format!("{prefix}-{pid}-{stamp}-{attempt}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(format!("could not allocate a unique directory for {prefix}").into())
}

fn safe_suffix(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Keep validation and fixture-management children away from provider
/// credentials and other ambient process state. Cargo/Rustup roots are
/// explicitly allowed so a local toolchain can still be selected.
fn clean_environment() -> Vec<(String, String)> {
    [
        "PATH",
        "TMPDIR",
        "TEMP",
        "LANG",
        "LC_ALL",
        "TERM",
        "SystemRoot",
        "WINDIR",
        "CARGO_HOME",
        "RUSTUP_HOME",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
    .collect()
}

fn failed_outcome(task: &AlignmentTask) -> AlignmentOutcome {
    AlignmentOutcome {
        schema_version: ALIGNMENT_SCHEMA_VERSION,
        task_id: task.id.clone(),
        kind: task.kind,
        run: AlignmentRun {
            delivery_state: Some("infrastructure_error".into()),
            ..AlignmentRun::default()
        },
        satisfied_requirements: 0,
        expected_requirements: task.requirement_checks.len() as u32,
        // Infrastructure failure leaves the forbidden checks unmeasured. The
        // independent measurement-error sidecar records why this attempt is
        // excluded from the release bar.
        violated_forbidden: false,
    }
}

fn write_json_atomic<T: serde::Serialize + ?Sized>(
    path: &Path,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid JSON output path {}", path.display()))?;
    let pid = std::process::id();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".{file_name}.tmp-{pid}-{stamp}"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let rendered = serde_json::to_string_pretty(value)?;
        file.write_all(rendered.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_measurement_errors(
    report_path: &Path,
    errors: &[MeasurementError],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut sidecar = report_path.to_owned();
    sidecar.set_extension("errors.json");
    write_json_atomic(&sidecar, errors)
}

/// Copy a directory tree, skipping build output and any repository metadata.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if fs::symlink_metadata(source)?.file_type().is_symlink() {
        return Err(format!(
            "refusing to copy symlinked benchmark root {}",
            source.display()
        )
        .into());
    }
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git") | Some("target")) {
            continue;
        }
        if entry.file_type()?.is_symlink() {
            return Err(format!(
                "refusing to copy symlink {} into benchmark tree",
                entry.path().display()
            )
            .into());
        }
        let from = entry.path();
        let to = destination.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Run one task to a settled state and score it.
async fn run_task(
    client: &reqwest::Client,
    options: &Options,
    task: &AlignmentTask,
    repository: &Path,
) -> Result<AlignmentOutcome, Box<dyn std::error::Error>> {
    let base = options.daemon.trim_end_matches('/');
    let started = Instant::now();
    let created: serde_json::Value = post_json(
        client,
        &format!("{base}/v1/sessions"),
        &options.token,
        &serde_json::json!({
            "objective": task.objective,
            "repository": repository,
            "task_mode": "build",
            "permission_mode": "auto",
        }),
    )
    .await?;
    let session_id = created["id"]
        .as_str()
        .ok_or("the daemon did not return a session id")?
        .to_owned();

    let result =
        run_task_after_create(client, options, task, repository, &session_id, started).await;
    if let Err(error) = &result {
        if let Err(cleanup_error) =
            cancel_and_confirm(client, base, &options.token, &session_id).await
        {
            return Err(format!(
                "task failed: {error}; cleanup could not confirm stopped lease/workers: {cleanup_error}"
            )
            .into());
        }
    }
    result
}

async fn run_task_after_create(
    client: &reqwest::Client,
    options: &Options,
    task: &AlignmentTask,
    repository: &Path,
    session_id: &str,
    started: Instant,
) -> Result<AlignmentOutcome, Box<dyn std::error::Error>> {
    let base = options.daemon.trim_end_matches('/');
    let deadline = Duration::from_secs(task.maximum_seconds);
    let mut corrections_issued = 0_u32;
    let mut correction_pending = task.correction.clone();
    let mut timed_out = false;

    loop {
        if started.elapsed() >= deadline {
            timed_out = true;
            break;
        }
        let session = fetch_session(client, base, &options.token, &session_id).await?;
        let status = session_status(&session);
        if started.elapsed() >= deadline {
            timed_out = true;
            break;
        }

        // The correction goes in once the agent has a contract and has started
        // acting on it. Sending it before there is a plan to correct tests
        // nothing — the trap is that work already done under the old wording
        // must stop counting.
        if let Some(correction) = correction_pending.clone()
            && committed_to_a_plan(client, options, &session_id).await?
        {
            if started.elapsed() >= deadline {
                timed_out = true;
                break;
            }
            let _: serde_json::Value = post_json(
                client,
                &format!("{base}/v1/sessions/{session_id}/messages"),
                &options.token,
                &serde_json::json!({ "content": correction }),
            )
            .await?;
            corrections_issued += 1;
            correction_pending = None;
            eprintln!("  ↩ sent the correction");
        }

        let settled = matches!(
            status.as_str(),
            "completed" | "failed" | "cancelled" | "paused" | "awaiting_review"
        );
        // A correction that has not been delivered yet keeps the run going: a
        // session that finished before the harness could correct it did not run
        // this task.
        if settled && correction_pending.is_none() {
            break;
        }
        if started.elapsed() >= deadline {
            eprintln!("  ⏱ timed out");
            timed_out = true;
            break;
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        tokio::time::sleep(std::cmp::min(POLL_INTERVAL, remaining)).await;
    }

    if timed_out {
        // A timeout is a lifecycle event, not permission to inspect a moving
        // tree. The daemon's cancel route is authoritative; after it returns,
        // require an independent lease/worker drain confirmation.
        request_cancel(
            client,
            base,
            &options.token,
            &session_id,
            "benchmark task deadline exceeded",
        )
        .await?;
    }

    // Do this for every terminal path, not only a timeout: a status without a
    // drained lease is not a stable artifact boundary for external scoring.
    wait_until_stopped(client, base, &options.token, &session_id).await?;

    let events: Vec<SessionEvent> = get_json(
        client,
        &format!("{base}/v1/sessions/{session_id}/events"),
        &options.token,
    )
    .await?;
    let mut state = SessionState::empty(purrcode_runtime_core::SessionId(session_id.parse()?));
    for event in &events {
        state.reduce_event(event)?;
    }
    let mut run = AlignmentRun::from_session(&state, &events, started.elapsed().as_secs());
    run.corrections_issued = corrections_issued;

    // The tree the run left behind, which is where the requirements are
    // actually settled. The session's worktree when it has one — that is where
    // the work happened — and the clone otherwise.
    let tree = state
        .worktree
        .clone()
        .unwrap_or_else(|| repository.to_owned());
    // Structural checks read this frozen copy. The validator receives a
    // second copy because tests and Cargo are allowed to write build metadata
    // while they run; those writes must never alter the scored snapshot.
    let snapshot = snapshot_final_tree(&tree, &task.id)?;
    let validation_tree = unique_directory(&format!(
        "purrcode-alignment-validation-tree-{}",
        safe_suffix(&task.id)
    ))?;
    copy_tree(&snapshot, &validation_tree)?;
    let validation_target = unique_directory(&format!(
        "purrcode-alignment-validation-{}",
        safe_suffix(&task.id)
    ))?;
    let mut validation_result = None;
    let mut satisfied = 0_u32;
    for check in &task.requirement_checks {
        if evaluate_check(
            check,
            &snapshot,
            repository,
            &validation_tree,
            &validation_target,
            &mut validation_result,
        )
        .await?
        {
            satisfied += 1;
        } else {
            eprintln!("  ✗ {}", check.label());
        }
    }
    // A forbidden check states what must still be true. One that fails is the
    // forbidden thing having happened.
    let mut violated_forbidden = false;
    for check in &task.forbidden_checks {
        if !evaluate_check(
            check,
            &snapshot,
            repository,
            &validation_tree,
            &validation_target,
            &mut validation_result,
        )
        .await?
        {
            eprintln!("  ⚠ forbidden: {}", check.label());
            violated_forbidden = true;
        }
    }

    Ok(AlignmentOutcome {
        schema_version: ALIGNMENT_SCHEMA_VERSION,
        task_id: task.id.clone(),
        kind: task.kind,
        run,
        satisfied_requirements: satisfied,
        expected_requirements: task.requirement_checks.len() as u32,
        violated_forbidden,
    })
}

async fn post_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    body: &serde_json::Value,
) -> Result<T, Box<dyn std::error::Error>> {
    let response = client
        .post(url)
        .bearer_auth(token)
        .json(body)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

async fn fetch_session(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session_id: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    get_json(client, &format!("{base}/v1/sessions/{session_id}"), token).await
}

fn session_status(session: &serde_json::Value) -> String {
    session["status_code"]
        .as_str()
        .or_else(|| session["status"].as_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

async fn request_cancel(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session_id: &str,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = client
        .post(format!("{base}/v1/sessions/{session_id}/cancel"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "reason": reason }))
        .send()
        .await?;
    let already_terminal = response.status() == StatusCode::CONFLICT;
    match response.error_for_status() {
        Ok(response) => {
            let _: serde_json::Value = response.json().await?;
            Ok(())
        }
        // A task can cross the terminal boundary immediately before the
        // timeout request. It is still scored only after the drain check below.
        Err(_) if already_terminal => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn cancel_and_confirm(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let cancel_error = request_cancel(
        client,
        base,
        token,
        session_id,
        "benchmark task failed before scoring",
    )
    .await
    .err();
    let stopped = wait_until_stopped(client, base, token, session_id).await;
    match (cancel_error, stopped) {
        (None, Ok(_)) => Ok(()),
        (Some(cancel), Ok(_)) => {
            Err(format!("cancel request failed after drain confirmation: {cancel}").into())
        }
        (None, Err(stop)) => Err(format!("cancelled session did not drain: {stop}").into()),
        (Some(cancel), Err(stop)) => {
            Err(format!("cancel request failed: {cancel}; drain failed: {stop}").into())
        }
    }
}

fn session_stopped(session: &serde_json::Value) -> bool {
    session["lease_active"].as_bool() == Some(false)
        && matches!(
            session_status(session).as_str(),
            "completed"
                | "cancelled"
                | "failed"
                | "paused"
                | "awaiting_review"
                | "awaiting_approval"
                | "ready"
        )
}

fn supervisor_workers_stopped(supervisor: &serde_json::Value) -> bool {
    supervisor
        .get("workers")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|workers| {
            workers
                .iter()
                .all(|worker| worker["status"].as_str().is_some_and(worker_status_stopped))
        })
}

fn worker_status_stopped(status: &str) -> bool {
    status.eq_ignore_ascii_case("completed")
        || status.eq_ignore_ascii_case("interrupted")
        || status.eq_ignore_ascii_case("failed")
        || status.to_ascii_lowercase().starts_with("failed:")
        || status
            .to_ascii_lowercase()
            .starts_with("skipped dependency:")
}

async fn wait_until_stopped(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session_id: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + STOP_CONFIRM_TIMEOUT;
    loop {
        let session = fetch_session(client, base, token, session_id).await?;
        let supervisor: serde_json::Value =
            get_json(client, &format!("{base}/v1/supervisor/{session_id}"), token).await?;
        if session_stopped(&session) && supervisor_workers_stopped(&supervisor) {
            return Ok(session);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "could not confirm stopped session {session_id}: status={}, lease_active={:?}, workers_stopped={}",
                session_status(&session),
                session["lease_active"].as_bool(),
                supervisor_workers_stopped(&supervisor)
            )
            .into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn snapshot_final_tree(tree: &Path, task_id: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let snapshot = unique_directory(&format!(
        "purrcode-alignment-snapshot-{}",
        safe_suffix(task_id)
    ))?;
    copy_tree(tree, &snapshot)?;
    Ok(snapshot)
}

async fn evaluate_check(
    check: &TaskCheck,
    worktree: &Path,
    baseline: &Path,
    validation_tree: &Path,
    validation_target: &Path,
    validation_result: &mut Option<bool>,
) -> Result<bool, Box<dyn std::error::Error>> {
    match check {
        TaskCheck::Contains { path, needle } => Ok(fs::read_to_string(worktree.join(path))
            .map(|contents| contents.contains(needle.as_str()))
            .unwrap_or(false)),
        TaskCheck::Lacks { path, needle } => Ok(fs::read_to_string(worktree.join(path))
            .map(|contents| !contents.contains(needle.as_str()))
            .unwrap_or(false)),
        TaskCheck::Exists { path } => Ok(worktree.join(path).exists()),
        TaskCheck::Unchanged { path } => Ok(
            match (fs::read(worktree.join(path)), fs::read(baseline.join(path))) {
                (Ok(after), Ok(before)) => after == before,
                _ => false,
            },
        ),
        TaskCheck::TestsPass => match validation_result {
            Some(result) => Ok(*result),
            None => {
                let result = run_validation(validation_tree, validation_target).await?;
                *validation_result = Some(result);
                Ok(result)
            }
        },
    }
}

async fn run_validation(
    worktree: &Path,
    target_dir: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    // This is an external oracle process with a filtered environment. It is
    // not a hostile-repository sandbox; adversarial fixtures stay out of this
    // benchmark until the validator has a separate OS isolation boundary.
    let mut command = tokio::process::Command::new("cargo");
    command
        .args(["test", "-q"])
        .current_dir(worktree)
        .env_clear()
        .envs(clean_environment())
        .env("CARGO_TARGET_DIR", target_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let Ok(mut child) = command.spawn() else {
        return Ok(false);
    };
    match tokio::time::timeout(VALIDATION_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => Ok(status.success()),
        Ok(Err(_)) => Ok(false),
        Err(_) => {
            #[cfg(unix)]
            let child_pid = child.id();
            #[cfg(unix)]
            let process_group_signal_sent = terminate_validation_process(&mut child);
            #[cfg(not(unix))]
            let process_group_signal_sent = false;
            if !process_group_signal_sent {
                let _ = child.kill().await;
            }
            match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                Ok(Ok(_)) if process_group_signal_sent => {
                    #[cfg(unix)]
                    {
                        let Some(pid) = child_pid else {
                            return Err(
                                "validation process exited without a process-group id".into()
                            );
                        };
                        wait_for_process_group_exit(pid).await?;
                    }
                    Ok(false)
                }
                Ok(Ok(_)) => Err(
                    "validation process-group cleanup could not be confirmed; refusing to score"
                        .into(),
                ),
                Ok(Err(error)) => {
                    Err(format!("validation process wait failed after timeout: {error}").into())
                }
                Err(_) => Err("validation process did not stop after timeout".into()),
            }
        }
    }
}

#[cfg(unix)]
fn terminate_validation_process(child: &mut tokio::process::Child) -> bool {
    let Some(pid) = child.id() else {
        return false;
    };
    let process_group = format!("-{pid}");
    let term_sent = std::process::Command::new("kill")
        .args(["-TERM", process_group.as_str()])
        .env_clear()
        .envs(clean_environment())
        .status()
        .is_ok_and(|status| status.success());
    let kill_sent = std::process::Command::new("kill")
        .args(["-KILL", process_group.as_str()])
        .env_clear()
        .envs(clean_environment())
        .status()
        .is_ok_and(|status| status.success());
    term_sent || kill_sent
}

#[cfg(unix)]
async fn wait_for_process_group_exit(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let process_group = format!("-{pid}");
    loop {
        let status = std::process::Command::new("kill")
            .args(["-0", process_group.as_str()])
            .env_clear()
            .envs(clean_environment())
            .status();
        match status {
            Ok(status) if !status.success() => return Ok(()),
            Err(error) => {
                return Err(
                    format!("could not verify validation process-group cleanup: {error}").into(),
                );
            }
            Ok(_) if Instant::now() >= deadline => {
                return Err("validation process-group descendants did not stop".into());
            }
            Ok(_) => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
}

/// Whether the agent has understood the task and started acting on it.
///
/// Both halves matter. A contract means the agent has decided what the task is;
/// an action means it has begun doing it. Correcting before either has happened
/// is just a second opening message.
async fn committed_to_a_plan(
    client: &reqwest::Client,
    options: &Options,
    session_id: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let base = options.daemon.trim_end_matches('/');
    let events: Vec<SessionEvent> = get_json(
        client,
        &format!("{base}/v1/sessions/{session_id}/events"),
        &options.token,
    )
    .await?;
    let has_contract = events
        .iter()
        .any(|event| matches!(event, SessionEvent::ExpectationContractCreated { .. }));
    let has_acted = events
        .iter()
        .any(|event| matches!(event, SessionEvent::ExecutionFinished { .. }));
    Ok(has_contract && has_acted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_directories_are_unique_and_existing_contents_are_preserved() {
        let first = unique_directory("alignment-runner-test").expect("first directory");
        fs::write(first.join("sentinel"), "keep").expect("sentinel");
        let second = unique_directory("alignment-runner-test").expect("second directory");

        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(first.join("sentinel")).unwrap(), "keep");
    }

    #[test]
    fn stopped_evidence_requires_an_explicit_false_lease_and_known_status() {
        let stopped = serde_json::json!({
            "status_code": "cancelled",
            "lease_active": false,
        });
        let active = serde_json::json!({
            "status_code": "active",
            "lease_active": false,
        });
        let missing_lease = serde_json::json!({"status_code": "cancelled"});

        assert!(session_stopped(&stopped));
        assert!(!session_stopped(&active));
        assert!(!session_stopped(&missing_lease));
    }

    #[test]
    fn worker_drain_evidence_fails_closed_for_missing_or_running_workers() {
        assert!(supervisor_workers_stopped(
            &serde_json::json!({ "workers": [] })
        ));
        assert!(supervisor_workers_stopped(&serde_json::json!({
            "workers": [{"status": "completed"}]
        })));
        assert!(!supervisor_workers_stopped(&serde_json::json!({
            "workers": [{"status": "running"}]
        })));
        assert!(!supervisor_workers_stopped(&serde_json::json!({
            "workers": [{"status": "finished"}]
        })));
        assert!(!supervisor_workers_stopped(&serde_json::json!({})));
    }

    #[test]
    fn validation_environment_does_not_forward_provider_variables() {
        let environment = clean_environment();
        assert!(environment.iter().all(|(key, _)| {
            !matches!(
                key.as_str(),
                "OPENAI_API_KEY" | "ANTHROPIC_API_KEY" | "NVIDIA_API_KEY" | "OLLAMA_API_KEY"
            )
        }));
    }

    #[test]
    fn infrastructure_outcome_leaves_forbidden_checks_unmeasured() {
        let task = default_catalog().into_iter().next().expect("catalog task");
        let outcome = failed_outcome(&task);

        assert_eq!(
            outcome.run.delivery_state.as_deref(),
            Some("infrastructure_error")
        );
        assert!(!outcome.violated_forbidden);
    }

    #[test]
    fn partial_report_is_written_as_valid_json() {
        let directory = unique_directory("alignment-report-test").expect("report directory");
        let path = directory.join("partial.json");
        write_json_atomic(&path, &AlignmentReport::new(Vec::new())).expect("atomic report");

        let parsed: AlignmentReport =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).expect("valid report");
        assert!(parsed.outcomes.is_empty());

        write_measurement_errors(
            &directory.join("partial.json"),
            &[MeasurementError {
                task_id: "task-1".into(),
                status: "infrastructure_error",
                detail: "drain not confirmed".into(),
            }],
        )
        .expect("atomic error sidecar");
        let errors: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(directory.join("partial.errors.json")).unwrap(),
        )
        .expect("valid error sidecar");
        assert_eq!(errors[0]["task_id"], "task-1");
        assert_eq!(errors[0]["status"], "infrastructure_error");
    }
}
