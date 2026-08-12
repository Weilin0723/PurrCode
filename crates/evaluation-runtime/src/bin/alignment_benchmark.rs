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
    default_catalog,
};
use purrcode_runtime_core::{SessionEvent, SessionState};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Options {
    daemon: String,
    token: String,
    repository: PathBuf,
    out: Option<PathBuf>,
    only: Option<String>,
}

fn usage() -> ! {
    eprintln!(
        "usage: alignment-benchmark --daemon URL --token TOKEN --repository PATH \\\n\
         \x20                        [--out FILE] [--only TASK_ID]\n\n\
         Runs the twenty-task alignment catalog and reports whether the agent finished\n\
         the task the user asked for — and stopped when it hadn't."
    );
    std::process::exit(2)
}

fn parse_options() -> Options {
    let mut daemon = None;
    let mut token = None;
    let mut repository = None;
    let mut out = None;
    let mut only = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--daemon" => daemon = Some(value()),
            "--token" => token = Some(value()),
            "--repository" => repository = Some(PathBuf::from(value())),
            "--out" => out = Some(PathBuf::from(value())),
            "--only" => only = Some(value()),
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    Options {
        daemon: daemon.unwrap_or_else(|| usage()),
        token: token.unwrap_or_else(|| usage()),
        repository: repository.unwrap_or_else(|| usage()),
        out,
        only,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options();
    let client = reqwest::Client::new();
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
    for task in &catalog {
        eprintln!("▶ {} ({})", task.id, task.kind.label());
        let repository = clone_fixture(&options.repository, &task.id)?;
        let outcome = run_task(&client, &options, task, &repository).await?;
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
    // A non-zero exit when the bar is not met, so this is usable from CI
    // without a human reading the markdown.
    if report.meets_release_bar() {
        Ok(())
    } else {
        eprintln!("the release bar was not met");
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
    let destination = std::env::temp_dir().join(format!("purrcode-alignment-{suffix}"));
    if destination.exists() {
        std::fs::remove_dir_all(&destination)?;
    }
    copy_tree(source, &destination)?;
    let git = |arguments: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        let status = std::process::Command::new("git")
            .args(arguments)
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

/// Copy a directory tree, skipping build output and any repository metadata.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git") | Some("target")) {
            continue;
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
    let created: serde_json::Value = client
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(&options.token)
        .json(&serde_json::json!({
            "objective": task.objective,
            "repository": repository,
            "task_mode": "build",
            "permission_mode": "auto",
        }))
        .send()
        .await?
        .json()
        .await?;
    let session_id = created["id"]
        .as_str()
        .ok_or("the daemon did not return a session id")?
        .to_owned();

    let deadline = Duration::from_secs(task.maximum_seconds);
    let mut corrections_issued = 0_u32;
    let mut correction_pending = task.correction.clone();

    loop {
        let session: serde_json::Value = client
            .get(format!("{base}/v1/sessions/{session_id}"))
            .bearer_auth(&options.token)
            .send()
            .await?
            .json()
            .await?;
        let status = session["status"].as_str().unwrap_or_default().to_owned();

        // The correction goes in once the agent has a contract and has started
        // acting on it. Sending it before there is a plan to correct tests
        // nothing — the trap is that work already done under the old wording
        // must stop counting.
        if let Some(correction) = correction_pending.clone()
            && committed_to_a_plan(client, options, &session_id).await?
        {
            client
                .post(format!("{base}/v1/sessions/{session_id}/messages"))
                .bearer_auth(&options.token)
                .json(&serde_json::json!({ "content": correction }))
                .send()
                .await?;
            corrections_issued += 1;
            correction_pending = None;
            eprintln!("  ↩ sent the correction");
        }

        let settled = matches!(
            status.as_str(),
            "completed" | "failed" | "cancelled" | "awaiting_review" | "awaiting_approval"
        );
        // A correction that has not been delivered yet keeps the run going: a
        // session that finished before the harness could correct it did not run
        // this task.
        if settled && correction_pending.is_none() {
            break;
        }
        if started.elapsed() >= deadline {
            eprintln!("  ⏱ timed out");
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let events: Vec<SessionEvent> = client
        .get(format!("{base}/v1/sessions/{session_id}/events"))
        .bearer_auth(&options.token)
        .send()
        .await?
        .json()
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
    let satisfied = task
        .requirement_checks
        .iter()
        .filter(|check| check.evaluate(&tree, repository))
        .count() as u32;
    for check in &task.requirement_checks {
        if !check.evaluate(&tree, repository) {
            eprintln!("  ✗ {}", check.label());
        }
    }
    // A forbidden check states what must still be true. One that fails is the
    // forbidden thing having happened.
    let mut violated_forbidden = false;
    for check in &task.forbidden_checks {
        if !check.evaluate(&tree, repository) {
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
    let events: Vec<SessionEvent> = client
        .get(format!("{base}/v1/sessions/{session_id}/events"))
        .bearer_auth(&options.token)
        .send()
        .await?
        .json()
        .await?;
    let has_contract = events
        .iter()
        .any(|event| matches!(event, SessionEvent::ExpectationContractCreated { .. }));
    let has_acted = events
        .iter()
        .any(|event| matches!(event, SessionEvent::ExecutionFinished { .. }));
    Ok(has_contract && has_acted)
}
