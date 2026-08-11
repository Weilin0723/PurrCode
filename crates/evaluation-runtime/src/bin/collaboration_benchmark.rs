//! Run the v1.4 collaborative benchmark against a live daemon (§PR15).
//!
//! ```text
//! purrcode-collaboration-benchmark \
//!     --daemon http://127.0.0.1:4477 --token "$PURRCODE_TOKEN" \
//!     --repository /path/to/fixture --out target/collaboration-report.md
//! ```
//!
//! Each task runs twice against a **fresh copy** of the fixture repository:
//! once with delegation available and once without. Both arms are measured from
//! their durable session logs, never from what the agent said it did.
//!
//! This binary deliberately does no scoring of its own — it collects sessions
//! and hands them to `collaboration::CollaborationComparison::score`, which the
//! unit tests cover. What lives here is the part that cannot be unit-tested: an
//! HTTP conversation with a real daemon and a real model.

use purrcode_evaluation_runtime::collaboration::{
    CollaborationComparison, CollaborationReport, CollaborationRun, CollaborationTask,
    default_catalog,
};
use purrcode_runtime_core::{SessionEvent, SessionState};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Options {
    daemon: String,
    token: String,
    repository: PathBuf,
    out: Option<PathBuf>,
    only: Option<String>,
}

fn usage() -> ! {
    eprintln!(
        "usage: collaboration-benchmark --daemon URL --token TOKEN --repository PATH \\\n\
         \x20                            [--out FILE] [--only TASK_ID]\n\n\
         Runs each catalog task twice — single-agent and collaborative — and reports\n\
         whether delegation earned its coordination cost."
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
    let catalog: Vec<CollaborationTask> = default_catalog()
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

    let mut comparisons = Vec::new();
    for task in &catalog {
        eprintln!("▶ {} ({})", task.id, task.category.label());
        // Each arm gets its own copy of the fixture: an arm that inherits the
        // other's changes is not running the same task.
        let single_repository = clone_fixture(&options.repository, &format!("{}-single", task.id))?;
        let single = run_arm(&client, &options, task, &single_repository, false).await?;
        eprintln!(
            "  single:        {} model call(s), {} tokens, completed={}",
            single.model_calls,
            single.total_tokens(),
            single.arm_completed
        );

        let collaborative_repository =
            clone_fixture(&options.repository, &format!("{}-collab", task.id))?;
        let collaborative =
            run_arm(&client, &options, task, &collaborative_repository, true).await?;
        eprintln!(
            "  collaborative: {} model call(s), {} tokens, {} worker(s), completed={}",
            collaborative.model_calls,
            collaborative.total_tokens(),
            collaborative.workers,
            collaborative.arm_completed
        );

        comparisons.push(CollaborationComparison::score(task, single, collaborative));
    }

    let report = CollaborationReport::new(comparisons);
    let markdown = report.to_markdown();
    match &options.out {
        Some(path) => {
            std::fs::write(path, &markdown)?;
            eprintln!("\nwrote {}", path.display());
        }
        None => println!("{markdown}"),
    }
    // A non-zero exit when the bar is not met, so CI can gate on it.
    if report.meets_release_bar() {
        Ok(())
    } else {
        eprintln!("the collaborative benchmark did not meet the v1.4 release bar");
        std::process::exit(1)
    }
}

/// Copy the fixture repository so each arm starts from the same clean state.
fn clone_fixture(source: &Path, suffix: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let destination = std::env::temp_dir().join(format!("purrcode-benchmark-{suffix}"));
    if destination.exists() {
        std::fs::remove_dir_all(&destination)?;
    }
    let status = std::process::Command::new("git")
        .args([
            "clone",
            "--quiet",
            "--no-hardlinks",
            &source.display().to_string(),
            &destination.display().to_string(),
        ])
        .status()?;
    if !status.success() {
        return Err(format!("could not clone the fixture into {}", destination.display()).into());
    }
    Ok(destination)
}

/// Run one arm to completion and measure it from the durable log.
async fn run_arm(
    client: &reqwest::Client,
    options: &Options,
    task: &CollaborationTask,
    repository: &Path,
    collaborative: bool,
) -> Result<CollaborationRun, Box<dyn std::error::Error>> {
    let started = Instant::now();
    // The two arms differ in exactly one way: whether the objective invites a
    // split. Nothing else — same model, same policy, same fixture — so the
    // comparison measures delegation and not two different tasks.
    let objective = if collaborative {
        task.objective.clone()
    } else {
        format!(
            "{} Work on this yourself in this session; do not delegate.",
            task.objective
        )
    };
    let created: serde_json::Value = client
        .post(format!(
            "{}/v1/sessions",
            options.daemon.trim_end_matches('/')
        ))
        .bearer_auth(&options.token)
        .json(&serde_json::json!({
            "objective": objective,
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

    // Wait for the session to leave a live state. A task that never settles is
    // reported as an incomplete arm rather than hanging the benchmark.
    let deadline = std::time::Duration::from_secs(task.maximum_seconds);
    loop {
        let session: serde_json::Value = client
            .get(format!(
                "{}/v1/sessions/{session_id}",
                options.daemon.trim_end_matches('/')
            ))
            .bearer_auth(&options.token)
            .send()
            .await?
            .json()
            .await?;
        let status = session["status"].as_str().unwrap_or_default().to_owned();
        let settled = matches!(
            status.as_str(),
            "completed" | "failed" | "cancelled" | "awaiting_review" | "awaiting_approval"
        );
        if settled || started.elapsed() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    let events: Vec<SessionEvent> = client
        .get(format!(
            "{}/v1/sessions/{session_id}/events",
            options.daemon.trim_end_matches('/')
        ))
        .bearer_auth(&options.token)
        .send()
        .await?
        .json()
        .await?;
    let mut state = SessionState::empty(purrcode_runtime_core::SessionId(session_id.parse()?));
    for event in &events {
        state.reduce_event(event)?;
    }
    Ok(CollaborationRun::from_session(
        &state,
        &events,
        started.elapsed().as_secs(),
    ))
}
